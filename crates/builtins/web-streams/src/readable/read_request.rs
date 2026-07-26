// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Read requests.
//!
//! <https://streams.spec.whatwg.org/#read-request>
//!
//! A read request is a spec struct bundling three native step-algorithms —
//! `chunk steps` (given a chunk), `close steps`, and `error steps` (given an
//! error). Each consumer of a `DefaultReader` (a `read()` call,
//! `pipeTo`, `tee`, the async iterator) supplies its own. They are modelled as
//! an enum — one variant per consumer — rather than as JS values, because the
//! steps are native and the `[[readRequests]]` list must trace whatever rooted
//! state they hold. The `Traceable` derive rejects enums, so `Trace` is
//! hand-written.

use js::error::ExnThrown;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::iteration::create_iter_result;
use js::native::JSTracer;
use js::prelude::HandleValue;
use js::promise::Promise;

use crate::readable::algorithms::{
    byte_tee_byob_chunk_steps, byte_tee_byob_close_steps, byte_tee_default_chunk_steps,
    byte_tee_default_close_steps, byte_tee_set_not_reading, pipe_read_request_chunk_steps,
    tee_read_request_chunk_steps, tee_read_request_close_steps, tee_read_request_error_steps,
    ByteTeeStateImpl, PipeStateImpl, TeeStateImpl,
};
use crate::readable::default_reader::DefaultReaderImpl;
use crate::readable::read_all_bytes::{
    read_all_bytes_chunk_steps, read_all_bytes_close_steps, read_all_bytes_error_steps,
    ReadAllBytesStateImpl,
};

/// An entry in a reader's `[[readRequests]]` list.
///
/// Marked `#[js::must_root]` because each variant holds a `Heap` (the read
/// promise, or tee/pipe/iterator state): crown then rejects holding a
/// `ReadRequest` by value in a plain local across an allocation — the exact
/// untraced-`Heap`-across-GC hazard — forcing callers to keep it traced
/// (`RootedTraceableBox`) until they settle it.
#[js::must_root]
#[derive(js::ScopeRoot)]
pub(crate) enum ReadRequest {
    /// A `DefaultReader.read()` call.
    ///
    /// <https://streams.spec.whatwg.org/#default-reader-read>: chunk steps
    /// resolve the promise with `{ value: chunk, done: false }`, close steps
    /// with `{ value: undefined, done: true }`, and error steps reject it.
    Read {
        /// The promise returned to the caller of `read()`.
        promise: Heap<Promise>,
    },
    /// The read request used by `ReadableStreamDefaultTee`'s pull algorithm.
    /// Its steps drive both branches; the shared tee state is a `TeeState` (see
    /// `crate::algorithms`'s tee helpers).
    ///
    /// <https://streams.spec.whatwg.org/#abstract-opdef-readablestreamdefaulttee>
    Tee {
        /// The tee state object.
        state: Heap<TeeStateImpl>,
    },
    /// The read request used by `ReadableStreamPipeTo`'s pipe loop. Its chunk
    /// steps write the chunk to the destination and continue the loop; close and
    /// error steps do nothing (the closed-promise propagation reactions drive
    /// shutdown). The shared pipe state is a `PipeState` (see `crate::algorithms`'s
    /// pipe helpers).
    ///
    /// <https://streams.spec.whatwg.org/#readable-stream-pipe-to>
    Pipe {
        /// The pipe state object.
        state: Heap<PipeStateImpl>,
    },
    /// The read request used by the `ReadableStream` async iterator's `next()`.
    /// Its steps settle the per-call promise and release the reader on close or
    /// error.
    ///
    /// <https://streams.spec.whatwg.org/#rs-asynciterator>
    AsyncIter {
        /// The promise returned by `get the next iteration result`.
        promise: Heap<js::promise::Promise>,
        /// The reader to release on close or error.
        reader: Heap<DefaultReaderImpl>,
        /// The unique "end of iteration" sentinel: the close steps resolve
        /// `promise` with it, and `next()`'s fulfill steps recognise it by
        /// identity to finish the iterator.
        end_of_iteration: Heap<js::object::Object>,
    },
    /// The default-reader read request used by `ReadableByteStreamTee`'s
    /// `pullWithDefaultReader`. Its steps drive both byte branches; the shared
    /// tee state is a `ByteTeeState` (see `crate::algorithms`'s byte-tee helpers).
    ///
    /// <https://streams.spec.whatwg.org/#abstract-opdef-readablebytestreamtee>
    ByteTeeDefault {
        /// The byte-tee state object.
        state: Heap<ByteTeeStateImpl>,
    },
    /// The internal read request used by `Read all bytes from reader`. Its chunk
    /// steps accumulate the chunk's bytes and issue the next internal read, its
    /// close steps run the caller's success callback on the assembled bytes, and
    /// its error steps reject the result promise. The shared state is a
    /// `ReadAllBytesState` (see `crate::readable::read_all_bytes`).
    ///
    /// Unlike a `Read` request, the chunk never becomes an `{ value, done }`
    /// object resolved into a promise, so a hijacked `Object.prototype.then`
    /// cannot intercept it.
    ///
    /// <https://streams.spec.whatwg.org/#readablestreamdefaultreader-read-all-bytes>
    Consume {
        /// The read-all-bytes state object.
        state: Heap<ReadAllBytesStateImpl>,
    },
    /// An out-of-crate native consumer's read request (see
    /// `crate::readable::native_read`): the steps are plain fn pointers
    /// receiving the rooted `payload`. Like `Consume`, delivery never builds an
    /// `{ value, done }` object or resolves a promise, so author code cannot
    /// observe it.
    Native {
        /// The consumer's step functions.
        steps: crate::readable::native_read::NativeReadSteps,
        /// The consumer's rooted state object.
        payload: Heap<js::object::Object>,
    },
}

impl<'s> StackReadRequest<'s> {
    /// Perform the read request's `chunk steps` given `chunk`.
    pub(crate) fn chunk_steps(
        self,
        scope: &Scope<'_>,
        chunk: HandleValue<'_>,
    ) -> Result<(), ExnThrown> {
        match self {
            StackReadRequest::Read { promise } => {
                let result = create_iter_result(scope, chunk, false)?;
                promise.resolve(scope, result)
            }
            StackReadRequest::Tee { state } => tee_read_request_chunk_steps(scope, state, chunk),
            StackReadRequest::Pipe { state } => pipe_read_request_chunk_steps(scope, state, chunk),
            StackReadRequest::AsyncIter { promise, .. } => {
                super::async_iterator::async_iter_chunk_steps(scope, promise, chunk)
            }
            StackReadRequest::ByteTeeDefault { state } => {
                byte_tee_default_chunk_steps(scope, state, chunk)
            }
            StackReadRequest::Consume { state } => read_all_bytes_chunk_steps(scope, state, chunk),
            StackReadRequest::Native { steps, payload } => (steps.chunk)(scope, payload, chunk),
        }
    }

    /// Perform the read request's `close steps`.
    pub(crate) fn close_steps(self, scope: &Scope<'_>) -> Result<(), ExnThrown> {
        match self {
            StackReadRequest::Read { promise } => {
                let result = create_iter_result(scope, HandleValue::undefined(), true)?;
                promise.resolve(scope, result)
            }
            StackReadRequest::Tee { state } => tee_read_request_close_steps(scope, state),
            // The pipe loop's close steps do nothing — shutdown is driven by the
            // closing-propagation reaction on the reader's closed promise.
            StackReadRequest::Pipe { .. } => Ok(()),
            StackReadRequest::AsyncIter {
                promise,
                reader,
                end_of_iteration,
            } => super::async_iterator::async_iter_close_steps(
                scope,
                promise,
                reader,
                end_of_iteration,
            ),
            StackReadRequest::ByteTeeDefault { state } => {
                byte_tee_default_close_steps(scope, state)
            }
            StackReadRequest::Consume { state } => read_all_bytes_close_steps(scope, state),
            StackReadRequest::Native { steps, payload } => (steps.close)(scope, payload),
        }
    }

    /// Perform the read request's `error steps` given `e`.
    pub(crate) fn error_steps(
        self,
        scope: &Scope<'_>,
        e: HandleValue<'_>,
    ) -> Result<(), ExnThrown> {
        match self {
            StackReadRequest::Read { promise } => promise.reject(scope, e),
            StackReadRequest::Tee { state } => tee_read_request_error_steps(scope, state, e),
            // The pipe loop's error steps do nothing — shutdown is driven by the
            // error-propagation reaction on the reader's closed promise.
            StackReadRequest::Pipe { .. } => Ok(()),
            StackReadRequest::AsyncIter {
                promise, reader, ..
            } => super::async_iterator::async_iter_error_steps(scope, promise, reader, e),
            // The byte-tee default read request's error steps only set `reading`
            // to false; stream errors are forwarded via the reader's closed promise.
            StackReadRequest::ByteTeeDefault { state } => {
                let _ = e;
                byte_tee_set_not_reading(scope, state);
                Ok(())
            }
            StackReadRequest::Consume { state } => read_all_bytes_error_steps(scope, state, e),
            StackReadRequest::Native { steps, payload } => (steps.error)(scope, payload, e),
        }
    }
}

// Safety: trace every GC pointer held by each variant.
unsafe impl js::heap::Trace for ReadRequest {
    #[inline]
    unsafe fn trace(&self, trc: *mut JSTracer) {
        match self {
            ReadRequest::Read { promise } => promise.trace(trc),
            ReadRequest::Tee { state } => state.trace(trc),
            ReadRequest::Pipe { state } => state.trace(trc),
            ReadRequest::AsyncIter {
                promise,
                reader,
                end_of_iteration,
            } => {
                promise.trace(trc);
                reader.trace(trc);
                end_of_iteration.trace(trc);
            }
            ReadRequest::ByteTeeDefault { state } => state.trace(trc),
            ReadRequest::Consume { state } => state.trace(trc),
            // `steps` holds only fn pointers; the payload is the sole GC pointer.
            ReadRequest::Native { payload, .. } => payload.trace(trc),
        }
    }
}

/// An entry in a BYOB reader's `[[readIntoRequests]]` list.
///
/// <https://streams.spec.whatwg.org/#read-into-request>
///
/// Like [`ReadRequest`] but for `BYOBReader.read(view)`. Its
/// `chunk`/`close` steps both receive a view (the filled or empty destination),
/// unlike a default read request whose close step produces `undefined`.
///
/// Marked `#[js::must_root]` for the same reason as [`ReadRequest`]: its
/// variants hold a `Heap`, so crown forbids holding one untraced across a GC.
#[js::must_root]
#[derive(js::ScopeRoot)]
pub(crate) enum ReadIntoRequest {
    /// A `BYOBReader.read(view)` call.
    ///
    /// <https://streams.spec.whatwg.org/#byob-reader-read>: chunk steps resolve
    /// the promise with `{ value: chunk, done: false }`, close steps with
    /// `{ value: chunk, done: true }`, and error steps reject it.
    Read {
        /// The promise returned to the caller of `read()`.
        promise: Heap<Promise>,
    },
    /// The BYOB read-into request used by `ReadableByteStreamTee`'s
    /// `pullWithBYOBReader`. Its steps drive both byte branches; the shared tee
    /// state is a `ByteTeeState` (see `crate::algorithms`'s byte-tee helpers).
    ///
    /// <https://streams.spec.whatwg.org/#abstract-opdef-readablebytestreamtee>
    ByteTeeByob {
        /// The byte-tee state object.
        state: Heap<ByteTeeStateImpl>,
        /// Whether this read-into targets branch 2 (`forBranch2`).
        for_branch2: bool,
    },
}

impl<'s> StackReadIntoRequest<'s> {
    /// Perform the read-into request's `chunk steps` given `chunk`.
    pub(crate) fn chunk_steps(
        self,
        scope: &Scope<'_>,
        chunk: HandleValue<'_>,
    ) -> Result<(), ExnThrown> {
        match self {
            StackReadIntoRequest::Read { promise } => {
                let result = create_iter_result(scope, chunk, false)?;
                promise.resolve(scope, result)
            }
            StackReadIntoRequest::ByteTeeByob { state, for_branch2 } => {
                byte_tee_byob_chunk_steps(scope, state, for_branch2, chunk)
            }
        }
    }

    /// Perform the read-into request's `close steps` given `chunk`.
    pub(crate) fn close_steps(
        self,
        scope: &Scope<'_>,
        chunk: HandleValue<'_>,
    ) -> Result<(), ExnThrown> {
        match self {
            StackReadIntoRequest::Read { promise } => {
                let result = create_iter_result(scope, chunk, true)?;
                promise.resolve(scope, result)
            }
            StackReadIntoRequest::ByteTeeByob { state, for_branch2 } => {
                byte_tee_byob_close_steps(scope, state, for_branch2, chunk)
            }
        }
    }

    /// Perform the read-into request's `error steps` given `e`.
    pub(crate) fn error_steps(
        self,
        scope: &Scope<'_>,
        e: HandleValue<'_>,
    ) -> Result<(), ExnThrown> {
        match self {
            StackReadIntoRequest::Read { promise } => promise.reject(scope, e),
            // The byte-tee BYOB read-into request's error steps only set `reading`
            // to false; stream errors are forwarded via the reader's closed promise.
            StackReadIntoRequest::ByteTeeByob { state, .. } => {
                let _ = e;
                byte_tee_set_not_reading(scope, state);
                Ok(())
            }
        }
    }
}

// Safety: trace every GC pointer held by each variant.
unsafe impl js::heap::Trace for ReadIntoRequest {
    #[inline]
    unsafe fn trace(&self, trc: *mut JSTracer) {
        match self {
            ReadIntoRequest::Read { promise } => promise.trace(trc),
            ReadIntoRequest::ByteTeeByob { state, .. } => state.trace(trc),
        }
    }
}
