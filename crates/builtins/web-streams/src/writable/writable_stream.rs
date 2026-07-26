// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/>

use super::algorithms;
use super::default_controller::WritableStreamDefaultControllerImpl;
use super::default_writer::WritableStreamDefaultWriter;
use super::default_writer::WritableStreamDefaultWriterImpl;
use super::underlying_sink::UnderlyingSink;
use crate::algorithms::extract_high_water_mark;
use crate::algorithms::extract_size_algorithm;
use crate::queuing::QueuingStrategy;
use crate::support;
use crate::writable::WritableStreamDefaultController;
use core_runtime::{webidl_interface, webidl_methods};
use js::conversion::FromJSVal;
use js::error::ExnThrown;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::native::Value;
use js::prelude::HandleValue;
use js::Promise;

/// A traced slot holding a `Promise`.
///
/// `must_root`: the slot wraps a `Heap<Promise>`, so crown rejects holding a
/// `PromiseSlot` by value in a plain local across an allocation — the
/// untraced-`Heap`-across-GC hazard. While it lives inside a traced container (a
/// stream's write-request queue or an in-flight/close slot) its `Heap` is traced
/// normally; once moved out via `take`/`pop_front` it must be consumed
/// immediately (`into_promise`) or kept in a `RootedTraceableBox`.
#[js::must_root]
#[derive(Default, js::ScopeRoot)]
pub(crate) struct PromiseSlot(Heap<js::promise::Promise>);

// Safety: the only GC pointer is the inner Heap.
unsafe impl js::heap::Trace for PromiseSlot {
    #[inline]
    unsafe fn trace(&self, trc: *mut js::native::JSTracer) {
        self.0.trace(trc);
    }
}

impl PromiseSlot {
    /// Wrap a promise in a slot.
    pub(crate) fn new(promise: Promise<'_>) -> Self {
        Self(Heap::from(promise))
    }
}

impl<'s> StackPromiseSlot<'s> {
    /// Consume the rooted slot, returning its promise.
    pub(crate) fn into_promise(self) -> Promise<'s> {
        self.0
    }
}

/// A pending abort request.
///
/// <https://streams.spec.whatwg.org/#writablestream-pendingabortrequest>
///
/// `must_root`: holds `Heap` fields, so crown rejects holding it by value across
/// an allocation. It lives in the stream's traced `[[pendingAbortRequest]]`
/// slot; once moved out it must be consumed immediately or kept rooted.
#[js::must_root]
#[derive(core_runtime::Traceable, Default, js::ScopeRoot)]
pub(crate) struct PendingAbortRequest {
    /// The promise returned by the `abort()` call that produced this request.
    pub(crate) promise: Heap<js::promise::Promise>,
    /// The abort reason.
    pub(crate) reason: Heap<Value>,
    /// Whether the stream was already erroring when the abort was requested.
    #[no_trace]
    pub(crate) was_already_erroring: bool,
}

impl<'s> StackPendingAbortRequest<'s> {
    /// Consume the rooted request, returning its `promise` (and dropping the
    /// reason).
    pub(crate) fn into_promise(self) -> Promise<'s> {
        self.promise
    }
}

/// <https://streams.spec.whatwg.org/#ws-class>
#[webidl_interface]
pub struct WritableStream {
    /// <https://streams.spec.whatwg.org/#writablestream-backpressure>
    /// A boolean indicating the backpressure signal set by the controller
    pub(crate) backpressure: bool,
    /// <https://streams.spec.whatwg.org/#writablestream-closerequest>
    /// The promise returned from the writer’s close() method
    pub(crate) close_request: Option<PromiseSlot>,
    /// <https://streams.spec.whatwg.org/#writablestream-controller>
    /// A WritableStreamDefaultController created with the ability to control the state and queue of
    /// this stream
    ///
    /// `Option` because the stream is created before `SetUp...Controller` wires
    /// it; always `Some` thereafter.
    pub(crate) controller: Option<Heap<WritableStreamDefaultControllerImpl>>,
    /// <https://streams.spec.whatwg.org/#writablestream-detached>
    /// A boolean flag set to true when the stream is transferred
    pub(crate) detached: bool,
    /// <https://streams.spec.whatwg.org/#writablestream-inflightwriterequest>
    /// A slot set to the promise for the current in-flight write operation while the underlying
    /// sink’s write algorithm is executing and has not yet fulfilled, used to prevent reentrant
    /// calls
    pub(crate) in_flight_write_request: Option<PromiseSlot>,
    /// <https://streams.spec.whatwg.org/#writablestream-inflightcloserequest>
    /// A slot set to the promise for the current in-flight close operation while the underlying
    /// sink’s close algorithm is executing and has not yet fulfilled, used to prevent the abort()
    /// method from interrupting close
    pub(crate) in_flight_close_request: Option<PromiseSlot>,
    /// <https://streams.spec.whatwg.org/#writablestream-pendingabortrequest>
    /// A pending abort request
    pub(crate) pending_abort_request: Option<PendingAbortRequest>,
    /// <https://streams.spec.whatwg.org/#writablestream-state>
    /// A string containing the stream’s current state, used internally; one of "writable",
    /// "closed", "erroring", or "errored"
    #[no_trace]
    pub(crate) state: WritableStreamState,
    /// <https://streams.spec.whatwg.org/#writablestream-storederror>
    /// A value indicating how the stream failed, to be given as a failure reason or exception when
    /// trying to operate on the stream while in the "errored" state
    pub(crate) stored_error: Heap<Value>,
    /// <https://streams.spec.whatwg.org/#writablestream-writer>
    /// A WritableStreamDefaultWriter instance, if the stream is locked to a writer, or undefined if
    /// it is not
    pub(crate) writer: Option<Heap<WritableStreamDefaultWriterImpl>>,
    /// <https://streams.spec.whatwg.org/#writablestream-writerequests>
    /// A list of promises representing the stream’s internal queue of write requests not yet
    /// processed by the underlying sink
    pub(crate) write_requests: std::collections::VecDeque<PromiseSlot>,
    /// If this writable is the writable end of an identity `TransformStream`,
    /// this holds the transform's readable end. `pipeTo` uses it to propagate
    /// a native byte source through the identity transform.
    /// `None` for a standalone writable or a non-identity transform.
    pub(crate) identity_transform_readable:
        Option<Heap<crate::readable::readable_stream::ReadableStreamImpl>>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum WritableStreamState {
    #[default]
    Writable,
    Closed,
    Erroring,
    Errored,
}

#[webidl_methods]
impl WritableStream {
    /// <https://streams.spec.whatwg.org/#ws-constructor>
    #[constructor]
    fn new(
        &self,
        scope: &Scope<'_>,
        underlying_sink: Option<HandleValue<'_>>,
        strategy: Option<QueuingStrategy>,
    ) -> Result<(), ExnThrown> {
        // Step 1: If _underlyingSink_ is missing, set it to null.
        //         (A missing/undefined argument is `None`; taken as `any` so an explicit `null`
        //         can be rejected — the WebIDL `object` type is not nullable.)
        // Step 2: Let _underlyingSinkDict_ be _underlyingSink_, `converted to an IDL value` of type
        //         ``UnderlyingSink``. We cannot declare the _underlyingSink_ argument as having the
        //         ``UnderlyingSink`` type directly, because doing so would lose the reference to
        //         the original object. We need to retain the object so we can `invoke` the various
        //         methods on it.
        let underlying_sink_value = match underlying_sink {
            None => HandleValue::undefined(),
            Some(v) => {
                if !v.is_object() {
                    return Err(js::error::throw_type_error(
                        scope,
                        c"WritableStream constructor: underlyingSink must be an object",
                    ));
                }
                v
            }
        };
        let underlying_sink_dict = UnderlyingSink::from_jsval(scope, underlying_sink_value, ())
            .map_err(|_| {
                if js::exception::get_pending(scope).is_err() {
                    js::error::throw_type_error(scope, c"Invalid underlying sink");
                }
                ExnThrown
            })?;
        // Step 2 (continued): converting the dictionary validates that each present
        // callback member is callable (a `TypeError`), before the step-3 `type`
        // `RangeError`.
        support::ensure_callback_members_callable(
            scope,
            &[
                (
                    underlying_sink_dict.abort.as_ref(),
                    c"underlying sink abort must be a function",
                ),
                (
                    underlying_sink_dict.close.as_ref(),
                    c"underlying sink close must be a function",
                ),
                (
                    underlying_sink_dict.start.as_ref(),
                    c"underlying sink start must be a function",
                ),
                (
                    underlying_sink_dict.write.as_ref(),
                    c"underlying sink write must be a function",
                ),
            ],
        )?;
        // Step 3: If _underlyingSinkDict_["``type``"] `exists`, throw a ``RangeError`` exception.
        //         This is to allow us to add new potential types in the future, without
        //         backward-compatibility concerns.
        if underlying_sink_dict.r#type.is_some() {
            return Err(js::error::throw_range_error(
                scope,
                c"Invalid type is specified",
            ));
        }
        // Step 4: Perform ! `InitializeWritableStream`(`this`).
        algorithms::initialize_writable_stream(self);
        // Step 5: Let _sizeAlgorithm_ be ! `ExtractSizeAlgorithm`(_strategy_).
        let size_algorithm = scope.root_value(extract_size_algorithm(scope, &strategy)?);
        // Step 6: Let _highWaterMark_ be ? `ExtractHighWaterMark`(_strategy_, 1).
        let high_water_mark = extract_high_water_mark(scope, &strategy, 1.0)?;
        // Step 7: Perform ? `SetUpWritableStreamDefaultControllerFromUnderlyingSink`(`this`,
        //         _underlyingSink_, _underlyingSinkDict_, _highWaterMark_, _sizeAlgorithm_).
        algorithms::set_up_writable_stream_default_controller_from_underlying_sink(
            scope,
            self,
            underlying_sink_value,
            &underlying_sink_dict,
            high_water_mark,
            size_algorithm,
        )
    }

    /// <https://streams.spec.whatwg.org/#ws-locked>
    #[getter]
    fn locked(&self) -> bool {
        // Step 1: Return ! `IsWritableStreamLocked`(`this`).
        algorithms::is_writable_stream_locked(self)
    }

    /// <https://streams.spec.whatwg.org/#writablestream-abort>
    #[method(length = 0)]
    fn abort<'r>(
        &self,
        scope: &'r Scope<'_>,
        reason: HandleValue<'_>,
    ) -> Result<Promise<'r>, ExnThrown> {
        // Step 1: If ! `IsWritableStreamLocked`(`this`) is true, return `a promise rejected with` a
        //         ``TypeError`` exception.
        if algorithms::is_writable_stream_locked(self) {
            js::error::throw_type_error(scope, c"Cannot abort a stream that already has a writer");
            return Promise::new_rejected_with_pending_error(scope);
        }
        // Step 2: Return ! `WritableStreamAbort`(`this`, _reason_).
        Ok(algorithms::writable_stream_abort(scope, self, reason))
    }

    /// <https://streams.spec.whatwg.org/#writablestream-close>
    #[method]
    fn close<'r>(&self, scope: &'r Scope<'_>) -> Result<Promise<'r>, ExnThrown> {
        // Step 1: If ! `IsWritableStreamLocked`(`this`) is true, return `a promise rejected with` a
        //         ``TypeError`` exception.
        if algorithms::is_writable_stream_locked(self) {
            js::error::throw_type_error(scope, c"Cannot close a stream that already has a writer");
            return Promise::new_rejected_with_pending_error(scope);
        }
        // Step 2: If ! `WritableStreamCloseQueuedOrInFlight`(`this`) is true, return `a promise
        //         rejected with` a ``TypeError`` exception.
        if algorithms::writable_stream_close_queued_or_in_flight(self) {
            js::error::throw_type_error(scope, c"Cannot close an already-closing stream");
            return Promise::new_rejected_with_pending_error(scope);
        }
        // Step 3: Return ! `WritableStreamClose`(`this`).
        Ok(algorithms::writable_stream_close(scope, self))
    }

    /// <https://streams.spec.whatwg.org/#ws-get-writer>
    #[method]
    fn get_writer<'r>(
        &self,
        scope: &'r Scope<'_>,
    ) -> Result<WritableStreamDefaultWriter<'r>, ExnThrown> {
        // Step 1: Return ? `AcquireDefaultWriter`(`this`).
        algorithms::acquire_writable_stream_default_writer(scope, self)
    }

    pub(crate) fn controller<'r>(
        &'r self,
        scope: &'r Scope<'_>,
    ) -> WritableStreamDefaultController<'r> {
        self.data()
            .controller
            .as_ref()
            .expect("stream has a controller")
            .get(scope)
    }
}
