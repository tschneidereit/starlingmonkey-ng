// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Keystone integration tests for `ReadableStream.prototype.pipeTo` and
//! `pipeThrough`: chunks flow from a source to a destination, the result
//! promise settles, `preventClose` leaves the destination open, source errors
//! propagate forward (aborting the destination and rejecting the promise), and
//! `pipeThrough` returns the transform's readable side. Covered far more
//! thoroughly by `streams/piping/*.any.js`; this is the fast,
//! design-validating (and GC-rooting-validating) smoke test.

// This file contains nothing platform-specific, so skip it on wasm32.
#![cfg(not(target_arch = "wasm32"))]

use core_runtime::config::RuntimeConfig;
use core_runtime::event_loop::run_microtasks;
use core_runtime::runtime::{clear_global_initializers, register_global_initializer, Runtime};
use js::conversion::FromJSVal;
use js::error::ExnThrown;

fn run(code: &str) -> String {
    clear_global_initializers();
    register_global_initializer(|scope, global| web_globals::add_to_global(scope, global));
    register_global_initializer(|scope, global| web_streams::add_to_global(scope, global));
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

/// A WritableStream that records written chunks and whether it was closed.
const RECORDING: &str = r#"
    function recordingWritable() {
        const written = [];
        const state = { closed: false, aborted: false };
        const ws = new WritableStream({
            write(chunk) { written.push(chunk); },
            close() { state.closed = true; },
            abort(reason) { state.aborted = true; state.abortReason = reason; },
        });
        return { ws, written, state };
    }
"#;

/// Every chunk flows from the source to the destination, the destination is
/// closed, and the pipe promise fulfills.
#[test]
fn pipes_all_chunks_and_closes_dest() {
    let out = run(&format!(
        r#"{RECORDING}
        globalThis.__out = "pending";
        const {{ ws, written, state }} = recordingWritable();
        const rs = new ReadableStream({{ start(c) {{ c.enqueue("a"); c.enqueue("b"); c.enqueue("c"); c.close(); }} }});
        rs.pipeTo(ws).then(() => {{
            globalThis.__out = written.join("") + "|closed:" + state.closed;
        }}, e => {{ globalThis.__out = "rejected:" + e; }});
        "#
    ));
    assert_eq!(out, "abc|closed:true");
}

/// With `preventClose: true`, the destination is left open after the source
/// closes (the pipe promise still fulfills).
#[test]
fn prevent_close_leaves_dest_open() {
    let out = run(&format!(
        r#"{RECORDING}
        globalThis.__out = "pending";
        const {{ ws, written, state }} = recordingWritable();
        const rs = new ReadableStream({{ start(c) {{ c.enqueue("x"); c.close(); }} }});
        rs.pipeTo(ws, {{ preventClose: true }}).then(() => {{
            globalThis.__out = written.join("") + "|closed:" + state.closed;
        }}, e => {{ globalThis.__out = "rejected:" + e; }});
        "#
    ));
    assert_eq!(out, "x|closed:false");
}

/// A source error propagates forward: the destination is aborted and the pipe
/// promise rejects with the source's error.
#[test]
fn source_error_aborts_dest_and_rejects() {
    let out = run(&format!(
        r#"{RECORDING}
        globalThis.__out = "pending";
        const {{ ws, written, state }} = recordingWritable();
        let ctrl;
        const rs = new ReadableStream({{ start(c) {{ ctrl = c; }} }});
        const p = rs.pipeTo(ws);
        ctrl.error(new Error("boom"));
        p.then(() => {{ globalThis.__out = "fulfilled"; }},
               e => {{ globalThis.__out = "rejected:" + e.message + "|aborted:" + state.aborted; }});
        "#
    ));
    assert_eq!(out, "rejected:boom|aborted:true");
}

/// `pipeThrough` returns the transform's readable side, and chunks piped into
/// the writable side flow through (an identity transform) to it.
#[test]
fn pipe_through_identity_transform() {
    let out = run(&format!(
        r#"{RECORDING}
        globalThis.__out = "pending";
        async function drain(stream) {{
            const reader = stream.getReader();
            const acc = [];
            for (;;) {{
                const {{ value, done }} = await reader.read();
                if (done) break;
                acc.push(value);
            }}
            return acc.join("");
        }}
        const rs = new ReadableStream({{ start(c) {{ c.enqueue("p"); c.enqueue("q"); c.close(); }} }});
        const ts = new TransformStream();
        const readable = rs.pipeThrough(ts);
        const isReadable = readable instanceof ReadableStream;
        drain(readable).then(s => {{ globalThis.__out = s + "|" + isReadable; }});
        "#
    ));
    assert_eq!(out, "pq|true");
}

/// Aborting the pipe's signal mid-flight rejects the pipe promise with the abort
/// reason, cancels the source, and aborts the destination. This exercises the
/// abort algorithm, the wait-for-all composite action, and the deferred-shutdown
/// state slots — the most allocation-heavy path, validated under compacting GC.
#[test]
fn abort_signal_rejects_and_cancels_both_sides() {
    let out = run(&format!(
        r#"{RECORDING}
        globalThis.__out = "pending";
        const {{ ws, written, state }} = recordingWritable();
        const ac = new AbortController();
        let cancelled = false;
        const rs = new ReadableStream({{
            start(c) {{ c.enqueue("a"); }},
            cancel() {{ cancelled = true; }},
        }});
        const p = rs.pipeTo(ws, {{ signal: ac.signal }});
        ac.abort(new Error("stop"));
        p.then(() => {{ globalThis.__out = "fulfilled"; }},
               e => {{ globalThis.__out = "rejected:" + e.message + "|cancelled:" + cancelled + "|aborted:" + state.aborted; }});
        "#
    ));
    assert_eq!(out, "rejected:stop|cancelled:true|aborted:true");
}

/// pipeTo's abort handling is a DOM abort algorithm, not an `abort` event
/// listener: an author `abort` listener registered earlier that calls
/// `stopImmediatePropagation()` must not suppress the pipe's shutdown. The pipe
/// still rejects with the abort reason.
#[test]
fn abort_signal_not_suppressed_by_stop_immediate_propagation() {
    let out = run(&format!(
        r#"{RECORDING}
        globalThis.__out = "pending";
        const {{ ws, state }} = recordingWritable();
        const ac = new AbortController();
        ac.signal.addEventListener("abort", (e) => {{ e.stopImmediatePropagation(); }});
        const rs = new ReadableStream({{ start(c) {{ c.enqueue("a"); }} }});
        const p = rs.pipeTo(ws, {{ signal: ac.signal }});
        ac.abort(new Error("stop"));
        p.then(() => {{ globalThis.__out = "fulfilled"; }},
               e => {{ globalThis.__out = "rejected:" + e.message + "|aborted:" + state.aborted; }});
        "#
    ));
    assert_eq!(out, "rejected:stop|aborted:true");
}
