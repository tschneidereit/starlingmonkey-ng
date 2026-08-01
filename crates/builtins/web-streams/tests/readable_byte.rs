// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Keystone integration tests for the readable byte stream path (default
//! reader, no auto-allocation).
//!
//! These validate the parts the byte controller's design rests on, on a
//! minimal end-to-end slice: the queue of `ArrayBuffer`-backed entries, buffer
//! transfer on `enqueue`, constructing a `Uint8Array` view over a transferred
//! region, the byte controller's own start/pull reactions, and the
//! default-reader `[[PullSteps]]` dispatch. They run JS, drain the microtask
//! queue, and read back a result stashed on `globalThis`.
//!
//! Behaviour here is covered far more thoroughly by the
//! `streams/readable-byte-streams` WPT suites; this is the fast,
//! design-validating smoke test (and the GC-rooting keystone under zeal).

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

/// A byte chunk enqueued during `start` is queued, then delivered to a `read()`
/// as a fresh `Uint8Array` over the transferred buffer. Exercises the queue,
/// `TransferArrayBuffer`, the `Uint8Array`-over-region construction, and
/// `FillReadRequestFromQueue`.
#[test]
fn enqueue_in_start_then_read() {
    let out = run(r#"
        globalThis.__out = "pending";
        const rs = new ReadableStream({
            type: "bytes",
            start(c) { c.enqueue(new Uint8Array([1, 2, 3])); },
        });
        rs.getReader().read().then(r => {
            globalThis.__out = `${r.value.constructor.name},${Array.from(r.value).join("-")},${r.done}`;
        });
        "#);
    assert_eq!(out, "Uint8Array,1-2-3,false");
}

/// A `pull`-driven byte source: the read request is queued first, then
/// satisfied once `start` settles and `CallPullIfNeeded` invokes `pull` (which
/// enqueues directly into the waiting read request). Exercises the byte pull
/// reaction and `ProcessReadRequestsUsingQueue`/fulfill-via-`Construct`.
#[test]
fn pull_driven_byte_read() {
    let out = run(r#"
        globalThis.__out = "pending";
        let pulls = 0;
        const rs = new ReadableStream({
            type: "bytes",
            pull(c) { pulls++; c.enqueue(new Uint8Array([42])); },
        });
        rs.getReader().read().then(r => {
            globalThis.__out = `${Array.from(r.value).join("-")},pulls=${pulls}`;
        });
        "#);
    assert_eq!(out, "42,pulls=1");
}

/// Closing after enqueuing drains the queue before delivering `done`. The first
/// read gets the chunk; the second sees the close.
#[test]
fn enqueue_then_close_drains_then_done() {
    let out = run(r#"
        globalThis.__out = "pending";
        const rs = new ReadableStream({
            type: "bytes",
            start(c) { c.enqueue(new Uint8Array([9])); c.close(); },
        });
        const reader = rs.getReader();
        reader.read().then(r1 => {
            const first = `${Array.from(r1.value).join("-")},${r1.done}`;
            reader.read().then(r2 => {
                globalThis.__out = `${first}|${r2.value},${r2.done}`;
            });
        });
        "#);
    assert_eq!(out, "9,false|undefined,true");
}

/// `cancel()` runs the underlying byte source's cancel algorithm with the
/// reason, exercising the byte controller's `[[CancelSteps]]`.
#[test]
fn cancel_runs_byte_cancel_steps() {
    let out = run(r#"
        globalThis.__out = "pending";
        let reason = null;
        const rs = new ReadableStream({ type: "bytes", cancel(r) { reason = r; } });
        rs.cancel("stop").then(() => { globalThis.__out = `cancelled:${reason}`; });
        "#);
    assert_eq!(out, "cancelled:stop");
}

/// `desiredSize` reflects the strategy high-water mark minus the queued bytes,
/// and `byobRequest` is null with no pending pull-into. Exercises the two
/// getters and `GetDesiredSize`.
#[test]
fn desired_size_and_byob_request_getters() {
    let out = run(r#"
        let ds0, dsAfter, br;
        const rs = new ReadableStream({
            type: "bytes",
            start(c) {
                ds0 = c.desiredSize;
                c.enqueue(new Uint8Array([1, 2, 3, 4]));
                dsAfter = c.desiredSize;
                br = c.byobRequest;
            },
        }, { highWaterMark: 10 });
        globalThis.__out = `${ds0},${dsAfter},${br}`;
        "#);
    assert_eq!(out, "10,6,null");
}

/// Enqueuing a zero-length view or a non-`ArrayBufferView` throws a
/// `TypeError`; a byte source with a positive `autoAllocateChunkSize`
/// constructs fine, while `autoAllocateChunkSize: 0` throws a `TypeError`.
#[test]
fn enqueue_validation_and_auto_allocate() {
    let out = run(r#"
        let zeroThrew = false, nonViewThrew = false, auto16Ok = false, auto0Threw = false;
        const rs = new ReadableStream({
            type: "bytes",
            start(c) {
                try { c.enqueue(new Uint8Array(0)); } catch (e) { zeroThrew = e instanceof TypeError; }
                try { c.enqueue([1]); } catch (e) { nonViewThrew = e instanceof TypeError; }
            },
        });
        try { new ReadableStream({ type: "bytes", autoAllocateChunkSize: 16 }); auto16Ok = true; }
        catch (e) {}
        try { new ReadableStream({ type: "bytes", autoAllocateChunkSize: 0 }); }
        catch (e) { auto0Threw = e instanceof TypeError; }
        globalThis.__out = `${zeroThrew},${nonViewThrew},${auto16Ok},${auto0Threw}`;
        "#);
    assert_eq!(out, "true,true,true,true");
}

/// For a byte stream, a *callable* `size` is rejected with a `RangeError` (a
/// byte stream's strategy must not have a size function), but a *non-callable*
/// `size` is rejected earlier with a `TypeError` — the `QueuingStrategySize`
/// callback conversion fails before the byte-stream RangeError. Not covered by
/// the enabled WPT (`general.any.js` only exercises callable size functions).
#[test]
fn byte_stream_size_function_error_ordering() {
    let out = run(r#"
        let callableErr = "none", nonCallableErr = "none";
        try { new ReadableStream({ type: "bytes" }, { size() { return 1; } }); }
        catch (e) { callableErr = e.constructor.name; }
        try { new ReadableStream({ type: "bytes" }, { size: {} }); }
        catch (e) { nonCallableErr = e.constructor.name; }
        globalThis.__out = `${callableErr},${nonCallableErr}`;
        "#);
    assert_eq!(out, "RangeError,TypeError");
}

/// A non-callable underlying-source callback throws a `TypeError` at dictionary
/// conversion, before the byte-stream + size-function `RangeError`.
#[test]
fn non_callable_callback_throws_type_error_before_byte_size_check() {
    let out = run(r#"
        let err = "none";
        try { new ReadableStream({ pull: {}, type: "bytes" }, { size: () => 1 }); err = "no-throw"; }
        catch (e) { err = e.constructor.name; }
        globalThis.__out = err;
        "#);
    assert_eq!(out, "TypeError");
}
