// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/>

use super::async_iterator::{ReadableStreamAsyncIterator, ReadableStreamAsyncIteratorImpl};
use super::enums::ReadableStreamType;
use super::options::{
    ReadableStreamGetReaderOptions, ReadableStreamIteratorOptions, StreamPipeOptions,
};
use super::underlying_source::UnderlyingSource;
use super::{algorithms, ReadableByteStreamController};
use crate::algorithms::{extract_high_water_mark, extract_size_algorithm};
use crate::queuing::QueuingStrategy;
use crate::readable::algorithms::{
    acquire_readable_stream_default_reader, create_readable_stream,
    readable_byte_stream_controller_error, readable_byte_stream_tee,
    readable_stream_default_controller_close, readable_stream_default_controller_enqueue,
    readable_stream_default_controller_error, readable_stream_default_tee,
};
use crate::readable::ReadableStreamDefaultController;
use crate::transform::readable_writable_pair::ReadableWritablePair;
use crate::writable::WritableStream;
use core_runtime::{webidl_interface, webidl_methods};
use js::{
    conversion::ConversionError, conversion::FromJSVal, error::ExnThrown, gc::handle::Heap,
    gc::handle::OptionHeapExt, gc::scope::Scope, native::Value, prelude::HandleValue, Object,
    Promise, Uint8Array,
};
use web_globals::signals::AbortSignal;

/// Extract `(preventClose, preventAbort, preventCancel, signal)` from a
/// `StreamPipeOptions` dictionary, defaulting to all-false and an undefined
/// signal when the `options` argument was not supplied.
fn pipe_options<'r>(
    options: &Option<StreamPipeOptions<'r>>,
) -> (bool, bool, bool, Option<AbortSignal<'r>>) {
    match options {
        Some(o) => (o.prevent_close, o.prevent_abort, o.prevent_cancel, o.signal),
        None => (false, false, false, None),
    }
}

pub type ReadableStreamReader<'a> = HandleValue<'a>; // WebIDL: (DefaultReader or BYOBReader)

/// <https://streams.spec.whatwg.org/#rs-class>
#[webidl_interface]
pub struct ReadableStream {
    /// <https://streams.spec.whatwg.org/#readablestream-controller>
    /// A ReadableStreamDefaultController or ReadableByteStreamController created with the ability to
    /// control the state and queue of this stream
    ///
    /// Polymorphic (default-or-byte controller), so stored as an `Object` and
    /// downcast with `Object::cast` at the use site. `None` until set up.
    pub(crate) controller: Option<Heap<js::object::Object>>,
    /// <https://streams.spec.whatwg.org/#readablestream-detached>
    /// A boolean flag set to true when the stream is transferred
    pub(crate) detached: bool,
    /// <https://streams.spec.whatwg.org/#readablestream-disturbed>
    /// A boolean flag set to true when the stream has been read from or canceled
    pub(crate) disturbed: bool,
    /// <https://streams.spec.whatwg.org/#readablestream-reader>
    /// A DefaultReader or BYOBReader instance, if the stream is locked
    /// to a reader, or undefined if it is not
    ///
    /// Polymorphic (default-or-BYOB reader), so stored as an `Object` and
    /// downcast with `Object::cast` at the use site. `None` when unlocked.
    pub(crate) reader: Option<Heap<js::object::Object>>,
    /// <https://streams.spec.whatwg.org/#readablestream-state>
    /// A string containing the stream’s current state, used internally; one of "readable",
    /// "closed", or "errored"
    #[no_trace]
    pub(crate) state: ReadableStreamState,
    /// <https://streams.spec.whatwg.org/#readablestream-storederror>
    /// A value indicating how the stream failed, to be given as a failure reason or exception when
    /// trying to operate on an errored stream
    pub(crate) stored_error: Heap<Value>,
    /// A native byte source backing this stream, set for streams created via
    /// [`ReadableStream::new_native`] and propagated through an identity
    /// `TransformStream` by `pipeTo`. A private slot with no content-visible
    /// effect: it lets a consumer recognize a stream backed by its own native
    /// source. `None` otherwise.
    pub(crate) native_byte_source: Option<Heap<js::object::Object>>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ReadableStreamState {
    #[default]
    Readable,
    Closed,
    Errored,
}

#[webidl_methods]
impl ReadableStream {
    /// <https://streams.spec.whatwg.org/#rs-constructor>
    #[constructor]
    pub fn new(
        &self,
        scope: &Scope<'_>,
        underlying_source: Option<HandleValue<'_>>,
        strategy: Option<QueuingStrategy>,
    ) -> Result<(), ExnThrown> {
        // Step 1: If _underlyingSource_ is missing, set it to null.
        //         (A missing/undefined argument is `None`. The argument is taken as `any` rather
        //         than `object` so an explicit `null` can be rejected: the WebIDL `object` type is
        //         not nullable.)
        // Step 2: Let _underlyingSourceDict_ be _underlyingSource_, `converted to an IDL value` of
        //         type ``UnderlyingSource``. We cannot declare the _underlyingSource_ argument as
        //         having the ``UnderlyingSource`` type directly, because doing so would lose the
        //         reference to the original object. We need to retain the object so we can `invoke`
        //         the various methods on it.
        let underlying_source_value = match underlying_source {
            None => HandleValue::undefined(),
            Some(v) => {
                // The WebIDL `object underlyingSource` argument is non-nullable, so `null` (and any
                // non-object such as a number or string) throws a `TypeError`.
                if !v.is_object() {
                    return Err(js::error::throw_type_error(
                        scope,
                        c"ReadableStream constructor: underlyingSource must be an object",
                    ));
                }
                v
            }
        };
        let underlying_source_dict =
            UnderlyingSource::from_jsval(scope, underlying_source_value, ()).map_err(|_| {
                if js::exception::get_pending(scope).is_err() {
                    js::error::throw_type_error(scope, c"Invalid underlying source");
                }
                ExnThrown
            })?;
        // Step 2 (continued): converting the dictionary validates that each present
        // callback member is callable, before the step-4 byte-stream checks.
        // TODO: consider introducing a `Callable` type that WebIDL type checking can use.
        crate::support::ensure_callback_members_callable(
            scope,
            &[
                (
                    underlying_source_dict.cancel.as_ref(),
                    c"underlying source cancel must be a function",
                ),
                (
                    underlying_source_dict.pull.as_ref(),
                    c"underlying source pull must be a function",
                ),
                (
                    underlying_source_dict.start.as_ref(),
                    c"underlying source start must be a function",
                ),
            ],
        )?;
        // Step 3: Perform ! `InitializeReadableStream`(`this`).
        algorithms::initialize_readable_stream(self);
        // Step 4: If _underlyingSourceDict_["``type``"] is "``bytes``": ...
        if matches!(
            underlying_source_dict.r#type,
            Some(ReadableStreamType::Bytes)
        ) {
            // If _strategy_["``size``"] `exists`, throw a ``RangeError`` exception. Let
            // _highWaterMark_ be ? `ExtractHighWaterMark`(_strategy_, 0). Perform ?
            // `SetUpByteStreamControllerFromUnderlyingSource`(`this`, _underlyingSource_,
            // _underlyingSourceDict_, _highWaterMark_).
            if let Some(size) = strategy.as_ref().and_then(|s| s.size.as_ref()) {
                // `size` is a `QueuingStrategySize` callback: WebIDL converts it when binding the
                // `strategy` argument and throws a `TypeError` if it is not callable, before this
                // RangeError. The codebase defers callback callability checks to their use sites,
                // so replicate the conversion-time `TypeError` here. (TASKLOG: a validating
                // callback-conversion type would centralize this for all callback members.)
                if !size.is_callable() {
                    return Err(js::error::throw_type_error(
                        scope,
                        c"queuing strategy size must be a function",
                    ));
                }
                return Err(js::error::throw_range_error(
                    scope,
                    c"a byte stream's queuing strategy must not have a size function",
                ));
            }
            let high_water_mark = extract_high_water_mark(scope, &strategy, 0.0)?;
            return algorithms::set_up_readable_byte_stream_controller_from_underlying_source(
                scope,
                self,
                underlying_source_value,
                &underlying_source_dict,
                high_water_mark,
            );
        }
        // Step 5: Otherwise, Assert: _underlyingSourceDict_["``type``"] does not `exist`. Let
        //         _sizeAlgorithm_ be ! `ExtractSizeAlgorithm`(_strategy_). Let _highWaterMark_ be ?
        //         `ExtractHighWaterMark`(_strategy_, 1). Perform ?
        //         `SetUpDefaultControllerFromUnderlyingSource`(`this`,
        //         _underlyingSource_, _underlyingSourceDict_, _highWaterMark_, _sizeAlgorithm_).
        let size_algorithm = scope.root_value(extract_size_algorithm(scope, &strategy)?);
        let high_water_mark = extract_high_water_mark(scope, &strategy, 1.0)?;
        algorithms::set_up_readable_stream_default_controller_from_underlying_source(
            scope,
            self,
            underlying_source_value,
            &underlying_source_dict,
            high_water_mark,
            size_algorithm,
        )
    }

    /// Create a `ReadableStream` driven by the provided `pull`/`cancel` algorithms (JS
    /// functions, typically native callbacks).
    ///
    /// `pull` is called with the stream's controller as its argument when more data is wanted.
    /// `cancel` is called with the cancel reason.
    ///
    /// The stream is created with a high-water mark of `0`, so it'll pull data lazily
    /// instead of eagerly requesting the first chunk. Useful for leaving native data sources,
    /// such as external streams, untouched if no JS consumer is reading from the stream.
    ///
    /// `source` can be retrieved with [`ReadableStream::native_source`], letting a consumer
    /// recognize a stream backed by a particular native source. Pass `undefined` if there is
    /// no such object.
    pub fn new_native<'r>(
        scope: &'r Scope<'_>,
        underlying_source: HandleValue<'_>,
        pull: HandleValue<'_>,
        cancel: HandleValue<'_>,
    ) -> Result<ReadableStream<'r>, ExnThrown> {
        let undef = HandleValue::undefined();
        let stream = create_readable_stream(scope, undef, pull, cancel, 0.0, undef)?;
        if let Ok(source) = js::Object::from_value(scope, underlying_source.get()) {
            stream.set_native_source(&source);
        }
        Ok(stream)
    }

    /// Create a `ReadableStream` that yields `bytes` as a single `Uint8Array` chunk
    /// and is then closed. The stream uses native (no-op) start/pull/cancel
    /// algorithms and the default queuing strategy, so it behaves as an ordinary
    /// `ReadableStream` whose reader produces `Uint8Array` chunks.
    ///
    /// An empty slice produces an immediately-closed stream with no chunks.
    pub fn from_bytes<'r>(
        scope: &'r Scope<'_>,
        bytes: &[u8],
    ) -> Result<ReadableStream<'r>, ExnThrown> {
        let undef = HandleValue::undefined();
        let stream = create_readable_stream(scope, undef, undef, undef, 1.0, undef)?;
        let controller = stream
            .default_controller(scope)
            .expect("stream has a default controller");
        if !bytes.is_empty() {
            let chunk = Uint8Array::with_data(scope, bytes)?;
            let chunk_val = scope.root_value(chunk.as_value());
            readable_stream_default_controller_enqueue(scope, &controller, chunk_val)?;
        }
        readable_stream_default_controller_close(scope, &controller);
        Ok(stream)
    }

    /// Create a locked and disturbed `ReadableStream`.
    ///
    /// Useful to implement other builtins' behavior of internally consuming a stream
    /// which is visible to content, such as the `body` field on `Request`/`Response`.
    pub fn new_consumed<'r>(scope: &'r Scope<'_>) -> Result<ReadableStream<'r>, ExnThrown> {
        let stream = ReadableStream::from_bytes(scope, &[])?;
        stream.lock_and_disturb(scope)?;
        Ok(stream)
    }

    /// Lock `stream` to an internal reader (never released) and mark it disturbed.
    ///
    /// Useful to implement other builtins' behavior of internally consuming a stream
    /// which is visible to content, such as the `body` field on `Request`/`Response`.
    pub fn lock_and_disturb(&self, scope: &Scope<'_>) -> Result<(), ExnThrown> {
        if self.is_locked() {
            return Ok(());
        }
        let _reader = acquire_readable_stream_default_reader(scope, self)?;
        self.data_mut().disturbed = true;
        Ok(())
    }

    /// <https://streams.spec.whatwg.org/#readablestream-locked>
    #[getter(name = "locked")]
    pub fn is_locked(&self) -> bool {
        // Step 1: Return ! `IsReadableStreamLocked`(`this`).
        // (Inlined)
        // Step 1: If _stream_.`[[reader]]` is undefined, return false.
        // Step 2: Return true.
        self.data().reader.is_some()
    }

    /// <https://streams.spec.whatwg.org/#readablestream-cancel>
    #[method]
    fn cancel<'r>(
        &self,
        scope: &'r Scope<'_>,
        reason: Option<HandleValue<'r>>,
    ) -> Result<Promise<'r>, ExnThrown> {
        // Step 1: If ! `IsReadableStreamLocked`(`this`) is true, return `a promise rejected with` a
        //         ``TypeError`` exception.
        if self.is_locked() {
            js::error::throw_type_error(scope, c"Cannot cancel a stream that already has a reader");
            return Promise::new_rejected_with_pending_error(scope);
        }
        // Step 2: Return ! `ReadableStreamCancel`(`this`, _reason_).
        let reason = reason.unwrap_or_else(|| HandleValue::undefined());
        Ok(algorithms::readable_stream_cancel(scope, self, reason))
    }

    /// <https://streams.spec.whatwg.org/#rs-get-reader>
    #[method]
    fn get_reader<'r>(
        &self,
        scope: &'r Scope<'_>,
        options: Option<ReadableStreamGetReaderOptions>,
    ) -> Result<ReadableStreamReader<'r>, ExnThrown> {
        // Step 1: If _options_["``mode``"] does not `exist`, return ?
        //         `AcquireDefaultReader`(`this`).
        let mode = options.and_then(|o| o.mode);
        if mode.is_none() {
            let reader = algorithms::acquire_readable_stream_default_reader(scope, self)?;
            return Ok(scope.root_value(reader.as_value()));
        }
        // Step 2: Assert: _options_["``mode``"] is "``byob``".
        // Step 3: Return ? `AcquireBYOBReader`(`this`).
        let reader = algorithms::acquire_readable_stream_byob_reader(scope, self)?;
        Ok(scope.root_value(reader.as_value()))
    }

    /// <https://streams.spec.whatwg.org/#rs-pipe-through>
    #[method]
    fn pipe_through<'r>(
        &self,
        scope: &'r Scope<'_>,
        transform: ReadableWritablePair<'r>,
        options: Option<StreamPipeOptions>,
    ) -> Result<ReadableStream<'r>, ExnThrown> {
        // Step 1: If ! `IsReadableStreamLocked`(`this`) is true, throw a ``TypeError`` exception.
        if self.is_locked() {
            return Err(js::error::throw_type_error(
                scope,
                c"cannot pipe from a locked ReadableStream",
            ));
        }
        // Step 2: If ! `IsWritableStreamLocked`(_transform_["``writable``"]) is true, throw a
        //         ``TypeError`` exception.
        if crate::writable::algorithms::is_writable_stream_locked(&transform.writable) {
            return Err(js::error::throw_type_error(
                scope,
                c"cannot pipe to a locked WritableStream",
            ));
        }
        // Step 3: Let _signal_ be _options_["``signal``"] if it `exists`, or undefined otherwise.
        let (prevent_close, prevent_abort, prevent_cancel, signal) = pipe_options(&options);
        // Step 4: Let _promise_ be ! `ReadableStreamPipeTo`(`this`, _transform_["``writable``"],
        //         _options_["``preventClose``"], _options_["``preventAbort``"],
        //         _options_["``preventCancel``"], _signal_).
        let promise = algorithms::readable_stream_pipe_to(
            scope,
            self,
            &transform.writable,
            prevent_close,
            prevent_abort,
            prevent_cancel,
            signal,
        )?;
        // Step 5: Set _promise_.[[PromiseIsHandled]] to true.
        let _ = promise.set_any_is_handled(scope);
        // Step 6: Return _transform_["``readable``"].
        Ok(transform.readable)
    }

    /// <https://streams.spec.whatwg.org/#rs-pipe-to>
    #[method]
    fn pipe_to<'r>(
        &self,
        scope: &'r Scope<'_>,
        destination: WritableStream<'_>,
        options: Option<HandleValue<'_>>,
    ) -> Result<Promise<'r>, ExnThrown> {
        // `pipeTo` returns a promise, so per WebIDL a failed coercion of the
        // `options` dictionary (e.g. an invalid `signal`) must reject the
        // returned promise rather than throw synchronously. The dictionary is
        // therefore coerced here, inside the method, instead of as a typed
        // parameter.
        let options = match options {
            Some(v) if !v.is_undefined() => match StreamPipeOptions::from_jsval(scope, v, ()) {
                Ok(o) => Some(o),
                Err(e) => {
                    // A non-object value (e.g. an invalid `signal`) fails without
                    // a pending exception; surface it as a TypeError so the
                    // returned promise rejects with one.
                    if let ConversionError::Failure(msg) = e {
                        js::error::throw_type_error(scope, msg.as_ref());
                    }
                    return Promise::new_rejected_with_pending_error(scope);
                }
            },
            _ => None,
        };
        // Step 1: If ! `IsReadableStreamLocked`(`this`) is true, return `a promise rejected with` a
        //         ``TypeError`` exception.
        if self.is_locked() {
            js::error::throw_type_error(scope, c"cannot pipe from a locked ReadableStream");
            return Promise::new_rejected_with_pending_error(scope);
        }
        // Step 2: If ! `IsWritableStreamLocked`(_destination_) is true, return `a promise rejected
        //         with` a ``TypeError`` exception.
        if crate::writable::algorithms::is_writable_stream_locked(&destination) {
            js::error::throw_type_error(scope, c"cannot pipe to a locked WritableStream");
            return Promise::new_rejected_with_pending_error(scope);
        }
        // Step 3: Let _signal_ be _options_["``signal``"] if it `exists`, or undefined otherwise.
        let (prevent_close, prevent_abort, prevent_cancel, signal) = pipe_options(&options);
        // Step 4: Return ! `ReadableStreamPipeTo`(`this`, _destination_,
        //         _options_["``preventClose``"], _options_["``preventAbort``"],
        //         _options_["``preventCancel``"], _signal_).
        algorithms::readable_stream_pipe_to(
            scope,
            self,
            &destination,
            prevent_close,
            prevent_abort,
            prevent_cancel,
            signal,
        )
    }

    /// The JS-exposed `tee` method.
    ///
    /// <https://streams.spec.whatwg.org/#rs-tee>
    #[method(name = "tee")]
    fn js_tee<'r>(&self, scope: &'r Scope<'_>) -> Result<Vec<ReadableStream<'r>>, ExnThrown> {
        // Step 1: Return ? `ReadableStreamTee`(`this`, false).
        let branches = self.tee(scope, false)?;
        Ok(vec![branches.0, branches.1])
    }

    /// Tee the stream into two branches.
    ///
    /// If `clone_for_branch2` is `true`, chunks will be cloned before being produced by the
    /// second branch.
    ///
    /// <https://streams.spec.whatwg.org/#readablestream-tee>
    pub fn tee<'r>(
        &self,
        scope: &'r Scope<'_>,
        clone_for_branch2: bool,
    ) -> Result<(ReadableStream<'r>, ReadableStream<'r>), ExnThrown> {
        // Step 1: Assert: _stream_ `implements` ``ReadableStream``.
        // Step 2: Assert: _cloneForBranch2_ is a boolean.
        // Step 3: If _stream_.`[[controller]]` `implements` ``ReadableByteStreamController``, return ?
        //         `ReadableByteStreamTee`(_stream_).
        if self.byte_controller(scope).is_some() {
            return readable_byte_stream_tee(scope, self);
        }
        // Step 4: Return ? `ReadableStreamDefaultTee`(_stream_, _cloneForBranch2_).
        readable_stream_default_tee(scope, self, clone_for_branch2)
    }

    /// <https://streams.spec.whatwg.org/#rs-asynciterator>
    /// The `values` method, also installed as `[Symbol.asyncIterator]`.
    #[method]
    fn values<'r>(
        &self,
        scope: &'r Scope<'_>,
        options: Option<ReadableStreamIteratorOptions>,
    ) -> Result<ReadableStreamAsyncIterator<'r>, ExnThrown> {
        // Step 1: Let _reader_ be ? `AcquireDefaultReader`(`this`).
        let reader = algorithms::acquire_readable_stream_default_reader(scope, self)?;
        // Step 2: Let _iterator_ be a `new` ``ReadableStreamAsyncIterator``.
        let iterator = unsafe {
            js::class::create_instance_with::<ReadableStreamAsyncIteratorImpl>(scope, |_| {
                ReadableStreamAsyncIteratorImpl::default()
            })
        }?
        .cast::<ReadableStreamAsyncIterator>()
        .map_err(|_| ExnThrown)?;
        // Step 3: Set _iterator_'s reader to _reader_.
        iterator.data_mut().reader = Some(Heap::from(reader));
        // Step 4: Let _preventCancel_ be _options_["``preventCancel``"].
        // Step 5: Set _iterator_'s prevent cancel to _preventCancel_.
        iterator.data_mut().prevent_cancel = options.map(|o| o.prevent_cancel).unwrap_or(false);
        // Step 6: Return _iterator_.
        Ok(iterator)
    }

    /// <https://streams.spec.whatwg.org/#rs-from>
    #[static_method(name = "from")]
    fn js_from<'r>(
        scope: &'r Scope<'_>,
        async_iterable: HandleValue<'_>,
    ) -> Result<ReadableStream<'r>, ExnThrown> {
        // Step 1: Return ? `ReadableStreamFromIterable`(_asyncIterable_).
        algorithms::readable_stream_from_iterable(scope, async_iterable)
    }

    /// Whether the stream has been read from or canceled.
    pub fn is_disturbed(&self) -> bool {
        self.data().disturbed
    }

    pub fn error<'r>(&self, scope: &'r Scope<'_>, reason: HandleValue<'_>) {
        if let Some(controller) = self.default_controller(scope) {
            readable_stream_default_controller_error(scope, &controller, reason);
        } else {
            let controller = self.byte_controller(scope).unwrap();
            readable_byte_stream_controller_error(scope, &controller, reason);
        }
    }

    pub(crate) fn default_controller<'r>(
        &'r self,
        scope: &'r Scope<'_>,
    ) -> Option<ReadableStreamDefaultController<'r>> {
        self.data()
            .controller
            .get(scope)
            .expect("stream has a controller")
            .cast::<ReadableStreamDefaultController>()
            .ok()
    }

    pub(crate) fn byte_controller<'r>(
        &self,
        scope: &'r Scope<'_>,
    ) -> Option<ReadableByteStreamController<'r>> {
        self.data()
            .controller
            .get(scope)
            .expect("stream has a controller")
            .cast::<ReadableByteStreamController>()
            .ok()
    }

    /// The native source object backing this stream, if any (see
    /// [`native_byte_source`](Self::native_byte_source)). Lets a consumer
    /// recognize a stream backed by a native source instead of content code.
    /// This works for stream pipelines as well as Readable/Writable streams
    /// connected through an identity `TransformStream`
    /// (see [`set_native_source`](Self::set_native_source)).
    pub fn native_source<'r>(&self, scope: &'r Scope<'_>) -> Option<Object<'r>> {
        self.data().native_byte_source.get(scope)
    }

    /// Record `source` as this stream's native byte source.
    pub fn set_native_source(&self, source: &Object<'_>) {
        self.data_mut().native_byte_source = Some(Heap::from(*source));
    }
}

impl<'s> ReadableStream<'s> {
    pub fn reader(&self, scope: &'s Scope<'_>) -> Option<Object<'s>> {
        self.data().reader.get(scope)
    }
}
