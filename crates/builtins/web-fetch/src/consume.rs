// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Reading a `ReadableStream`-source body to a value (`fully read a body`).
//!
//! For a byte-sequence body, `consume body` reads the source synchronously. For a
//! `ReadableStream`-source body it must drain the stream chunk by chunk. The
//! draining itself — the read loop and byte accumulation — is web-streams'
//! `Read all bytes from reader`, which reads via internal read requests. This
//! module supplies only the `successSteps`: the `convertBytesToJSValue` step that
//! turns the assembled bytes into the requested value.

use js::conversion::ToJSVal;
use js::error::ExnThrown;
use js::gc::scope::Scope;
use js::prelude::HandleValue;
use js::Promise;
use web_streams::readable::read_all_bytes::read_all_bytes;
use web_streams::readable::readable_stream::ReadableStream;

use crate::algorithms::{convert_owned_bytes_to_js_value, ConsumeType};

/// <https://fetch.spec.whatwg.org/#body-fully-read> for a stream-source body
/// (a byte-source body is read synchronously in `consume body` instead).
///
/// Fully read `stream`, returning a promise that settles with the result of
/// applying `conversion` to the concatenated bytes (or rejects if the stream
/// errors or a chunk is not a `Uint8Array`).
pub(crate) fn fully_read_stream_to_value<'r>(
    scope: &'r Scope<'_>,
    stream: ReadableStream<'_>,
    conversion: ConsumeType,
) -> Result<Promise<'r>, ExnThrown> {
    // Step 1: If _taskDestination_ is null, then set _taskDestination_ to the result of `starting a
    //     new parallel queue`.
    // Note: Single-threaded runtime: no parallel queue.
    // Step 2: Let _successSteps_ given a `byte sequence` _bytes_ be to `queue a fetch task` to run
    //     _processBody_ given _bytes_, with _taskDestination_.
    let success_steps = convert_bytes;

    // Step 3: Let _errorSteps_ optionally given an `exception` _exception_ be to `queue a fetch
    //     task` to run _processBodyError_ given _exception_, with _taskDestination_.
    // Handled in `read_all_bytes`.
    // Step 4: Let _reader_ be the result of `getting a reader` for _body_’s `stream`. If that threw
    //     an exception, then run _errorSteps_ with that exception and return.
    // Note: Errors are handled in `read_all_bytes`.
    // Step 5: `Read all bytes` from _reader_, given _successSteps_ and _errorSteps_.
    let payload = conversion_to_payload(conversion).to_jsval_throwing(scope)?;
    read_all_bytes(scope, stream, success_steps, payload)
}

/// The `successSteps` for `consume body`: run `convertBytesToJSValue` on the
/// assembled bytes — owned, so the `ArrayBuffer`/`Uint8Array` results are
/// backed by the accumulator directly with no further copies.
fn convert_bytes<'r>(
    scope: &'r Scope<'r>,
    payload: HandleValue<'_>,
    bytes: Vec<u8>,
) -> Result<HandleValue<'r>, ExnThrown> {
    let conversion = conversion_from_payload(payload.to_int32());
    convert_owned_bytes_to_js_value(scope, bytes::Bytes::from(bytes), conversion)
}

/// Encode a [`ConsumeType`] as the integer carried in the success callback's
/// payload.
fn conversion_to_payload(conversion: ConsumeType) -> i32 {
    match conversion {
        ConsumeType::ArrayBuffer => 0,
        ConsumeType::Bytes => 1,
        ConsumeType::Text => 2,
        ConsumeType::Json => 3,
        ConsumeType::Blob => 4,
        ConsumeType::FormData => 5,
    }
}

/// Decode a [`ConsumeType`] from the integer carried in the success callback's
/// payload (the inverse of [`conversion_to_payload`]).
fn conversion_from_payload(payload: i32) -> ConsumeType {
    match payload {
        0 => ConsumeType::ArrayBuffer,
        1 => ConsumeType::Bytes,
        2 => ConsumeType::Text,
        3 => ConsumeType::Json,
        4 => ConsumeType::Blob,
        5 => ConsumeType::FormData,
        other => unreachable!("unknown consume conversion payload {other}"),
    }
}
