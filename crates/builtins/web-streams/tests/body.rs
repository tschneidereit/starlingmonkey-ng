// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Keystone test for the bytes → `ReadableStream` bridge (`web_streams::body`).
//!
//! Fetch (and the File API) create response/request body streams from Rust byte
//! buffers via `readable_stream_from_bytes`, then hand them to JS as `.body`.
//! This validates that such a Rust-created stream is a fully functional JS
//! `ReadableStream`: a reader drains it chunk-by-chunk and observes exactly the
//! original bytes, then `done`. Runs JS, drains microtasks, reads back a result
//! stashed on `globalThis`.

use core_runtime::config::RuntimeConfig;
use core_runtime::event_loop::run_microtasks;
use core_runtime::runtime::{clear_global_initializers, register_global_initializer, Runtime};
use js::conversion::FromJSVal;
use web_streams::readable::ReadableStream;

/// Create a stream from `bytes`, expose it as `globalThis.__stream`, then evaluate
/// `code`, drain microtasks, and return `String(globalThis.__out)`.
fn run_with_stream(bytes: &[u8], code: &str) -> String {
    clear_global_initializers();
    register_global_initializer(|scope, global| web_streams::add_to_global(scope, global));
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();
    let global = scope.global();
    let stream = ReadableStream::from_bytes(&scope, bytes).expect("create stream from bytes");
    global
        .set_property(&scope, c"__stream", scope.root_value(stream.as_value()))
        .expect("expose __stream");
    if js::compile::evaluate_with_filename(&scope, code, "test.js", 1).is_err() {
        panic!(
            "evaluation threw: {:?}",
            js::error::ExnThrown::capture(&scope)
        );
    }
    run_microtasks(&scope);
    let out = js::compile::evaluate_with_filename(&scope, "globalThis.__out", "out.js", 1)
        .expect("reading __out threw");
    String::from_jsval(&scope, out, ()).unwrap()
}

/// A non-empty Rust-created stream drains to exactly its original bytes, then
/// closes. Chunks are `Uint8Array`s (as observed for `Response.body`).
#[test]
fn bytes_round_trip_through_js_reader() {
    let out = run_with_stream(
        b"hello world",
        r#"
        globalThis.__out = "pending";
        (async () => {
            const reader = globalThis.__stream.getReader();
            const bytes = [];
            let chunkCount = 0;
            for (;;) {
                const { value, done } = await reader.read();
                if (done) break;
                chunkCount++;
                if (!(value instanceof Uint8Array)) {
                    globalThis.__out = "chunk not Uint8Array: " + value;
                    return;
                }
                bytes.push(...value);
            }
            globalThis.__out = chunkCount + ":" + bytes.join(",");
        })();
        "#,
    );
    // "hello world" is 11 bytes, delivered as a single Uint8Array chunk.
    assert_eq!(out, "1:104,101,108,108,111,32,119,111,114,108,100");
}

/// An empty Rust-created stream is immediately closed: the first `read()`
/// resolves to `{ done: true }` with no chunk.
#[test]
fn empty_stream_closes_immediately() {
    let out = run_with_stream(
        b"",
        r#"
        globalThis.__out = "pending";
        (async () => {
            const reader = globalThis.__stream.getReader();
            const { value, done } = await reader.read();
            globalThis.__out = `done=${done},value=${value}`;
        })();
        "#,
    );
    assert_eq!(out, "done=true,value=undefined");
}
