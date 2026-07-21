// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/>

use super::algorithms;
use super::options::BYOBReaderReadOptions;
use super::read_request::ReadIntoRequest;
use super::readable_stream::ReadableStream;
use super::readable_stream::ReadableStreamImpl;
use core_runtime::{webidl_interface, webidl_methods};
use js::error::ExnThrown;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::prelude::HandleValue;
use js::Promise;

/// <https://streams.spec.whatwg.org/#byob-reader-class>
#[webidl_interface(name = "ReadableStreamBYOBReader")]
pub struct BYOBReader {
    /// <https://streams.spec.whatwg.org/#BYOBReader-readintorequests>
    /// A list of read-into requests, used when a consumer requests chunks sooner than they are
    /// available.
    pub(crate) read_into_requests: std::collections::VecDeque<ReadIntoRequest>,
    /// `ReadableStreamGenericReader` mixin slot `[[stream]]`: the stream owning
    /// this reader, or `None` once released.
    ///
    /// <https://streams.spec.whatwg.org/#readablestreamgenericreader-stream>
    pub(crate) stream: Option<Heap<ReadableStreamImpl>>,
    /// `ReadableStreamGenericReader` mixin slot `[[closedPromise]]`.
    ///
    /// <https://streams.spec.whatwg.org/#readablestreamgenericreader-closedpromise>
    pub(crate) closed_promise: Heap<js::promise::Promise>,
}

#[webidl_methods]
impl BYOBReader {
    /// <https://streams.spec.whatwg.org/#byob-reader-constructor>
    ///
    /// Setup-style: the JS object is allocated with default data first, then
    /// `SetUpBYOBReader` populates it (it needs `&self` to wire the
    /// stream↔reader links and the closed promise).
    #[constructor]
    fn new(&self, scope: &Scope<'_>, stream: ReadableStream<'_>) -> Result<(), ExnThrown> {
        // Step 1: Perform ? `SetUpBYOBReader`(`this`, _stream_).
        algorithms::set_up_readable_stream_byob_reader(scope, self, &stream)
    }

    /// <https://streams.spec.whatwg.org/#dom-BYOBReader-closed>
    #[getter]
    fn closed<'r>(&self, scope: &'r Scope<'_>) -> Promise<'r> {
        // Step 1: Return `this`.`[[closedPromise]]`.
        self.data().closed_promise.get(scope)
    }

    /// <https://streams.spec.whatwg.org/#byob-reader-read>
    #[method]
    fn read<'r>(
        &self,
        scope: &'r Scope<'_>,
        view: HandleValue<'_>, /* WebIDL: ArrayBufferView */
        options: Option<BYOBReaderReadOptions>,
    ) -> Result<Promise<'r>, ExnThrown> {
        // The WebIDL signature coerces the argument to an `ArrayBufferView`; a
        // non-view value rejects with a ``TypeError`` before the steps below.
        let view = match js::Object::from_value(scope, *view)
            .ok()
            .and_then(js::ArrayBufferView::from_object)
        {
            Some(v) => v,
            None => {
                js::error::throw_type_error(scope, c"read() argument is not an ArrayBufferView");
                return Promise::new_rejected_with_pending_error(scope).map_err(|_| ExnThrown);
            }
        };
        // WebIDL: `min` is `[EnforceRange] unsigned long long`, defaulting to 1 when
        // the options dictionary or member is absent. Apply `[EnforceRange]` here:
        // a non-finite value, or one that is negative after truncating toward zero,
        // rejects with a `TypeError`. (The upper bound 2^64-1 is not enforced — it is
        // beyond `f64` precision and a value that large fails the length checks below
        // with a `RangeError` regardless.) A conversion failure surfaces as a rejected
        // promise rather than a synchronous throw, per WebIDL §3.7.7 ("Operations")
        // for a promise-returning operation.
        let min_f64 = options.map(|o| o.min).unwrap_or(1.0);
        if !min_f64.is_finite() || min_f64.trunc() < 0.0 {
            js::error::throw_type_error(scope, c"read() option min is out of range");
            return Promise::new_rejected_with_pending_error(scope).map_err(|_| ExnThrown);
        }
        let min = min_f64.trunc() as usize;
        // Step 1: If _view_.[[ByteLength]] is 0, return `a promise rejected with` a ``TypeError``
        //         exception.
        if view.byte_length() == 0 {
            js::error::throw_type_error(scope, c"read() view has a byte length of 0");
            return Promise::new_rejected_with_pending_error(scope).map_err(|_| ExnThrown);
        }
        // Step 2: If _view_.[[ViewedArrayBuffer]].[[ByteLength]] is 0, return `a promise rejected
        //         with` a ``TypeError`` exception.
        let buffer = view.viewed_buffer(scope)?;
        if buffer.byte_length() == 0 {
            js::error::throw_type_error(scope, c"read() view's buffer has a byte length of 0");
            return Promise::new_rejected_with_pending_error(scope).map_err(|_| ExnThrown);
        }
        // Step 3: If ! `IsDetachedBuffer`(_view_.[[ViewedArrayBuffer]]) is true, return `a promise
        //         rejected with` a ``TypeError`` exception.
        if buffer.is_detached() {
            js::error::throw_type_error(scope, c"read() view's buffer is detached");
            return Promise::new_rejected_with_pending_error(scope).map_err(|_| ExnThrown);
        }
        // Step 4: If _options_["``min``"] is 0, return `a promise rejected with` a ``TypeError``
        //         exception.
        if min == 0 {
            js::error::throw_type_error(scope, c"read() option min cannot be 0");
            return Promise::new_rejected_with_pending_error(scope).map_err(|_| ExnThrown);
        }
        if view.view_kind().is_typed_array() {
            // Step 5: If _view_ has a [[TypedArrayName]] internal slot, If _options_["``min``"] >
            //         _view_.[[ArrayLength]], return `a promise rejected with` a ``RangeError``
            //         exception.
            if min > view.array_length() {
                js::error::throw_range_error(scope, c"read() option min exceeds the view's length");
                return Promise::new_rejected_with_pending_error(scope).map_err(|_| ExnThrown);
            }
        } else {
            // Step 6: Otherwise (i.e., it is a ``DataView``), If _options_["``min``"] >
            //         _view_.[[ByteLength]], return `a promise rejected with` a ``RangeError``
            //         exception.
            if min > view.byte_length() {
                js::error::throw_range_error(
                    scope,
                    c"read() option min exceeds the view's byte length",
                );
                return Promise::new_rejected_with_pending_error(scope).map_err(|_| ExnThrown);
            }
        }
        // Step 7: If `this`.`[[stream]]` is undefined, return `a promise rejected with` a
        //         ``TypeError`` exception.
        if self.data().stream.is_none() {
            js::error::throw_type_error(
                scope,
                c"Cannot read from a reader that is not attached to a stream",
            );
            return Promise::new_rejected_with_pending_error(scope).map_err(|_| ExnThrown);
        }
        // Step 8: Let _promise_ be `a new promise`.
        let promise = Promise::new_pending(scope)?;
        // Step 9: Let _readIntoRequest_ be a new `read-into request` with the following `items`:
        //         `chunk steps`, given _chunk_ `Resolve` _promise_ with «[ "``value``" →
        //         _chunk_, "``done``" → false ]». `close steps`, given _chunk_ `Resolve`
        //         _promise_ with «[ "``value``" → _chunk_, "``done``" → true ]». `error
        //         steps`, given _e_ `Reject` _promise_ with _e_.
        //         (The steps settle `promise`; see `ReadIntoRequest::Read`.)
        let read_into_request = ReadIntoRequest::Read {
            promise: Heap::from(promise),
        };
        // Step 10: Perform ! `BYOBReaderRead`(`this`, _view_, _options_["``min``"],
        //          _readIntoRequest_).
        algorithms::readable_stream_byob_reader_read(scope, self, view, min, read_into_request);
        // Step 11: Return _promise_.
        Ok(promise)
    }

    /// <https://streams.spec.whatwg.org/#byob-reader-release-lock>
    #[method]
    fn release_lock(&self, scope: &Scope<'_>) -> Result<(), ExnThrown> {
        // Step 1: If `this`.`[[stream]]` is undefined, return.
        if self.data().stream.is_none() {
            return Ok(());
        }
        // Step 2: Perform ! `BYOBReaderRelease`(`this`).
        algorithms::readable_stream_byob_reader_release(scope, self)
    }

    /// <https://streams.spec.whatwg.org/#dom-BYOBReader-cancel>
    #[method]
    fn cancel<'r>(
        &self,
        scope: &'r Scope<'_>,
        reason: Option<HandleValue<'r>>,
    ) -> Result<Promise<'r>, ExnThrown> {
        // Step 1: If `this`.`[[stream]]` is undefined, return `a promise rejected with` a
        //         ``TypeError`` exception.
        if self.data().stream.is_none() {
            js::error::throw_type_error(scope, c"Cannot cancel a reader that has no stream");
            return Promise::new_rejected_with_pending_error(scope).map_err(|_| ExnThrown);
        }
        // Step 2: Return ! `ReadableStreamReaderGenericCancel`(`this`, _reason_).
        let reason = reason.unwrap_or_else(|| scope.root_value(js::value::undefined()));
        Ok(algorithms::readable_stream_reader_generic_cancel(
            scope, self, reason,
        ))
    }
}
