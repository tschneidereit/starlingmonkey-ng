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

use core_runtime::webidl_interface;
use core_runtime::webidl_methods;
use js::error::ExnThrown;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::native::Value;
use js::prelude::{CallbackArgs, HandleValue};
use js::value;
use js::Function;
use js::{Object, Promise};

use super::algorithms;
use super::default_reader::DefaultReader;
use super::read_request::ReadRequest;
use crate::algorithms::{pair_parts, pair_payload};
use crate::support;

/// <https://streams.spec.whatwg.org/#rs-asynciterator>
///
/// The prototype's `[[Prototype]]` is set to `%AsyncIteratorPrototype%` at
/// registration (see `add_to_global`), which supplies `[Symbol.asyncIterator]`.
// WebIDL §3.7.10: the class string of an asynchronous iterator prototype object
// is the interface identifier followed by " AsyncIterator" (with a space).
#[webidl_interface(hidden, to_string_tag = "ReadableStream AsyncIterator")]
pub struct ReadableStreamAsyncIterator {
    /// The `DefaultReader` acquired for this iteration.
    pub(crate) reader: Option<Heap<js::object::Object>>,
    /// The `preventCancel` option captured at creation.
    #[no_trace]
    pub(crate) prevent_cancel: bool,
    /// WebIDL default async iterator `[[isFinished]]`.
    #[no_trace]
    pub(crate) is_finished: bool,
    /// WebIDL default async iterator `[[ongoingPromise]]`, serializing calls.
    pub(crate) ongoing_promise: Option<Heap<js::promise::Promise>>,
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
        let prev = self.data().ongoing_promise.as_ref().map(|h| h.get(scope));
        let ongoing = match prev {
            Some(prev) => {
                let payload = scope.root_value(self.as_value());
                let cb = Function::new_callback(scope, c"", 1, after_ongoing_next, payload)?;
                prev.call_original_then(scope, Some(*cb), Some(*cb))?
            }
            None => run_next_steps(scope, self)?,
        };
        self.data_mut().ongoing_promise = Some(Heap::from(ongoing));
        Ok(ongoing)
    }

    /// <https://webidl.spec.whatwg.org/#dfn-asynchronous-iterator-prototype-object>: `return`.
    #[method(name = "return")]
    fn iterator_return<'r>(
        &self,
        scope: &'r Scope<'_>,
        value: Option<HandleValue<'r>>,
    ) -> Result<Promise<'r>, ExnThrown> {
        // `return` must tolerate being called with no argument (the iteration
        // protocol does so), but reports `length` 1 (fixed up at registration).
        let value = value.unwrap_or_else(|| scope.root_value(value::undefined()));
        let prev = self.data().ongoing_promise.as_ref().map(|h| h.get(scope));
        let ongoing = match prev {
            Some(prev) => {
                let payload = pair_payload(scope, scope.root_value(self.as_value()), value)?;
                let cb = Function::new_callback(scope, c"", 1, after_ongoing_return, payload)?;
                prev.call_original_then(scope, Some(*cb), Some(*cb))?
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
        let undef = scope.root_value(value::undefined());
        let result = support::create_iter_result(scope, undef, true)?;
        return Promise::new_resolved_with_value(scope, result).map_err(|_| ExnThrown);
    }
    let next_promise = get_next_iteration_result(scope, iter)?;
    let payload = scope.root_value(iter.as_value());
    let on_f = Function::new_callback(scope, c"", 1, next_fulfilled, payload)?;
    let on_r = Function::new_callback(scope, c"", 1, next_rejected, payload)?;
    next_promise
        .call_original_then(scope, Some(*on_f), Some(*on_r))
        .map_err(|_| ExnThrown)
}

/// The fulfillment of `get the next iteration result`: mark finished on
/// end-of-iteration, then return the `{ value, done }` result.
fn next_fulfilled(
    scope: &Scope<'_>,
    args: CallbackArgs<'_>,
    payload: HandleValue<'_>,
) -> Result<Value, ExnThrown> {
    let iter = iter_from_value(scope, payload)?;
    let result = args.get(0);
    let result_obj = Object::from_value(scope, *result).map_err(|_| ExnThrown)?;
    let done = result_obj.get_property(scope, c"done")?.get().to_boolean();
    if done {
        iter.data_mut().is_finished = true;
    }
    Ok(result.get())
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
        let result = support::create_iter_result(scope, value, true)?;
        return Promise::new_resolved_with_value(scope, result).map_err(|_| ExnThrown);
    }
    iter.data_mut().is_finished = true;
    let return_promise = asynchronous_iterator_return(scope, iter, value)?;
    let payload = pair_payload(scope, scope.root_value(iter.as_value()), value)?;
    let on_f = Function::new_callback(scope, c"", 1, return_fulfilled, payload)?;
    return_promise
        .call_original_then(scope, Some(*on_f), None)
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
    let result = support::create_iter_result(scope, value, true)?;
    Ok(result.get())
}

/// The Streams "asynchronous iterator return steps": cancel (unless
/// `preventCancel`) and release the reader.
fn asynchronous_iterator_return<'r>(
    scope: &'r Scope<'_>,
    iter: &ReadableStreamAsyncIterator<'_>,
    arg: HandleValue<'r>,
) -> Result<Promise<'r>, ExnThrown> {
    let reader = iter_reader(scope, iter)?;
    if !iter.data().prevent_cancel {
        let result = algorithms::readable_stream_reader_generic_cancel(scope, &reader, arg);
        algorithms::readable_stream_default_reader_release(scope, &reader)?;
        Ok(result)
    } else {
        algorithms::readable_stream_default_reader_release(scope, &reader)?;
        let undef = scope.root_value(value::undefined());
        Promise::new_resolved_with_value(scope, undef).map_err(|_| ExnThrown)
    }
}

/// `get the next iteration result`: read a chunk, returning a promise that
/// resolves to a `{ value, done }` result (or rejects on error).
fn get_next_iteration_result<'r>(
    scope: &'r Scope<'_>,
    iter: &ReadableStreamAsyncIterator<'_>,
) -> Result<Promise<'r>, ExnThrown> {
    let reader = iter_reader(scope, iter)?;
    let reader_obj = iter
        .data()
        .reader
        .as_ref()
        .expect("reader is set")
        .get(scope);
    let promise = Promise::new_pending(scope)?;
    // Build the request directly into the call so it is never held as an
    // untraced `#[must_root]` local.
    algorithms::readable_stream_default_reader_read(
        scope,
        &reader,
        ReadRequest::AsyncIter {
            promise: Heap::from(promise),
            reader: Heap::from(reader_obj),
        },
    );
    Ok(promise)
}

fn iter_reader<'r>(
    scope: &'r Scope<'_>,
    iter: &ReadableStreamAsyncIterator<'_>,
) -> Result<DefaultReader<'r>, ExnThrown> {
    iter.data()
        .reader
        .as_ref()
        .expect("reader is set")
        .get(scope)
        .cast::<DefaultReader>()
        .map_err(|_| ExnThrown)
}

// --- ReadRequest::AsyncIter step bodies (called from `read_request.rs`) -------

/// Chunk steps: resolve with `{ value: chunk, done: false }`.
pub(crate) fn async_iter_chunk_steps(
    scope: &Scope<'_>,
    promise: Promise<'_>,
    chunk: HandleValue<'_>,
) -> Result<(), ExnThrown> {
    let result = support::create_iter_result(scope, chunk, false)?;
    promise.resolve(scope, result)
}

/// Close steps: release the reader, resolve with `{ value: undefined, done: true }`.
pub(crate) fn async_iter_close_steps(
    scope: &Scope<'_>,
    promise: Promise<'_>,
    reader: Object<'_>,
) -> Result<(), ExnThrown> {
    let reader = reader.cast::<DefaultReader>().map_err(|_| ExnThrown)?;
    algorithms::readable_stream_default_reader_release(scope, &reader)?;
    let undef = scope.root_value(value::undefined());
    let result = support::create_iter_result(scope, undef, true)?;
    promise.resolve(scope, result)
}

/// Error steps: release the reader, reject with the error.
pub(crate) fn async_iter_error_steps(
    scope: &Scope<'_>,
    promise: Promise<'_>,
    reader: Object<'_>,
    e: HandleValue<'_>,
) -> Result<(), ExnThrown> {
    let reader = reader.cast::<DefaultReader>().map_err(|_| ExnThrown)?;
    algorithms::readable_stream_default_reader_release(scope, &reader)?;
    promise.reject(scope, e)
}
