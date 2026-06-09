// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Keystone integration tests for `ReadableStream.tee()`: both branches receive
//! the source's chunks, branch cancellation is independent, and the source's
//! error propagates to both branches. Covered far more thoroughly by
//! `streams/readable-streams/tee.any.js`; this is the fast, design-validating
//! (and GC-rooting-validating) smoke test.

use core_runtime::config::RuntimeConfig;
use core_runtime::event_loop::run_microtasks;
use core_runtime::runtime::{clear_global_initializers, register_global_initializer, Runtime};
use js::conversion::FromJSVal;
use js::error::ExnThrown;

fn run(code: &str) -> String {
    clear_global_initializers();
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

/// Both branches receive all of the source's chunks.
#[test]
fn both_branches_receive_chunks() {
    let out = run(&format!(
        r#"{DRAIN}
        globalThis.__out = "pending";
        const rs = new ReadableStream({{ start(c) {{ c.enqueue("a"); c.enqueue("b"); c.close(); }} }});
        const [b1, b2] = rs.tee();
        Promise.all([drain(b1), drain(b2)]).then(([s1, s2]) => {{ globalThis.__out = s1 + "|" + s2; }});
        "#
    ));
    assert_eq!(out, "ab|ab");
}

/// `tee()` locks the source and returns two readable branches.
#[test]
fn tee_locks_and_returns_two_branches() {
    let out = run(r#"
        const rs = new ReadableStream();
        const branches = rs.tee();
        const locked = rs.locked;
        const two = branches.length === 2;
        const both = branches[0] instanceof ReadableStream && branches[1] instanceof ReadableStream;
        globalThis.__out = `${locked},${two},${both}`;
        "#);
    assert_eq!(out, "true,true,true");
}

/// Cancelling one branch does not stop the other from draining.
#[test]
fn cancel_one_branch_other_still_drains() {
    let out = run(&format!(
        r#"{DRAIN}
        globalThis.__out = "pending";
        const rs = new ReadableStream({{ start(c) {{ c.enqueue("x"); c.enqueue("y"); c.close(); }} }});
        const [b1, b2] = rs.tee();
        b1.cancel();
        drain(b2).then(s => {{ globalThis.__out = s; }});
        "#
    ));
    assert_eq!(out, "xy");
}

/// An error in the source propagates to both branches' readers.
#[test]
fn source_error_propagates_to_both() {
    let out = run(&format!(
        r#"{DRAIN}
        globalThis.__out = "pending";
        let ctrl;
        const rs = new ReadableStream({{ start(c) {{ ctrl = c; }} }});
        const [b1, b2] = rs.tee();
        ctrl.error(new Error("boom"));
        const r1 = b1.getReader();
        const r2 = b2.getReader();
        Promise.all([
            r1.read().then(() => "ok", e => "e1:" + e.message),
            r2.read().then(() => "ok", e => "e2:" + e.message),
        ]).then(([a, b]) => {{ globalThis.__out = a + "," + b; }});
        "#
    ));
    assert_eq!(out, "e1:boom,e2:boom");
}
