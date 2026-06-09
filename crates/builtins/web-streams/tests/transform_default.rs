// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Keystone integration tests for the transform default path: the transform
//! stream wires its writable side (sink → transform) to its readable side
//! (source) through the controller, on top of the readable/writable defaults.
//! `web-globals` is registered because the underlying writable controller
//! creates an `AbortController`.
//!
//! Behaviour is covered far more thoroughly by the `streams/transform-streams`
//! WPT suites; this is the fast, design-validating smoke test.

use core_runtime::config::RuntimeConfig;
use core_runtime::event_loop::run_microtasks;
use core_runtime::runtime::{clear_global_initializers, register_global_initializer, Runtime};
use js::conversion::FromJSVal;
use js::error::ExnThrown;

fn run(code: &str) -> String {
    clear_global_initializers();
    register_global_initializer(|scope, global| {
        web_globals::add_to_global(scope, global);
        web_streams::add_to_global(scope, global);
    });
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();
    if js::compile::evaluate_with_filename(&scope, code, "test.js", 1).is_err() {
        panic!("evaluation threw: {:?}", ExnThrown::capture(&scope));
    }
    run_microtasks(&scope);
    let out = js::compile::evaluate_with_filename(&scope, "globalThis.__out", "out.js", 1)
        .expect("reading __out threw");
    String::from_jsval(&scope, out, ()).unwrap()
}

/// A transformer that uppercases each chunk: written chunks come out transformed
/// on the readable side.
#[test]
fn transform_uppercases_chunks() {
    let out = run(r#"
        globalThis.__out = "pending";
        const ts = new TransformStream({
            transform(chunk, controller) { controller.enqueue(chunk.toUpperCase()); },
        });
        const writer = ts.writable.getWriter();
        const reader = ts.readable.getReader();
        writer.write("a");
        reader.read().then(r => { globalThis.__out = r.value; });
        "#);
    assert_eq!(out, "A");
}

/// The default (identity) transform passes chunks through unchanged.
#[test]
fn identity_transform_passes_through() {
    let out = run(r#"
        globalThis.__out = "pending";
        const ts = new TransformStream();
        const writer = ts.writable.getWriter();
        const reader = ts.readable.getReader();
        writer.write("hello");
        reader.read().then(r => { globalThis.__out = r.value; });
        "#);
    assert_eq!(out, "hello");
}

/// `flush` runs on close and can enqueue a final chunk; closing the writable
/// closes the readable.
#[test]
fn flush_enqueues_and_close_propagates() {
    let out = run(r#"
        globalThis.__out = "pending";
        const ts = new TransformStream({
            transform(chunk, c) { c.enqueue(chunk); },
            flush(c) { c.enqueue("end"); },
        });
        const writer = ts.writable.getWriter();
        const reader = ts.readable.getReader();
        writer.write("x");
        writer.close();
        const seen = [];
        function pump() {
            return reader.read().then(r => {
                if (r.done) { globalThis.__out = seen.join(","); return; }
                seen.push(r.value);
                return pump();
            });
        }
        pump();
        "#);
    assert_eq!(out, "x,end");
}

/// `readable` and `writable` are the two sides and round-trip object identity.
#[test]
fn readable_writable_shape() {
    let out = run(r#"
        const ts = new TransformStream();
        const a = ts.readable instanceof ReadableStream;
        const b = ts.writable instanceof WritableStream;
        const c = ts.readable === ts.readable;
        globalThis.__out = `${a},${b},${c}`;
        "#);
    assert_eq!(out, "true,true,true");
}
