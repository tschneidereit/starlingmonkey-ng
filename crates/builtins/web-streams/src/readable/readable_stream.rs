// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/>

use super::algorithms;
use super::async_iterator::{ReadableStreamAsyncIterator, ReadableStreamAsyncIteratorImpl};
use super::enums::ReadableStreamType;
use super::options::ReadableStreamGetReaderOptions;
use super::options::ReadableStreamIteratorOptions;
use super::options::StreamPipeOptions;
use super::underlying_source::UnderlyingSource;
use crate::algorithms::{extract_high_water_mark, extract_size_algorithm};
use crate::queuing::QueuingStrategy;
use crate::readable::ReadableStreamDefaultController;
use crate::transform::readable_writable_pair::ReadableWritablePair;
use crate::writable::WritableStream;
use core_runtime::{webidl_interface, webidl_methods};
use js::conversion::ConversionError;
use js::conversion::FromJSVal;
use js::conversion::ToJSVal;
use js::error::ExnThrown;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::native::Value;
use js::prelude::HandleValue;
use js::value;
use js::Object;
use js::Promise;
use std::borrow::Cow;
use std::fmt;
use web_globals::signals::AbortSignal;

/// Extract `(preventClose, preventAbort, preventCancel, signal)` from a
/// `StreamPipeOptions` dictionary, defaulting to all-false and an undefined
/// signal when the options argument was not supplied.
fn pipe_options<'r>(
    options: &Option<StreamPipeOptions<'r>>,
) -> (bool, bool, bool, Option<AbortSignal<'r>>) {
    match options {
        Some(o) => (o.prevent_close, o.prevent_abort, o.prevent_cancel, o.signal),
        None => (false, false, false, None),
    }
}

pub type ReadableStreamReader<'a> = HandleValue<'a>; // WebIDL: (DefaultReader or BYOBReader)
pub type ReadableStreamController<'a> = HandleValue<'a>; // WebIDL: (ReadableStreamDefaultController or ReadableByteStreamController)

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
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ReadableStreamState {
    #[default]
    Readable,
    Closed,
    Errored,
}

impl fmt::Display for ReadableStreamState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Readable => "readable",
            Self::Closed => "closed",
            Self::Errored => "errored",
        })
    }
}

impl<'s> FromJSVal<'s> for ReadableStreamState {
    type Config = ();

    fn from_jsval(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        _: (),
    ) -> Result<Self, ConversionError> {
        let s = String::from_jsval(scope, val, ())?;
        match s.as_str() {
            "readable" => Ok(Self::Readable),
            "closed" => Ok(Self::Closed),
            "errored" => Ok(Self::Errored),
            _ => Err(ConversionError::Failure(Cow::Borrowed(
                c"invalid value for ReadableStreamState",
            ))),
        }
    }
}

impl<'s> ToJSVal<'s> for ReadableStreamState {
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        match self {
            Self::Readable => "readable".to_jsval(scope),
            Self::Closed => "closed".to_jsval(scope),
            Self::Errored => "errored".to_jsval(scope),
        }
    }
}

#[webidl_methods]
impl ReadableStream {
    /// <https://streams.spec.whatwg.org/#rs-constructor>
    #[constructor]
    fn new(
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
            None => scope.root_value(value::undefined()),
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

    /// <https://streams.spec.whatwg.org/#readablestream-locked>
    #[getter]
    fn locked(&self) -> bool {
        // Step 1: Return ! `IsReadableStreamLocked`(`this`).
        algorithms::is_readable_stream_locked(self)
    }

    /// <https://streams.spec.whatwg.org/#readablestream-cancel>
    #[method]
    fn cancel<'r>(
        &self,
        scope: &'r Scope<'_>,
        reason: Option<HandleValue<'_>>,
    ) -> Result<Promise<'r>, ExnThrown> {
        // Step 1: If ! `IsReadableStreamLocked`(`this`) is true, return `a promise rejected with` a
        //         ``TypeError`` exception.
        if algorithms::is_readable_stream_locked(self) {
            js::error::throw_type_error(scope, c"Cannot cancel a stream that already has a reader");
            return Promise::new_rejected_with_pending_error(scope).map_err(|_| ExnThrown);
        }
        // Step 2: Return ! `ReadableStreamCancel`(`this`, _reason_).
        let reason = reason.unwrap_or_else(|| scope.root_value(value::undefined()));
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
        if algorithms::is_readable_stream_locked(self) {
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
                    return Promise::new_rejected_with_pending_error(scope).map_err(|_| ExnThrown);
                }
            },
            _ => None,
        };
        // Step 1: If ! `IsReadableStreamLocked`(`this`) is true, return `a promise rejected with` a
        //         ``TypeError`` exception.
        if algorithms::is_readable_stream_locked(self) {
            js::error::throw_type_error(scope, c"cannot pipe from a locked ReadableStream");
            return Promise::new_rejected_with_pending_error(scope).map_err(|_| ExnThrown);
        }
        // Step 2: If ! `IsWritableStreamLocked`(_destination_) is true, return `a promise rejected
        //         with` a ``TypeError`` exception.
        if crate::writable::algorithms::is_writable_stream_locked(&destination) {
            js::error::throw_type_error(scope, c"cannot pipe to a locked WritableStream");
            return Promise::new_rejected_with_pending_error(scope).map_err(|_| ExnThrown);
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

    /// <https://streams.spec.whatwg.org/#readablestream-tee>
    #[method]
    fn tee<'r>(&self, scope: &'r Scope<'_>) -> Result<Vec<ReadableStream<'r>>, ExnThrown> {
        // Step 1: Return ? `ReadableStreamTee`(`this`, false).
        algorithms::readable_stream_tee(scope, self, false)
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
        let reader_obj = Object::from_value(scope, reader.as_value()).map_err(|_| ExnThrown)?;
        iterator.data_mut().reader = Some(Heap::from(reader_obj));
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

    pub(crate) fn controller<'r>(
        &'r self,
        scope: &'r Scope<'_>,
    ) -> ReadableStreamDefaultController<'r> {
        self.data()
            .controller
            .as_ref()
            .expect("stream has a controller")
            .get(scope)
            .cast::<ReadableStreamDefaultController>()
            .expect("Must only be called after the stream is set up with a default controller")
    }
}
