// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Keystone integration tests for the writable default path: the write/close/
//! abort state machine, the writer, and the default controller. `web-globals`
//! is registered alongside `web-streams` because the writable controller
//! creates an `AbortController` during setup.
//!
//! These exercise behaviour covered far more thoroughly by the
//! `streams/writable-streams` WPT suites; this is the fast, design-validating
//! smoke test.

use core_runtime::config::RuntimeConfig;
use core_runtime::event_loop::run_microtasks;
use core_runtime::runtime::{clear_global_initializers, register_global_initializer, Runtime};
use js::conversion::FromJSVal;
use js::error::ExnThrown;

/// Evaluate `code`, drain microtasks, and return `String(globalThis.__out)`.
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

/// Chunks written through a writer reach the sink's `write` algorithm in order,
/// and the per-write promises settle.
#[test]
fn write_chunks_reach_sink() {
    let out = run(r#"
        globalThis.__out = "pending";
        const written = [];
        const ws = new WritableStream({ write(chunk) { written.push(chunk); } });
        const w = ws.getWriter();
        w.write("a").then(() => w.write("b")).then(() => {
            globalThis.__out = written.join(",");
        });
        "#);
    assert_eq!(out, "a,b");
}

/// `close()` runs the sink's `close` algorithm and resolves.
#[test]
fn close_runs_sink_close() {
    let out = run(r#"
        globalThis.__out = "pending";
        let closed = false;
        const ws = new WritableStream({ close() { closed = true; } });
        const w = ws.getWriter();
        w.write("x");
        w.close().then(() => { globalThis.__out = "closed:" + closed; });
        "#);
    assert_eq!(out, "closed:true");
}

/// `abort(reason)` runs the sink's `abort` algorithm with the reason and resolves.
#[test]
fn abort_runs_sink_abort() {
    let out = run(r#"
        globalThis.__out = "pending";
        let abortReason;
        const ws = new WritableStream({ abort(r) { abortReason = r; } });
        const w = ws.getWriter();
        w.abort("boom").then(() => { globalThis.__out = "aborted:" + abortReason; });
        "#);
    assert_eq!(out, "aborted:boom");
}

/// `locked` reflects writer acquisition, and a second `getWriter` throws.
#[test]
fn locked_and_double_get_writer() {
    let out = run(r#"
        const ws = new WritableStream();
        const before = ws.locked;
        ws.getWriter();
        const after = ws.locked;
        let threw = false;
        try { ws.getWriter(); } catch (e) { threw = e instanceof TypeError; }
        globalThis.__out = `${before},${after},${threw}`;
        "#);
    assert_eq!(out, "false,true,true");
}

/// A sink whose `write` rejects errors the stream: the write promise rejects and
/// the writer's `closed` promise rejects.
#[test]
fn write_rejection_errors_stream() {
    let out = run(r#"
        globalThis.__out = "pending";
        const ws = new WritableStream({ write() { return Promise.reject(new Error("nope")); } });
        const w = ws.getWriter();
        w.write("a").then(
            () => { globalThis.__out = "unexpected-fulfill"; },
            (e) => { globalThis.__out = "rejected:" + e.message; },
        );
        "#);
    assert_eq!(out, "rejected:nope");
}
