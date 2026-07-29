// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Keystone integration tests for the readable default path.
//!
//! These validate the three things the implementation's design rests on, on a
//! minimal end-to-end slice: native promise reactions created via
//! `Function::new_callback` (the start/pull reactions), the `this`-binding of
//! the underlying source's start/pull callbacks, and read-request promise
//! resolution. They run JS, drain the microtask queue, and read back a result
//! stashed on `globalThis`.
//!
//! Behaviour here is covered far more thoroughly by the `streams/readable-streams`
//! WPT suites; this is the fast, design-validating smoke test.

// This file contains nothing platform-specific, so skip it on wasm32.
#![cfg(not(target_arch = "wasm32"))]

use core_runtime::config::RuntimeConfig;
use core_runtime::event_loop::run_microtasks;
use core_runtime::runtime::{clear_global_initializers, register_global_initializer, Runtime};
use js::conversion::FromJSVal;
use js::error::ExnThrown;

/// Evaluate `code`, drain microtasks, and return `String(globalThis.__out)`.
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

/// Bound functions are accepted as WebIDL callback-type members: a bound `pull`
/// and a bound queuing-strategy `size` are both `IsCallable` even though a bound
/// function is not a `JSFunction`. The dictionary conversion must not reject
/// them, and they must be invoked with their bound `this`.
#[test]
fn bound_function_callbacks() {
    let out = run(r#"
        globalThis.__out = "pending";
        let sizeCalledWith = null;
        // `start` enqueues before any reader exists, so the chunk goes through the
        // queue and the strategy `size` is actually invoked (an enqueue that
        // satisfies a pending read bypasses `size`).
        const src = { first: 7, start(c) { c.enqueue(this.first); } };
        const strat = {
            factor: 3,
            size(chunk) { sizeCalledWith = chunk; return chunk * this.factor; },
        };
        const rs = new ReadableStream(
            { start: src.start.bind(src) },
            { size: strat.size.bind(strat), highWaterMark: 100 },
        );
        rs.getReader().read().then(r => {
            globalThis.__out = `${r.value},${r.done},size=${sizeCalledWith}`;
        });
        "#);
    // Bound `start` enqueues this.first (7); bound `size` is invoked with that chunk.
    assert_eq!(out, "7,false,size=7");
}

/// A promise-returning operation invoked with a wrong-`this` (the prototype, or
/// `undefined`) must *reject* with a `TypeError`, not throw synchronously —
/// WebIDL §3.7.7. This drives the macro's `ResultPromise` brand-check reject
/// branch (a rejected promise created from the pending brand-check exception), a
/// distinct path from the argument-error reject branch; under GC zeal it
/// exercises that branch directly.
#[test]
fn cancel_with_wrong_this_rejects() {
    let out = run(r#"
        globalThis.__out = "pending";
        let threw = false;
        let p;
        try { p = ReadableStream.prototype.cancel.call(undefined); }
        catch (e) { threw = true; }
        Promise.resolve(p).then(
            () => { globalThis.__out = `threw=${threw},resolved`; },
            e => { globalThis.__out = `threw=${threw},rejected:${e.constructor.name}`; },
        );
        "#);
    assert_eq!(out, "threw=false,rejected:TypeError");
}

/// A promise-typed attribute getter (`closed`) invoked with a wrong-`this`
/// rejects with a `TypeError`, not throws — WebIDL §3.7.7 ("Attributes"). This
/// drives the accessor trampoline's promise-getter reject branch, distinct from
/// the operation path; under GC zeal it exercises that branch directly.
#[test]
fn closed_getter_with_wrong_this_rejects() {
    let out = run(r#"
        globalThis.__out = "pending";
        const getter = Object.getOwnPropertyDescriptor(
            ReadableStreamDefaultReader.prototype, "closed").get;
        let threw = false;
        let p;
        try { p = getter.call(undefined); } catch (e) { threw = true; }
        Promise.resolve(p).then(
            () => { globalThis.__out = `threw=${threw},resolved`; },
            e => { globalThis.__out = `threw=${threw},rejected:${e.constructor.name}`; },
        );
        "#);
    assert_eq!(out, "threw=false,rejected:TypeError");
}

/// A chunk enqueued during `start` is delivered to a `read()`, exercising the
/// start reaction (native `new_callback`), the `start` callback's `this`
/// binding, and read-request resolution.
#[test]
fn start_enqueue_then_read() {
    let out = run(r#"
        globalThis.__out = "pending";
        const rs = new ReadableStream({ start(c) { c.enqueue("hello"); } });
        const reader = rs.getReader();
        reader.read().then(r => { globalThis.__out = `${r.value},${r.done}`; });
        "#);
    assert_eq!(out, "hello,false");
}

/// A `pull`-driven source: the read request is queued first, then satisfied
/// once `start` settles and `CallPullIfNeeded` invokes `pull` (which enqueues
/// and closes). Exercises the pull reaction and the `pull` callback's `this`.
#[test]
fn pull_then_read_and_close() {
    let out = run(r#"
        globalThis.__out = "pending";
        const rs = new ReadableStream({
            pull(c) { c.enqueue("p"); c.close(); },
        });
        const reader = rs.getReader();
        reader.read().then(r => { globalThis.__out = `${r.value},${r.done}`; });
        "#);
    assert_eq!(out, "p,false");
}

/// Reading a closed stream resolves with `{ value: undefined, done: true }`.
#[test]
fn read_after_close_is_done() {
    let out = run(r#"
        globalThis.__out = "pending";
        const rs = new ReadableStream({ start(c) { c.close(); } });
        rs.getReader().read().then(r => { globalThis.__out = `${r.value},${r.done}`; });
        "#);
    assert_eq!(out, "undefined,true");
}

/// `locked` reflects reader acquisition, and a second `getReader` throws.
#[test]
fn locked_and_double_get_reader() {
    let out = run(r#"
        const rs = new ReadableStream();
        const before = rs.locked;
        rs.getReader();
        const after = rs.locked;
        let threw = false;
        try { rs.getReader(); } catch (e) { threw = e instanceof TypeError; }
        globalThis.__out = `${before},${after},${threw}`;
        "#);
    assert_eq!(out, "false,true,true");
}

/// A `read()` result is an ordinary object with *own* `value`/`done` data
/// properties (the spec's `CreateIterResultObject`), so a hostile accessor
/// installed on `Object.prototype` cannot intercept or replace them. Building the
/// result with `[[Set]]` would walk the prototype chain and let the accessor
/// observe the chunk and prevent the own property from being created.
#[test]
fn read_result_has_own_properties_immune_to_proto_accessor() {
    let out = run(r#"
        globalThis.__out = "pending";
        let leaked = "none";
        Object.defineProperty(Object.prototype, "value", {
            configurable: true,
            get() { return "HIJACKED"; },
            set(v) { leaked = v; },
        });
        const rs = new ReadableStream({ start(c) { c.enqueue(42); } });
        rs.getReader().read().then(r => {
            globalThis.__out =
                `own=${Object.hasOwn(r, "value")},value=${r.value},leaked=${leaked}`;
        });
        "#);
    assert_eq!(out, "own=true,value=42,leaked=none");
}
