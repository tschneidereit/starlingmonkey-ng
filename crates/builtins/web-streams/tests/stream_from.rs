// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Keystone integration tests for `ReadableStream.from`: a sync iterable (array)
//! and an async iterable (async generator) both stream their values, and the
//! returned stream's cancel calls the iterator's `return`. Covered far more
//! thoroughly by `streams/readable-streams/from.any.js`; this is the fast,
//! design-validating (and GC-rooting-validating) smoke test for the native
//! iterator-record callbacks.

// This file contains nothing platform-specific, so skip it on wasm32.
#![cfg(not(target_arch = "wasm32"))]

use core_runtime::config::RuntimeConfig;
use core_runtime::event_loop::run_microtasks;
use core_runtime::runtime::{clear_global_initializers, register_global_initializer, Runtime};
use js::conversion::FromJSVal;
use js::error::ExnThrown;

fn run(code: &str) -> String {
    clear_global_initializers();
    register_global_initializer(web_streams::add_to_global);
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

const DRAIN: &str = r#"
    async function drain(stream) {
        const reader = stream.getReader();
        const out = [];
        for (;;) {
            const { value, done } = await reader.read();
            if (done) break;
            out.push(value);
        }
        return out.join("");
    }
"#;

/// A sync iterable (array) streams its values via the async-from-sync wrapper.
#[test]
fn from_sync_array_streams_values() {
    let out = run(&format!(
        r#"{DRAIN}
        globalThis.__out = "pending";
        const rs = ReadableStream.from(["a", "b", "c"]);
        drain(rs).then(s => {{ globalThis.__out = s; }});
        "#
    ));
    assert_eq!(out, "abc");
}

/// An async iterable (async generator) streams its values.
#[test]
fn from_async_generator_streams_values() {
    let out = run(&format!(
        r#"{DRAIN}
        globalThis.__out = "pending";
        async function* gen() {{ yield "x"; yield "y"; yield "z"; }}
        const rs = ReadableStream.from(gen());
        drain(rs).then(s => {{ globalThis.__out = s; }});
        "#
    ));
    assert_eq!(out, "xyz");
}

/// A sync iterable that yields a rejected promise: the async-from-sync wrapper
/// adopts the rejection and closes the sync iterator (running its `return` /
/// `finally`) before the stream errors, per AsyncFromSyncIteratorContinuation
/// with closeOnRejection = true.
#[test]
fn from_sync_rejected_value_closes_iterator() {
    let out = run(r#"
        globalThis.__out = "pending";
        let cleaned = false;
        const it = (function* () {
            try { yield Promise.reject(new TypeError("boom")); }
            finally { cleaned = true; }
        })();
        ReadableStream.from(it).getReader().read().then(
            () => { globalThis.__out = "resolved"; },
            e => { globalThis.__out = `rejected:${e.constructor.name}:${e.message}|cleaned:${cleaned}`; },
        );
        "#);
    assert_eq!(out, "rejected:TypeError:boom|cleaned:true");
}

/// Cancelling the stream invokes the iterator's `return` method.
#[test]
fn from_cancel_calls_return() {
    let out = run(&format!(
        r#"{DRAIN}
        globalThis.__out = "pending";
        let returned = false;
        const iterable = {{
            [Symbol.asyncIterator]() {{
                let i = 0;
                return {{
                    next() {{ return Promise.resolve({{ value: i++, done: false }}); }},
                    return(v) {{ returned = true; return Promise.resolve({{ value: v, done: true }}); }},
                }};
            }}
        }};
        const rs = ReadableStream.from(iterable);
        const reader = rs.getReader();
        reader.read().then(() => reader.cancel("stop")).then(() => {{
            globalThis.__out = "returned:" + returned;
        }});
        "#
    ));
    assert_eq!(out, "returned:true");
}
