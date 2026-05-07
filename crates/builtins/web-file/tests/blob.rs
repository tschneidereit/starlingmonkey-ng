// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Integration tests for the Blob API.

// This file contains nothing platform-specific, so skip it on wasm32.
#![cfg(not(target_arch = "wasm32"))]

use core_runtime::test_util::{eval_with_setup, throws_with_setup};
use js::conversion::FromJSVal as _;
use libstarling::{
    event_loop::{run_to_completion, with_event_loop, EventLoop},
    runtime::{clear_global_initializers, Runtime},
};

fn setup() {
    core_runtime::runtime::register_global_initializer(|scope, global| {
        web_globals::add_to_global(scope, global);
        web_file::add_to_global(scope, global);
    });
}

fn eval(code: &str) -> String {
    eval_with_setup(setup, code)
}

fn throws(code: &str) -> bool {
    throws_with_setup(setup, code)
}

/// Evaluate `code` (which sets `globalThis.__out`), drive the event loop to
/// completion, and return `__out`.
fn run_and_get_out(code: &str) -> String {
    clear_global_initializers();
    libstarling::register_builtins();
    let rt = Runtime::init(&core_runtime::config::RuntimeConfig::default());
    let scope = rt.default_global();
    let rawcx = unsafe { scope.cx_mut().raw_cx() };
    let el = EventLoop::new();

    {
        with_event_loop(&el, |_| {
            js::compile::evaluate_with_filename(&scope, code, "<test>", 1).expect("eval failed");
        });
    }
    // Drain microtasks queued during top-level evaluation (e.g. a synchronously settled promise's
    // reactions), as the runtime does before stepping the event loop.
    js::jobs::run_jobs(&scope);

    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    tokio_rt.block_on(async { unsafe { run_to_completion(rawcx, &el, tokio::time::sleep).await } });

    let out =
        js::compile::evaluate_with_filename(&scope, "globalThis.__out", "<check>", 1).unwrap();
    String::from_jsval(&scope, out, ()).unwrap()
}

#[test]
fn wtf() {
   assert_eq!(run_and_get_out(r#"
       new Blob([
           null,
           undefined,
           true,
           false,
           0,
           1,
           new String("stringobject"),
           [],
           ['x', 'y'],
           {},
           { 0: "FAIL", length: 1 },
           { toString: function() { return "stringA"; } },
           { toString: undefined, valueOf: function() { return "stringB"; } },
           { valueOf: function() { assert_unreached("Should not call valueOf if toString is present on the prototype."); } }
         ]).text().then((t) => globalThis.__out = t);
       "#), "nullundefinedtruefalse01stringobjectx,y[object Object][object Object]stringAstringB[object Object]") 
}

#[test]
fn blob_exists() {
    assert_eq!(eval("typeof Blob"), "function");
}

#[test]
fn blob_construct() {
    assert_eq!(eval("new Blob() instanceof Blob"), "true");
}

#[test]
fn constructor_empty() {
    assert_eq!(
        eval(
            r#"
            const blob = new Blob();
            JSON.stringify([blob.size, blob.type])
            "#
        ),
        "[0,\"\"]"
    );
}

#[test]
fn constructor_bad_init() {
    assert!(throws("new Blob(1)"));
}

#[test]
fn constructor_bad_opts() {
    assert!(throws("new Blob([1,2,3], 1)"));
}

#[test]
fn constructor_string_part() {
    assert_eq!(
        run_and_get_out(
            r#"
            const blob = new Blob(["hello world"]);
            blob.text().then((t) => globalThis.__out = t);
            "#
        ),
        "hello world"
    );
}

#[test]
fn constructor_typed_array_part() {
    assert_eq!(
        run_and_get_out(
            r#"
            const blob = new Blob([new Uint8Array([1, 2, 3])]);
            blob.bytes().then((t) => globalThis.__out = [blob.size, t]);
            "#
        ),
        "3,1,2,3"
    )
}

#[test]
fn constructor_array_buffer_part() {
    assert_eq!(
        run_and_get_out(
            r#"
            const buf = new ArrayBuffer(3);
            const view = new Uint8Array(buf);
            view[0] = 1;
            view[1] = 2;
            view[2] = 3;
            const blob = new Blob([buf]);
            blob.bytes().then((t) => globalThis.__out = t);
            "#
        ),
        "1,2,3"
    )
}

#[test]
fn constructor_array_buffer_view_part() {
    assert_eq!(
        run_and_get_out(
            r#"
            const buf = new ArrayBuffer(3);
            const view = new Uint8Array(buf);
            view[0] = 1;
            view[1] = 2;
            view[2] = 3;
            const blob = new Blob([view]);
            blob.bytes().then((t) => globalThis.__out = t);
            "#
        ),
        "1,2,3"
    )
}

#[test]
fn constructor_blob_part() {
    assert_eq!(
        run_and_get_out(
            r#"
            const blob1 = new Blob(["hello world"], { type: "application/json" });
            const blob = new Blob([blob1]);
            // blob.type should not be inherited.
            blob.text().then((t) => globalThis.__out = [t, blob.type]);
            "#
        ),
        "hello world,"
    )
}

#[test]
fn constructor_stringify_part() {
    assert_eq!(
        run_and_get_out(
            r#"
            const blob = new Blob([1]);
            blob.bytes().then((b) => globalThis.__out = b);
            "#
        ),
        "49"
    )
}

#[test]
fn constructor_nested_array() {
    assert_eq!(
        run_and_get_out(
            r#"
            const blob = new Blob(["hello", " ", ["world", ["!"]]]);
            blob.text().then((t) => globalThis.__out = t);
            "#
        ),
        // Don't go deeper before strigifying.
        "hello world,!"
    )
}

#[test]
fn constructor_mixed_parts() {
    assert_eq!(
        run_and_get_out(
            // From <https://w3c.github.io/FileAPI/#example-74beb70c>
            r#"
            // Create a new Blob object

            var a = new Blob();

            // Create an 8-byte ArrayBuffer
            // buffer could also come from reading a File

            var buffer = new ArrayBuffer(8);

            // Create ArrayBufferView objects based on buffer

            var shorts = new Uint16Array(buffer, 4, 2);
            var bytes = new Uint8Array(buffer, shorts.byteOffset + shorts.byteLength);

            var b = new Blob(["foobarbazetcetc" + "birdiebirdieboo"], {type: "text/plain;charset=utf-8"});

            var c = new Blob([b, shorts]);

            var a = new Blob([b, c, bytes]);

            var d = new Blob([buffer, b, c, bytes]);

            d.bytes().then((b) => globalThis.__out = b);
            "#
        ),
        "0,0,0,0,0,0,0,0,102,111,111,98,97,114,98,97,122,101,116,99,101,116,99,98,105,114,100,105,101,98,105,114,100,105,101,98,111,111,102,111,111,98,97,114,98,97,122,101,116,99,101,116,99,98,105,114,100,105,101,98,105,114,100,105,101,98,111,111,0,0,0,0"
    )
}

#[test]
fn constructor_type_opt() {
    assert_eq!(
        eval(
            r#"
        const blob = new Blob(["hello"], { type: "text/plain" });
        blob.type
        "#
        ),
        "text/plain"
    );
}

#[test]
fn constructor_normalize_type_opt() {
    assert_eq!(
        eval(
            r#"
        const blob = new Blob(["hello"], { type: "text/あ" });
        blob.type
        "#
        ),
        // If out of range, set to empty string.
        ""
    );
}

#[test]
fn normalize_line_endings() {
    assert_eq!(
        run_and_get_out(
            r#"
            new Blob(["hello\nworld\ryay\r\nok"], {endings: "native"}).text().then((t) => globalThis.__out = t)
            "#
        ),
        "hello\nworld\nyay\nok"
    )
}

#[test]
fn method_text() {
    assert_eq!(
        run_and_get_out(
            r#"
            new Blob(["hello world", 1]).text().then((t) => globalThis.__out = [typeof t === "string", t]);
            "#
        ),
        "true,hello world1"
    )
}

#[test]
fn method_array_buffer() {
    assert_eq!(
        run_and_get_out(
            r#"
            new Blob([new Uint8Array([1, 2, 3])]).arrayBuffer().then(
                (b) => globalThis.__out = [b instanceof ArrayBuffer, new Uint8Array(b)]);
            "#
        ),
        "true,1,2,3"
    )
}

#[test]
fn method_bytes() {
    assert_eq!(
        run_and_get_out(
            r#"
            new Blob([new Uint8Array([1, 2, 3])]).bytes().then(
                (b) => globalThis.__out = [b instanceof Uint8Array, b]);
            "#
        ),
        "true,1,2,3"
    )
}

#[test]
fn method_stream() {
    assert_eq!(
        run_and_get_out(
            r#"
            const blob = new Blob([new Uint8Array([1,2,3,4,5,6,7,8,9,10])]);
            const stream = blob.stream();
            globalThis.__out = (stream instanceof ReadableStream) + ",";
            (async () => {
                for await (const chunk of stream) {
                    globalThis.__out += chunk;
                }
            })();
            "#
        ),
        "true,1,2,3,4,5,6,7,8,9,10"
    )
}
