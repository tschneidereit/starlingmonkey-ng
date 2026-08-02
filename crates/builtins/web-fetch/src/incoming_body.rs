// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Streaming an incoming body straight from the host.
//!
//! Used as the underlying source for a `body` [`ReadableStream`], or directly in
//! [`consume_host_body`] for `text`/`json`/`arrayBuffer`/`bytes`.

use crate::algorithms::{convert_owned_bytes_to_js_value, ConsumeType};
use core_runtime::jsclass;
use core_runtime::jsmethods;
use js::conversion::FromJSVal;
use js::error::{ExnThrown, TypeError};
use js::function::CallbackArgs;
use js::gc::handle::{Heap, OptionHeapExt, RootedHeap};
use js::gc::scope::Scope;
use js::native::Value;
use js::prelude::HandleValue;
use js::promise::{PromiseFuture, PromiseOutcome};
use js::{value, Function, Promise};
use platform::http::IncomingBody;
use web_streams::readable::readable_stream::ReadableStream;
use web_streams::readable::ReadableStreamDefaultController;

/// Back a new external `ArrayBuffer` with `bytes`, zero-copy when the chunk is
/// uniquely owned. A shared chunk (another `Bytes` clone alive, e.g. a body
/// source retained elsewhere) is copied instead: JS gets write access to the
/// buffer, so it must not alias memory other owners can still read.
pub(crate) fn array_buffer_from_body_bytes<'r>(
    scope: &'r Scope<'_>,
    bytes: platform::http::BodyBytes,
) -> Result<js::ArrayBuffer<'r>, ExnThrown> {
    /// Uniquely-owned host bytes.
    struct UniqueBytes(bytes::BytesMut);
    // SAFETY: `BytesMut` guarantees exclusive ownership of its range, and the
    // storage is heap-backed (stable behind the moved handle).
    unsafe impl js::typedarray::ExternalBytes for UniqueBytes {
        fn as_mut_slice(&mut self) -> &mut [u8] {
            &mut self.0
        }
    }

    match bytes.try_into_mut() {
        Ok(unique) => js::ArrayBuffer::from_external(scope, UniqueBytes(unique)),
        Err(shared) => js::ArrayBuffer::from_external(scope, shared.to_vec()),
    }
}

/// `consume body` for a host-backed body: read the whole body from the host and
/// settle the returned promise with the converted value, bypassing `ReadableStream`.
pub(crate) fn consume_host_body<'r>(
    scope: &'r Scope<'_>,
    host_body: IncomingBody,
    conversion: ConsumeType,
) -> Result<Promise<'r>, ExnThrown> {
    let promise = Promise::new_pending(scope)?;
    let future = async move {
        match host_body.read_all().await {
            Ok(bytes) => PromiseOutcome::Resolve(Box::new(move |scope: &Scope<'_>| {
                // The whole body is owned here, so arrayBuffer()/bytes() back the result with
                // it directly (no copy when aligned).
                convert_owned_bytes_to_js_value(scope, bytes, conversion)
            })),
            Err(error) => PromiseOutcome::Reject(format!("Failed to read response body: {error}")),
        }
    };
    promise.spawn(PromiseFuture::new(future));
    Ok(promise)
}

/// Native byte source for a host-backed body's `.body` stream: holds the host
/// body and hands out one chunk per pull. `current_pull` is the promise of the
/// in-flight chunk read, kept so an abort can cancel that read (dropping it
/// closes the host connection).
#[jsclass(hidden)]
pub struct HostBodySource {
    #[no_trace]
    host_body: Option<IncomingBody>,
    current_pull: Option<Heap<js::promise::Promise>>,
    #[no_trace]
    pulled: bool,
}

#[jsmethods]
impl HostBodySource {
    fn new(host_body: IncomingBody) -> Self {
        HostBodySourceImpl {
            host_body: Some(host_body),
            current_pull: None,
            pulled: false,
        }
    }
}

impl HostBodySource<'_> {
    /// Take the host body if it has not started being read, for the
    /// incoming→outgoing shortcut. Returns `None` once a pull is in flight or the
    /// body is already gone.
    pub(crate) fn take_host_body(&self) -> Option<IncomingBody> {
        if self.data().pulled || self.data().current_pull.is_some() {
            return None;
        }
        self.data_mut().host_body.take()
    }

    /// Abort the body: cancel the in-flight chunk read (if any) — which drops the
    /// host body and closes the connection — and drop any idle host body.
    pub(crate) fn abort(&self, scope: &Scope<'_>) {
        if let Some(pull) = self.data_mut().current_pull.take_rooted(scope) {
            js::promise::cancel_pending_future(pull);
        }
        self.data_mut().host_body = None;
    }
}

/// A `Request` or `Response` carrying a body still unread on the host.
///
/// Both keep the same quartet of fields — the body record, the unread host body,
/// the `.body` stream it materializes into, and that stream's native source — so
/// the operations over them ([`materialize_host_body`], [`clone_body_onto`],
/// [`crate::outgoing_body::outgoing_body`]) are written here once rather than
/// once per interface. The
/// accessors are deliberately fine-grained rather than one `&mut`-fields
/// accessor, because a caller must not hold a borrow of the object's data across
/// [`host_body_stream`]: allocating the stream can trigger a GC, whose trace of
/// this object reads the same data.
pub(crate) trait HostBackedBodyOwner {
    /// Take the host body, but only while it is still unread — once a `.body`
    /// stream exists it owns the body, and the caller has nothing to do.
    fn take_unread_host_body(&self) -> Option<IncomingBody>;

    /// Store the stream the host body materialized into, and its native source.
    fn set_host_body_stream(&self, stream: ReadableStream<'_>, source: HostBodySource<'_>);

    /// The body record, if this object has a body.
    fn body_record(&self) -> Option<crate::algorithms::Body>;

    /// The `.body` stream, once materialized.
    fn body_stream<'r>(&self, scope: &'r Scope<'_>) -> Option<ReadableStream<'r>>;

    /// Take the host body whether or not a stream exists (the outgoing path,
    /// which hands it straight to the transport).
    fn take_host_body(&self) -> Option<IncomingBody>;

    /// Replace the `.body` stream, and drop the byte and host sources it now
    /// supersedes — used after teeing, when the bytes live in the tee branches.
    fn replace_body_stream_after_tee(&self, scope: &Scope<'_>, stream: ReadableStream<'_>);
}

/// `clone a body` for a host-backed owner: materialize the stream if the body is
/// still on the host, tee it, leave branch one behind on `self`, and return the
/// cloned body record with branch two for the caller to install on the clone.
///
/// Shared by `Request.clone()` and `Response.clone()`, whose only real
/// difference is which object they build at the end.
pub(crate) fn clone_body_onto<'r>(
    scope: &'r Scope<'_>,
    object: &impl HostBackedBodyOwner,
) -> Result<(Option<crate::algorithms::Body>, Option<ReadableStream<'r>>), ExnThrown> {
    // A host-backed body has no materialized stream yet; bring it into existence (as the `.body`
    // getter does) so `clone a body` has a stream to tee.
    materialize_host_body(scope, object)?;
    // With no stream (an in-memory byte source, or no body at all) there is nothing to tee — copy
    // the body record and let each side materialize its own stream from its own byte copy.
    let Some(stream) = object.body_stream(scope) else {
        return Ok((object.body_record(), None));
    };
    let (out2, cloned_body) = crate::algorithms::clone_a_body(scope, object, &stream)?;
    Ok((Some(cloned_body), Some(out2)))
}

/// Bring a host-backed body's `.body` stream into existence, so `.body` has a
/// stream to hand out and `clone a body` has one to tee. A no-op for a body that
/// is not host-backed, or whose stream already exists.
pub(crate) fn materialize_host_body(
    scope: &Scope<'_>,
    object: &impl HostBackedBodyOwner,
) -> Result<(), ExnThrown> {
    let Some(host_body) = object.take_unread_host_body() else {
        return Ok(());
    };
    let (stream, source) = host_body_stream(scope, host_body)?;
    object.set_host_body_stream(stream, source);
    Ok(())
}

/// Build the `.body` `ReadableStream` for a host-backed body: its underlying
/// source reads the host body one chunk per pull and enqueues it, closing at end
/// of body, so streaming or unbounded bodies are delivered progressively.
pub(crate) fn host_body_stream<'r>(
    scope: &'r Scope<'_>,
    host_body: IncomingBody,
) -> Result<(ReadableStream<'r>, HostBodySource<'r>), ExnThrown> {
    let state = HostBodySource::new(scope, host_body)?;
    // TODO: check if `pull` and `cancel` could be per-global singletons and read `state` from their args.
    let pull = Function::new_callback(scope, c"", 1, host_pull, state)?;
    let cancel = Function::new_callback(scope, c"", 1, host_cancel, state)?;
    let pull_value = scope.root_value(pull.as_value());
    let cancel_value = scope.root_value(cancel.as_value());
    // Record the source so the stream can be recognized as host-backed via
    // `ReadableStream::native_source`.
    let stream = ReadableStream::new_native(scope, state, pull_value, cancel_value)?;
    Ok((stream, state))
}

/// Underlying-source `pull`: read the next host chunk and enqueue it (or close at
/// end of body / error on a read failure). Returns a promise resolved once the
/// chunk has been handled.
fn host_pull(
    scope: &Scope<'_>,
    args: CallbackArgs<'_>,
    payload: HandleValue<'_>,
) -> Result<Value, ExnThrown> {
    let state = HostBodySource::from_jsval_throwing(scope, payload, ())?;
    state.data_mut().pulled = true;
    let promise = Promise::new_pending(scope)?;
    let controller = ReadableStreamDefaultController::from_jsval_throwing(scope, args.get(0), ())?;
    let Some(host_body) = state.data_mut().host_body.take() else {
        // The body is already gone: the incoming→outgoing shortcut took it via a native source
        // propagated across an identity transform. The shortcut locks-and-disturbs the
        // *transform's* readable, but the original stream stays locked to the pipe feeding the
        // transform, and that pipe's next read lands here. Close the stream so the pipe winds
        // down instead of hanging on a chunk that will never arrive.
        controller.close(scope)?;
        promise.resolve(scope, HandleValue::undefined())?;
        return Ok(promise.as_value());
    };
    // Remember the in-flight read so an abort can cancel it.
    state.data_mut().current_pull = Some(Heap::from(promise));
    // The controller and state are used after the host read, keep them rooted across it.
    let controller = RootedHeap::new(controller);
    let state = RootedHeap::new(state);
    let future = async move {
        let mut host_body = host_body;
        let result = host_body.next_chunk().await;
        PromiseOutcome::Resolve(Box::new(move |scope: &Scope<'_>| {
            deliver_chunk(
                scope,
                controller.get(scope),
                state.get(scope),
                result,
                host_body,
            )
            .map(|_| HandleValue::undefined())
        }))
    };
    promise.spawn(PromiseFuture::new(future));
    Ok(promise.as_value())
}

/// Enqueue `result`'s chunk into the controller (keeping the host body for the
/// next pull), close at end of body, or error the stream on a read failure.
fn deliver_chunk(
    scope: &Scope<'_>,
    controller: ReadableStreamDefaultController,
    state: HostBodySource,
    result: Result<Option<platform::http::BodyBytes>, platform::http::Error>,
    host_body: IncomingBody,
) -> Result<(), ExnThrown> {
    // This pull has completed and can no longer be canceled.
    state.data_mut().current_pull = None;
    match result {
        Ok(Some(bytes)) => {
            // More may follow: keep the host body for the next pull.
            state.data_mut().host_body = Some(host_body);
            // Hand the host bytes to the engine without copying when they are uniquely owned
            // and aligned (a fresh host read buffer is both); shared or unaligned chunks are
            // copied.
            let len = bytes.len();
            let buffer = array_buffer_from_body_bytes(scope, bytes)?;
            let chunk = js::typedarray::construct_view(
                scope,
                js::typedarray::ViewKind::Uint8,
                buffer,
                0,
                len,
            )?;
            let chunk_value = scope.root_value(chunk.as_value());
            // Deliver via the controller abstract operation, not the
            // author-patchable `…prototype.enqueue` — a page must not be able to
            // observe or corrupt the network body's bytes.
            controller.enqueue(scope, chunk_value)?;
        }
        // End of body (host body dropped): close the stream.
        Ok(None) => {
            controller.close(scope)?;
        }
        // Read failure (host body dropped): error the stream.
        Err(error) => {
            let _ = TypeError(format!("Failed to read response body: {error}")).throw(scope);
            let reason = js::exception::take_pending(scope).map_err(|_| ExnThrown)?;
            controller.error(scope, Some(reason))?;
        }
    }
    Ok(())
}

/// Underlying-source `cancel`: drop the host body, ending the host read.
fn host_cancel(
    scope: &Scope<'_>,
    _args: CallbackArgs<'_>,
    payload: HandleValue<'_>,
) -> Result<Value, ExnThrown> {
    let state = HostBodySource::from_jsval_throwing(scope, payload, ())?;
    state.data_mut().host_body = None;
    Ok(value::undefined())
}
