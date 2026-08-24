// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Handing a JS body stream to the host transport as an outgoing body.
//!
//! This implements `HTTP-network fetch` Step 8.5's "transmit _request_'s `body`"
//! (<https://fetch.spec.whatwg.org/#concept-http-network-fetch>), i.e.
//! `incrementally read a body` and the `incrementally-read loop`
//! (<https://fetch.spec.whatwg.org/#body-incrementally-read>): each chunk is
//! delivered to native steps (_processBodyChunk_ = send through the channel),
//! end-of-body closes the channel, and an error or non-`Uint8Array` chunk aborts
//! the body.
//!
//! A body whose stream came straight from the host and has never been read is
//! handed through untouched. Otherwise, the stream is pumped: chunks from an
//! internal reader are sent through a channel which the platform transport reads
//! as it sends the body.

use core_runtime::jsclass;
use core_runtime::jsmethods;
use js::class::create_instance_with;
use js::conversion::FromJSVal;
use js::error::ExnThrown;
use js::gc::handle::{Heap, RootedHeap};
use js::gc::scope::Scope;
use js::prelude::HandleValue;
use js::promise::{PromiseFuture, PromiseOutcome};
use js::{Object, Promise, Uint8Array};
use platform::http::{BodySender, OutgoingBody};
use web_streams::readable::default_reader::DefaultReaderImpl;
use web_streams::readable::native_read::{
    acquire_native_reader, native_reader_read, NativeReadSteps,
};
use web_streams::readable::readable_stream::ReadableStream;

use crate::incoming_body::HostBackedBodyOwner;

/// A running tally of how outgoing bodies reached the transport.
///
/// Whether a host body is handed to the wire whole or pumped through JS chunk by chunk is
/// deliberately not observable from content — both deliver the same bytes, leave the donor stream
/// locked and disturbed, and read only internally. That also means losing the shortcut is invisible:
/// every byte would start travelling through JS and nothing would fail. These counters are the one
/// place that difference is visible, so a test can hold the choice in place.
///
/// Whether an in-memory body's owner let go of its buffer on the way out is invisible the same way:
/// the same bytes reach the wire either way, and only how long the buffer outlives them differs.
pub mod paths_taken {
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Bodies handed to the transport whole, host body and all.
    pub static SHORTCUT: AtomicUsize = AtomicUsize::new(0);
    /// Bodies pumped through JS, chunk by chunk.
    pub static PUMPED: AtomicUsize = AtomicUsize::new(0);
    /// Byte bodies handed over as the only reference left to their buffer.
    pub static SOLE_BYTES: AtomicUsize = AtomicUsize::new(0);
    /// Byte bodies whose owner still held a reference of its own when it handed them over.
    pub static SHARED_BYTES: AtomicUsize = AtomicUsize::new(0);

    /// `(shortcut, pumped)` since the last [`reset`].
    pub fn counts() -> (usize, usize) {
        (
            SHORTCUT.load(Ordering::Relaxed),
            PUMPED.load(Ordering::Relaxed),
        )
    }

    /// `(sole, shared)` since the last [`reset`], counting non-empty byte bodies only.
    pub fn byte_counts() -> (usize, usize) {
        (
            SOLE_BYTES.load(Ordering::Relaxed),
            SHARED_BYTES.load(Ordering::Relaxed),
        )
    }

    /// Start counting again. Callers that compare counts must not run concurrently with other
    /// traffic on the same process.
    pub fn reset() {
        SHORTCUT.store(0, Ordering::Relaxed);
        PUMPED.store(0, Ordering::Relaxed);
        SOLE_BYTES.store(0, Ordering::Relaxed);
        SHARED_BYTES.store(0, Ordering::Relaxed);
    }

    pub(super) fn note(body: &platform::http::OutgoingBody) {
        match body {
            platform::http::OutgoingBody::Host(_) => &SHORTCUT,
            platform::http::OutgoingBody::Stream(_) => &PUMPED,
            // Empty ones sit out: there is no buffer to hold on to, and `Bytes::is_unique` answers
            // for the static one an empty body carries as though it were shared.
            platform::http::OutgoingBody::Bytes(bytes) if !bytes.is_empty() => {
                if bytes.is_unique() {
                    &SOLE_BYTES
                } else {
                    &SHARED_BYTES
                }
            }
            // Neither route, and nothing held: an empty body or none at all.
            _ => return,
        }
        .fetch_add(1, Ordering::Relaxed);
    }
}

/// The body to send on the wire for a host-backed owner.
///
/// With `start_reading` false a `ReadableStream` body is canceled instead of being read, which
/// would call its underlying source's `pull()` method.
pub(crate) fn consume_outgoing_body(
    scope: &Scope<'_>,
    object: &impl HostBackedBodyOwner,
    start_reading: bool,
) -> OutgoingBody {
    let body = outgoing_body_inner(scope, object, start_reading);
    paths_taken::note(&body);
    body
}

fn outgoing_body_inner(
    scope: &Scope<'_>,
    object: &impl HostBackedBodyOwner,
    start_reading: bool,
) -> OutgoingBody {
    if let Some(bytes) = object.take_byte_source() {
        return OutgoingBody::Bytes(bytes);
    }
    if let Some(host_body) = object.take_host_body() {
        return OutgoingBody::Host(host_body);
    }
    match object.body_stream(scope) {
        Some(stream) => outgoing_body_from_stream(scope, stream, start_reading),
        None => OutgoingBody::Bytes(bytes::Bytes::new()),
    }
}

/// State for the body pump: the reader being drained and the channel's write
/// end. The sender is dropped (closing the body) when the stream ends or errors.
#[jsclass(hidden)]
pub struct OutgoingBodyPump {
    reader: Heap<DefaultReaderImpl>,
    #[no_trace]
    sender: Option<BodySender>,
}

#[jsmethods]
impl OutgoingBodyPump {
    fn new() -> Self {
        // Internal type, always created with a real reader/sender via `create_instance_with`; this
        // hidden constructor exists only so the prototype is registered.
        OutgoingBodyPumpImpl {
            reader: Heap::default(),
            sender: None,
        }
    }
}

/// Turn a body's materialized `ReadableStream` into a [`OutgoingBody`].
///
/// A host body that has never been read is handed straight through; anything
/// else is pumped through a `DefaultReader`, or cancelled unsent where
/// `start_reading` is false.
///
/// By this point the request/response is committed to being sent, so a failure
/// to start reading cannot be thrown to the caller. It becomes a body that
/// fails when the transport reads it, which fails the send.
pub(crate) fn outgoing_body_from_stream(
    scope: &Scope<'_>,
    stream: ReadableStream<'_>,
    start_reading: bool,
) -> OutgoingBody {
    // The shortcut refuses once the body has actually been read from: a chunk may sit in a
    // buffer the shortcut cannot see (a transform's queue), and bypassing the stream would
    // drop those bytes. The pump drains the stream in order instead. A pipe that has merely
    // _asked_ for a chunk does not count: that pull is deferred without touching the host body
    // (see `incoming_body::host_pull`).
    let host_body = stream
        .native_source(scope)
        .and_then(|source| source.cast::<crate::incoming_body::HostBodySource>().ok())
        .and_then(|source| source.take_host_body(scope));
    match host_body {
        Some(host_body) => {
            // The bytes are transmitted directly, bypassing the stream, so leave the donor
            // stream in the same locked + disturbed state the pump path (acquiring a reader)
            // would: author code can no longer read it and `bodyUsed` reports true.
            stream
                .lock_and_disturb(scope)
                .expect("If this happens, there's a bug in the runtime");
            OutgoingBody::Host(host_body)
        }
        // Cancelled rather than left alone: nothing will read this stream now, and cancelling is
        // what makes it release whatever it draws from — an upstream response feeding a transform,
        // say. It runs the handler's `cancel()`, not its `pull()`, so no content is produced.
        None if !start_reading => {
            let promise = stream.cancel_internal(scope, HandleValue::undefined());
            // Rejects if `cancel()` throws. The response carries no body either way and there is no
            // caller left to tell, so mark it handled rather than announcing an unhandled rejection.
            let _ = promise.set_any_is_handled(scope);
            OutgoingBody::Consumed
        }
        None => pump_body_from_stream(scope, stream).unwrap_or_else(|_| {
            // The pump could not be started, so the exception it left pending has no caller to
            // propagate to. Clear it rather than leaving it to surface at an unrelated point,
            // and let the failing body carry the failure instead.
            let _ = js::exception::take_pending(scope);
            platform::http::failed_body("the request body stream could not be read".to_string())
        }),
    }
}

/// Start pumping `stream` into a new streaming [`OutgoingBody`]: returns the body
/// (handed to the platform transport) and kicks off the read loop.
fn pump_body_from_stream(
    scope: &Scope<'_>,
    stream: ReadableStream<'_>,
) -> Result<OutgoingBody, ExnThrown> {
    let (sender, body) = platform::http::body_channel();
    let reader = acquire_native_reader(scope, &stream)?;
    let state = create_instance_with::<OutgoingBodyPumpImpl>(scope, |_| OutgoingBodyPumpImpl {
        reader: Heap::from(reader),
        sender: Some(sender),
    })?;
    read_next(scope, &state)?;
    Ok(body)
}

/// The pump's native read-request steps. Each chunk pauses the loop until the
/// channel accepted it (`spawn_send` resumes via [`read_next`]), so the pump
/// never outruns the peer and queued chunks cannot recurse.
const PUMP_STEPS: NativeReadSteps = NativeReadSteps {
    chunk: pump_chunk_step,
    close: pump_close_step,
    error: pump_error_step,
};

/// Issue the next internal read, delivering to the pump's steps.
fn read_next(scope: &Scope<'_>, state: &OutgoingBodyPump<'_>) -> Result<(), ExnThrown> {
    let reader = state.data().reader.get(scope);
    native_reader_read(scope, reader, PUMP_STEPS, state.as_object())
}

/// A pending send to the body channel: a data chunk (after which the next chunk
/// is read) or a terminal error (after which the pump stops).
enum Send {
    Chunk(Vec<u8>),
    Error(String),
}

/// Chunk steps: send a `Uint8Array` chunk to the channel; anything else aborts
/// the body (the spec's transmit-body chunk steps treat a non-`Uint8Array`
/// chunk as a fatal error).
fn pump_chunk_step(
    scope: &Scope<'_>,
    payload: Object<'_>,
    chunk: HandleValue<'_>,
) -> Result<(), ExnThrown> {
    let state = payload
        .cast::<OutgoingBodyPump>()
        .expect("payload is an OutgoingBodyPump");
    let chunk_array = Uint8Array::from_jsval(scope, chunk, ()).ok();
    match chunk_array {
        // SAFETY: the slice is copied immediately; nothing here can GC or
        // detach the chunk's buffer.
        Some(array) => {
            let bytes = unsafe { array.as_slice() }.to_vec();
            spawn_send(scope, &state, Send::Chunk(bytes));
        }
        None => spawn_send(
            scope,
            &state,
            Send::Error("Body stream chunks must be of type Uint8Array".to_string()),
        ),
    }
    Ok(())
}

/// Close steps: end of body — drop the sender, closing the channel.
fn pump_close_step(scope: &Scope<'_>, payload: Object<'_>) -> Result<(), ExnThrown> {
    let state = payload
        .cast::<OutgoingBodyPump>()
        .expect("payload is an OutgoingBodyPump");
    let _ = scope;
    state.data_mut().sender = None;
    Ok(())
}

/// Error steps: abort the body with the stream's error.
fn pump_error_step(
    scope: &Scope<'_>,
    payload: Object<'_>,
    _error: HandleValue<'_>,
) -> Result<(), ExnThrown> {
    let state = payload
        .cast::<OutgoingBodyPump>()
        .expect("payload is an OutgoingBodyPump");
    spawn_send(
        scope,
        &state,
        Send::Error("body stream errored".to_string()),
    );
    Ok(())
}

/// Send `item` to the body channel (taking the sender out of the pump), awaiting
/// channel capacity for backpressure. For a chunk, the next `read()` is issued
/// only once the chunk is accepted (so the pump never outruns the peer); for an
/// error, the pump stops and the sender is dropped (closing the channel after the
/// queued error).
fn spawn_send(scope: &Scope<'_>, state: &OutgoingBodyPump<'_>, item: Send) {
    let Some(sender) = state.data_mut().sender.take() else {
        return;
    };
    let Ok(promise) = Promise::new_pending(scope) else {
        return;
    };
    let _ = promise.set_any_is_handled(scope);
    let state = RootedHeap::new(*state);
    let is_chunk = matches!(item, Send::Chunk(_));
    // Ensure the event loop stays alive until the receiver accepts the chunk or error.
    let interest =
        core_runtime::event_loop::with_active_event_loop(|el| el.acquire_interest_handle());
    let future = async move {
        let mut sender = sender;
        let accepted = match item {
            Send::Chunk(bytes) => sender.send_chunk(bytes).await,
            Send::Error(message) => {
                sender.send_error(message).await;
                false
            }
        };
        PromiseOutcome::Resolve(Box::new(move |scope: &Scope<'_>| {
            drop(interest);
            if is_chunk {
                let state = state.get(scope);
                if accepted {
                    // For an accepted chunk, restore the sender and read the next
                    // one.
                    state.data_mut().sender = Some(sender);
                    let _ = read_next(scope, &state);
                } else {
                    // A refused chunk means the receiver is gone: the peer stopped reading the
                    // body, typically because the client disconnected. Cancel the stream, and
                    // transitively the underlying source, so it can stop producing chunks.
                    let reader = state.data().reader.get(scope);
                    match reader.cancel(scope, None) {
                        // Cancelling rejects if the underlying source's `cancel()` throws.
                        // There is no caller left to report that to: the body is already
                        // undeliverable. Mark it handled so it is not announced as an unhandled
                        // rejection on top.
                        Ok(promise) => {
                            let _ = promise.set_any_is_handled(scope);
                        }
                        Err(_) => js::exception::report_and_clear(
                            scope,
                            "cancelling an abandoned response body",
                        ),
                    }
                }
            }
            Ok(HandleValue::undefined())
        }))
    };
    promise.spawn(PromiseFuture::new(future));
}
