// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Internal (native) reading for out-of-crate consumers.
//!
//! A native consumer that pulls a stream one chunk at a time, such as fetch's
//! forwarding of incoming to outgoing bodies, must not read through
//! the author-facing `getReader()`/`read()` path: that builds `{ value, done }`
//! iterator-result objects and resolves promises with them, so a hijacked
//! `Object.prototype.then` (or a patched `getReader`/`read`) can observe and
//! corrupt the bytes. Internal read requests deliver each chunk directly to
//! native steps.
//!
//! The steps are plain `fn` pointers receiving a rooted `payload` object (the
//! consumer's state, typically a `#[jsclass]` instance), mirroring the spec's
//! read request struct without any JS-visible machinery.

use js::error::ExnThrown;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::prelude::HandleValue;
use js::Object;

use super::algorithms::{
    acquire_readable_stream_default_reader, readable_stream_default_reader_read,
};
use super::default_reader::DefaultReader;
use super::read_request::ReadRequest;
use super::readable_stream::ReadableStream;

/// The native steps of one read request: `chunk` receives the chunk value,
/// `close` runs at end of stream, `error` receives the stream's error. Each is
/// handed the `payload` object given to [`native_reader_read`].
#[derive(Clone, Copy)]
pub struct NativeReadSteps {
    pub chunk: fn(&Scope<'_>, Object<'_>, HandleValue<'_>) -> Result<(), ExnThrown>,
    pub close: fn(&Scope<'_>, Object<'_>) -> Result<(), ExnThrown>,
    pub error: fn(&Scope<'_>, Object<'_>, HandleValue<'_>) -> Result<(), ExnThrown>,
}

/// Acquire an internal default reader for `stream`, locking it. The returned
/// reader object is only useful with [`native_reader_read`] and never exposed
/// to author code.
pub fn acquire_native_reader<'r>(
    scope: &'r Scope<'_>,
    stream: &ReadableStream<'_>,
) -> Result<DefaultReader<'r>, ExnThrown> {
    acquire_readable_stream_default_reader(scope, stream)
}

/// Issue one internal read on a reader from [`acquire_native_reader`],
/// delivering the result to `steps` with `payload`. Queued chunks are delivered
/// synchronously, so a consumer that issues its next read directly from its
/// `chunk` step must defer it (a microtask, or resuming from async work) to
/// keep the loop iterative.
pub fn native_reader_read(
    scope: &Scope<'_>,
    reader: DefaultReader<'_>,
    steps: NativeReadSteps,
    payload: Object<'_>,
) -> Result<(), ExnThrown> {
    readable_stream_default_reader_read(
        scope,
        reader,
        ReadRequest::Native {
            steps,
            payload: Heap::from(payload),
        },
    );
    Ok(())
}
