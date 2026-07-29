// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! The default asynchronous iterator for `ReadableStream`.
//!
//! <https://streams.spec.whatwg.org/#rs-asynciterator>
//!
//! `ReadableStream.prototype.values()` and `[Symbol.asyncIterator]()` return an
//! instance of this interface. Its prototype chains to `%AsyncIteratorPrototype%`
//! (via `js_proto = "AsyncIterator"`), which supplies `[Symbol.asyncIterator]`
//! returning `this`. The `next`/`return` methods implement WebIDL §3.7.10.2's
//! default async iterator semantics: calls are serialized through an "ongoing
//! promise", and a finished iterator yields `{ value: undefined, done: true }`.

use super::algorithms;
use super::default_reader::{DefaultReader, DefaultReaderImpl};
use super::read_request::ReadRequest;
use crate::algorithms::{pair_parts, pair_payload};
use core_runtime::{webidl_interface, webidl_methods};
use js::error::ExnThrown;
use js::gc::handle::{Heap, OptionHeapExt};
use js::gc::scope::Scope;
use js::iteration::create_iter_result;
use js::native::Value;
use js::prelude::{CallbackArgs, HandleValue};
use js::{Function, Object, Promise};

/// <https://streams.spec.whatwg.org/#rs-asynciterator>
///
/// The prototype's `[[Prototype]]` is set to `%AsyncIteratorPrototype%` at
/// registration (see `add_to_global`), which supplies `[Symbol.asyncIterator]`.
// WebIDL §3.7.10: the class string of an asynchronous iterator prototype object
// is the interface identifier followed by " AsyncIterator" (with a space).
#[webidl_interface(hidden, to_string_tag = "ReadableStream AsyncIterator")]
pub struct ReadableStreamAsyncIterator {
    /// The `DefaultReader` acquired for this iteration.
    pub(crate) reader: Option<Heap<DefaultReaderImpl>>,
    /// The `preventCancel` option captured at creation.
    #[no_trace]
    pub(crate) prevent_cancel: bool,
    /// WebIDL default async iterator `[[isFinished]]`.
    #[no_trace]
    pub(crate) is_finished: bool,
    /// WebIDL default async iterator `[[ongoingPromise]]`, serializing calls.
    pub(crate) ongoing_promise: Option<Heap<js::promise::Promise>>,
    /// The unique "end of iteration" sentinel (see `get_next_iteration_result`),
    /// and the per-iterator reaction callbacks for `next()` (payload = this
    /// iterator). All are created on the first `next()` and reused for every
    /// subsequent call, so iterating allocates no callback objects per chunk.
    /// `None` until the first `next()`.
    pub(crate) end_of_iteration: Option<Heap<js::object::Object>>,
    pub(crate) next_fulfilled_fn: Option<Heap<js::function::Function>>,
    pub(crate) next_rejected_fn: Option<Heap<js::function::Function>>,
    pub(crate) after_ongoing_next_fn: Option<Heap<js::function::Function>>,
}

#[webidl_methods]
impl ReadableStreamAsyncIterator {
    /// Not exposed to JS (see `hidden`). Produces the default-initialized data;
    /// the fields are populated by `ReadableStream.prototype.values`.
    #[constructor]
    fn new() -> Self {
        ReadableStreamAsyncIteratorImpl::default()
    }

    /// <https://webidl.spec.whatwg.org/#dfn-asynchronous-iterator-prototype-object>: `next`.
    #[method]
    fn next<'r>(&self, scope: &'r Scope<'_>) -> Result<Promise<'r>, ExnThrown> {
        // Serialize on the ongoing promise: run the next steps now if none is
        // pending, otherwise after the pending one settles (either way).
        let prev = self.data().ongoing_promise.get(scope);
        let ongoing = match prev {
            Some(prev) => {
                if self.data().after_ongoing_next_fn.is_none() {
                    let cb = Function::new_callback(scope, c"", 1, after_ongoing_next, self)?;
                    self.data_mut().after_ongoing_next_fn = Some(Heap::from(cb));
                }
                let cb = self
                    .data()
                    .after_ongoing_next_fn
                    .get(scope)
                    .expect("created above");
                prev.then(scope, Some(*cb), Some(*cb))?
            }
            None => run_next_steps(scope, self)?,
        };
        self.data_mut().ongoing_promise = Some(Heap::from(ongoing));
        Ok(ongoing)
    }

    /// <https://webidl.spec.whatwg.org/#dfn-asynchronous-iterator-prototype-object>: `return`.
    #[method(name = "return", length = 1)]
    fn iterator_return<'r>(
        &self,
        scope: &'r Scope<'_>,
        value: Option<HandleValue<'r>>,
    ) -> Result<Promise<'r>, ExnThrown> {
        let value = value.unwrap_or(HandleValue::undefined());
        let prev = self.data().ongoing_promise.get(scope);
        let ongoing = match prev {
            Some(prev) => {
                let payload = pair_payload(scope, scope.root_value(self.as_value()), value)?;
                let cb = Function::new_callback(scope, c"", 1, after_ongoing_return, payload)?;
                prev.then(scope, Some(*cb), Some(*cb))?
            }
            None => run_return_steps(scope, self, value)?,
        };
        self.data_mut().ongoing_promise = Some(Heap::from(ongoing));
        Ok(ongoing)
    }
}

fn iter_from_value<'r>(
    scope: &'r Scope<'_>,
    v: HandleValue<'_>,
) -> Result<ReadableStreamAsyncIterator<'r>, ExnThrown> {
    Object::from_value(scope, *v)
        .map_err(|_| ExnThrown)?
        .cast::<ReadableStreamAsyncIterator>()
        .map_err(|_| ExnThrown)
}

/// The async iterator's `[[ongoingPromise]]` next continuation (payload = the
/// iterator): run the next steps regardless of how the previous call settled.
fn after_ongoing_next(
    scope: &Scope<'_>,
    _args: CallbackArgs<'_>,
    payload: HandleValue<'_>,
) -> Result<Value, ExnThrown> {
    let iter = iter_from_value(scope, payload)?;
    Ok(run_next_steps(scope, &iter)?.as_value())
}

/// The async iterator's `[[ongoingPromise]]` return continuation (payload =
/// `[iterator, value]`).
fn after_ongoing_return(
    scope: &Scope<'_>,
    _args: CallbackArgs<'_>,
    payload: HandleValue<'_>,
) -> Result<Value, ExnThrown> {
    let (iter_v, value) = pair_parts(scope, payload);
    let iter = iter_from_value(scope, iter_v)?;
    Ok(run_return_steps(scope, &iter, value)?.as_value())
}

/// WebIDL next steps: a finished iterator yields end-of-iteration; otherwise read
/// the next chunk and map the result.
fn run_next_steps<'r>(
    scope: &'r Scope<'_>,
    iter: &ReadableStreamAsyncIterator<'_>,
) -> Result<Promise<'r>, ExnThrown> {
    if iter.data().is_finished {
        let result = create_iter_result(scope, HandleValue::undefined(), true)?;
        return Promise::new_resolved_with_value(scope, result);
    }
    // The unique sentinel the close steps resolve the next-iteration promise
    // with (recognized by identity in `next_fulfilled`) and the reaction
    // callbacks are per-iterator; create them on the first `next()` and reuse
    // them for every subsequent chunk.
    if iter.data().next_fulfilled_fn.is_none() {
        // TODO: share the sentinel per-global.
        let sentinel = Object::new_plain(scope)?;
        iter.data_mut().end_of_iteration = Some(Heap::from(sentinel));
        let payload = scope.root_value(iter.as_value());
        let on_f = Function::new_callback(scope, c"", 1, next_fulfilled, payload)?;
        iter.data_mut().next_fulfilled_fn = Some(Heap::from(on_f));
        let on_r = Function::new_callback(scope, c"", 1, next_rejected, payload)?;
        iter.data_mut().next_rejected_fn = Some(Heap::from(on_r));
    }
    let sentinel = iter
        .data()
        .end_of_iteration
        .get(scope)
        .expect("sentinel is created with the callbacks");
    let next_promise = get_next_iteration_result(scope, iter, sentinel)?;
    let on_f = iter
        .data()
        .next_fulfilled_fn
        .get(scope)
        .expect("created above");
    let on_r = iter
        .data()
        .next_rejected_fn
        .get(scope)
        .expect("created above");
    next_promise
        .then(scope, Some(*on_f), Some(*on_r))
        .map_err(|_| ExnThrown)
}

/// The fulfill steps of `next()`: build the `{ value, done }` result from the
/// next-iteration promise's value. The value is either the unique end-of-iteration
/// sentinel (the iterator finished) or the raw chunk (already adopted by the
/// promise resolution, so a thenable chunk arrives here unwrapped). The payload is
/// the iterator; the sentinel lives in its `end_of_iteration` slot.
fn next_fulfilled(
    scope: &Scope<'_>,
    args: CallbackArgs<'_>,
    payload: HandleValue<'_>,
) -> Result<Value, ExnThrown> {
    let iter = iter_from_value(scope, payload)?;
    let sentinel = iter
        .data()
        .end_of_iteration
        .get(scope)
        .expect("sentinel is created with this callback");
    let next = args.get(0);
    // If _next_ is end of iteration: set the iterator finished and return
    // CreateIterResultObject(undefined, true).
    if next.get() == sentinel.as_value() {
        iter.data_mut().is_finished = true;
        let result = create_iter_result(scope, HandleValue::undefined(), true)?;
        return Ok(result.as_value());
    }
    // Otherwise return CreateIterResultObject(next, false).
    let result = create_iter_result(scope, next, false)?;
    Ok(result.as_value())
}

/// The rejection of `get the next iteration result`: mark finished and rethrow.
fn next_rejected(
    scope: &Scope<'_>,
    args: CallbackArgs<'_>,
    payload: HandleValue<'_>,
) -> Result<Value, ExnThrown> {
    let iter = iter_from_value(scope, payload)?;
    iter.data_mut().is_finished = true;
    Err(js::exception::set_pending(
        scope,
        args.get(0),
        js::native::ExceptionStackBehavior::Capture,
    ))
}

/// WebIDL return steps: a finished iterator resolves with `{ value, done: true }`;
/// otherwise mark finished, run the asynchronous iterator return, then wrap.
fn run_return_steps<'r>(
    scope: &'r Scope<'_>,
    iter: &ReadableStreamAsyncIterator<'_>,
    value: HandleValue<'r>,
) -> Result<Promise<'r>, ExnThrown> {
    if iter.data().is_finished {
        let result = create_iter_result(scope, value, true)?;
        return Promise::new_resolved_with_value(scope, result);
    }
    iter.data_mut().is_finished = true;
    let return_promise = asynchronous_iterator_return(scope, iter, value)?;
    let payload = pair_payload(scope, scope.root_value(iter.as_value()), value)?;
    let on_f = Function::new_callback(scope, c"", 1, return_fulfilled, payload)?;
    return_promise
        .then(scope, Some(*on_f), None)
        .map_err(|_| ExnThrown)
}

/// The fulfillment of the asynchronous iterator return: wrap the argument as
/// `{ value, done: true }` (payload = `[iterator, value]`).
fn return_fulfilled(
    scope: &Scope<'_>,
    _args: CallbackArgs<'_>,
    payload: HandleValue<'_>,
) -> Result<Value, ExnThrown> {
    let (_iter_v, value) = pair_parts(scope, payload);
    let result = create_iter_result(scope, value, true)?;
    Ok(result.as_value())
}

/// The Streams "asynchronous iterator return steps": cancel (unless
/// `preventCancel`) and release the reader.
fn asynchronous_iterator_return<'r>(
    scope: &'r Scope<'_>,
    iter: &ReadableStreamAsyncIterator<'_>,
    arg: HandleValue<'r>,
) -> Result<Promise<'r>, ExnThrown> {
    let reader = iter_reader(scope, iter);
    if !iter.data().prevent_cancel {
        let result = algorithms::readable_stream_reader_generic_cancel(scope, &reader, arg);
        algorithms::readable_stream_default_reader_release(scope, &reader)?;
        Ok(result)
    } else {
        algorithms::readable_stream_default_reader_release(scope, &reader)?;
        Promise::new_resolved_with_value(scope, HandleValue::undefined())
    }
}

/// `get the next iteration result`: read a chunk, returning a promise the read
/// request resolves with the raw chunk (adopting a thenable) or `end_of_iteration`
/// on close, or rejects on error.
fn get_next_iteration_result<'r>(
    scope: &'r Scope<'_>,
    iter: &ReadableStreamAsyncIterator<'_>,
    end_of_iteration: Object<'_>,
) -> Result<Promise<'r>, ExnThrown> {
    let reader = iter_reader(scope, iter);
    let promise = Promise::new_pending(scope)?;
    // Build the request directly into the call so it is never held as an
    // untraced `#[must_root]` local.
    algorithms::readable_stream_default_reader_read(
        scope,
        reader,
        ReadRequest::AsyncIter {
            promise: Heap::from(promise),
            reader: Heap::from(reader),
            end_of_iteration: Heap::from(end_of_iteration),
        },
    );
    Ok(promise)
}

fn iter_reader<'r>(
    scope: &'r Scope<'_>,
    iter: &ReadableStreamAsyncIterator<'_>,
) -> DefaultReader<'r> {
    iter.data().reader.get(scope).expect("reader is set")
}

// --- ReadRequest::AsyncIter step bodies (called from `read_request.rs`) -------

/// Chunk steps: resolve `promise` with the raw chunk.
/// Wrapping in `{ value, done }` happens in `next()`'s fulfill steps.
pub(crate) fn async_iter_chunk_steps(
    scope: &Scope<'_>,
    promise: Promise<'_>,
    chunk: HandleValue<'_>,
) -> Result<(), ExnThrown> {
    promise.resolve(scope, chunk)
}

/// Close steps: release the reader, then resolve `promise` with the end-of-iteration
/// sentinel (recognised by `next()`'s fulfill steps).
pub(crate) fn async_iter_close_steps(
    scope: &Scope<'_>,
    promise: Promise<'_>,
    reader: DefaultReader<'_>,
    end_of_iteration: Object<'_>,
) -> Result<(), ExnThrown> {
    algorithms::readable_stream_default_reader_release(scope, &reader)?;
    promise.resolve(scope, end_of_iteration)
}

/// Error steps: release the reader, reject with the error.
pub(crate) fn async_iter_error_steps(
    scope: &Scope<'_>,
    promise: Promise<'_>,
    reader: DefaultReader<'_>,
    e: HandleValue<'_>,
) -> Result<(), ExnThrown> {
    algorithms::readable_stream_default_reader_release(scope, &reader)?;
    promise.reject(scope, e)
}
