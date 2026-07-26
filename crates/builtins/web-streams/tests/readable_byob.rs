// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Keystone integration tests for the BYOB reader / read-into path: pull-into
//! descriptors, `byobRequest.respond` / `respondWithNewView`, and
//! `autoAllocateChunkSize`.
//!
//! These exercise the parts the M2 design rests on: a `read(view)` transfers the
//! caller's buffer into a pull-into descriptor, the source fills it through
//! `byobRequest.view` and commits with `respond`, and the filled region comes
//! back as a view of the requested type. Auto-allocation routes a default
//! `read()` through the same machinery. They run JS, drain microtasks, and read
//! back a result stashed on `globalThis`.
//!
//! WPT `streams/readable-byte-streams/*` covers this far more thoroughly; this is
//! the fast, design-validating smoke test (and the GC-rooting keystone under
//! zeal, since pull-into transfers buffers and creates views).

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

/// A BYOB `read(view)`: the source fills the provided view through
/// `byobRequest.view` and commits with `respond`, and the filled region is
/// returned as a `Uint8Array`. Exercises the pull-into descriptor, buffer
/// transfer, and `ReadableByteStreamControllerRespond`.
#[test]
fn byob_read_respond() {
    let out = run(r#"
        globalThis.__out = "pending";
        const rs = new ReadableStream({
            type: "bytes",
            pull(c) {
                const v = c.byobRequest.view;
                v[0] = 11; v[1] = 22; v[2] = 33;
                c.byobRequest.respond(3);
            },
        });
        rs.getReader({ mode: "byob" }).read(new Uint8Array(3)).then(r => {
            globalThis.__out = `${r.value.constructor.name},${Array.from(r.value).join("-")},${r.done}`;
        });
        "#);
    assert_eq!(out, "Uint8Array,11-22-33,false");
}

/// `respondWithNewView`: the source writes into a fresh view over the request's
/// buffer region and commits it. Exercises
/// `ReadableByteStreamControllerRespondWithNewView`.
#[test]
fn byob_respond_with_new_view() {
    let out = run(r#"
        globalThis.__out = "pending";
        const rs = new ReadableStream({
            type: "bytes",
            pull(c) {
                const req = c.byobRequest;
                const nv = new Uint8Array(req.view.buffer, req.view.byteOffset, 2);
                nv[0] = 99; nv[1] = 88;
                req.respondWithNewView(nv);
            },
        });
        rs.getReader({ mode: "byob" }).read(new Uint8Array(4)).then(r => {
            globalThis.__out = `${Array.from(r.value).join("-")},len=${r.value.length}`;
        });
        "#);
    assert_eq!(out, "99-88,len=2");
}

/// `autoAllocateChunkSize`: a default `read()` on a byte stream routes through a
/// pull-into descriptor with an auto-allocated buffer; the source fills it via
/// `byobRequest` and the bytes come back as a `Uint8Array`. Exercises PullSteps
/// step 5 and the read-into-to-default-reader commit.
#[test]
fn auto_allocate_default_read() {
    let out = run(r#"
        globalThis.__out = "pending";
        const rs = new ReadableStream({
            type: "bytes",
            autoAllocateChunkSize: 16,
            pull(c) {
                const v = c.byobRequest.view;
                v[0] = 7; v[1] = 8;
                c.byobRequest.respond(2);
            },
        });
        rs.getReader().read().then(r => {
            globalThis.__out = `${r.value.constructor.name},len=${r.value.length},${r.value[0]}-${r.value[1]}`;
        });
        "#);
    assert_eq!(out, "Uint8Array,len=2,7-8");
}

/// A typed-array view with `min` honours the element alignment: a `read(view, {
/// min })` on a `Uint16Array` is fulfilled only once at least `min` elements are
/// available. Here the source supplies exactly the requested bytes.
#[test]
fn byob_read_typed_array_min() {
    let out = run(r#"
        globalThis.__out = "pending";
        const rs = new ReadableStream({
            type: "bytes",
            pull(c) {
                const v = c.byobRequest.view;
                for (let i = 0; i < v.byteLength; i++) v[i] = i + 1;
                c.byobRequest.respond(v.byteLength);
            },
        });
        rs.getReader({ mode: "byob" }).read(new Uint16Array(2), { min: 2 }).then(r => {
            globalThis.__out = `${r.value.constructor.name},len=${r.value.length},${r.value[0]}`;
        });
        "#);
    // Bytes 1,2,3,4 little-endian -> u16 [0x0201, 0x0403] = [513, 1027].
    assert_eq!(out, "Uint16Array,len=2,513");
}

/// A BYOB `read()` with no view argument rejects (it does not throw
/// synchronously): a promise-returning WebIDL operation surfaces an
/// argument-conversion failure as a rejected promise. This drives the macro's
/// `ResultPromise` rejection branch (`new_rejected_with_pending_error` with a
/// live pending exception), so under GC zeal it exercises that path directly.
#[test]
fn byob_read_without_view_rejects() {
    let out = run(r#"
        globalThis.__out = "pending";
        const reader = new ReadableStream({ type: "bytes" }).getReader({ mode: "byob" });
        let threw = false;
        let p;
        try { p = reader.read(); } catch (e) { threw = true; }
        Promise.resolve(p).then(
            () => { globalThis.__out = `threw=${threw},resolved`; },
            e => { globalThis.__out = `threw=${threw},rejected:${e.constructor.name}`; },
        );
        "#);
    assert_eq!(out, "threw=false,rejected:TypeError");
}

/// Teeing a byte stream and reading both branches: `ReadableByteStreamTee`
/// acquires a default reader, clones each chunk's buffer for the second branch,
/// and pushes into both branches' byte controllers. This is the
/// allocation-heavy native-callback + GC-traced-state-object path; the buffer
/// clone and the dual enqueue make it the byte-tee GC-rooting keystone.
#[test]
fn byte_tee_reads_both_branches() {
    let out = run(r#"
        globalThis.__out = "pending";
        const rs = new ReadableStream({
            type: "bytes",
            start(c) { c.enqueue(new Uint8Array([5, 6, 7])); },
        });
        const [a, b] = rs.tee();
        Promise.all([a.getReader().read(), b.getReader().read()]).then(([ra, rb]) => {
            globalThis.__out =
                `${Array.from(ra.value).join("-")}|${Array.from(rb.value).join("-")}|` +
                `${ra.value.buffer !== rb.value.buffer}`;
        });
        "#);
    // Both branches see the same bytes, but over distinct (cloned) buffers.
    assert_eq!(out, "5-6-7|5-6-7|true");
}

/// Three un-awaited BYOB reads leave three pending pull-into descriptors; a
/// single `enqueue` fills the head while the other two are still queued. The
/// fill loop calls `ReadableByteStreamControllerFillHeadPullIntoDescriptor`
/// after popping the head into a stack local, so the descriptor deque still
/// holds the two trailing descriptors at that point — a regression test that
/// fill_head does not assume at most one pending pull-into remains.
#[test]
fn byob_multiple_pending_reads_then_enqueue() {
    let out = run(r#"
        globalThis.__out = "pending";
        let controller;
        const rs = new ReadableStream({
            type: "bytes",
            start(c) { controller = c; },
        });
        const reader = rs.getReader({ mode: "byob" });
        const p1 = reader.read(new Uint8Array(10));
        reader.read(new Uint8Array(10));
        reader.read(new Uint8Array(10));
        controller.enqueue(new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]));
        p1.then(r => {
            globalThis.__out = `${r.value.constructor.name},len=${r.value.length},${r.value[0]}-${r.value[9]}`;
        });
        "#);
    // The first read is fulfilled with all 10 bytes; the other two stay pending.
    assert_eq!(out, "Uint8Array,len=10,1-10");
}

/// A `Float16Array` BYOB view round-trips with its element kind intact: the
/// committed chunk comes back as a `Float16Array` (element size 2), not a
/// `DataView`, and the `min` option is validated against the view's element
/// length rather than its byte length.
#[test]
fn byob_read_float16_view_kind() {
    let out = run(r#"
        globalThis.__out = "pending";
        if (typeof Float16Array === "undefined") {
            globalThis.__out = "no-float16";
        } else {
            const rs = new ReadableStream({
                type: "bytes",
                pull(c) {
                    const v = c.byobRequest.view;
                    c.byobRequest.respond(v.byteLength);
                },
            });
            // `min` of 3 exceeds the 2-element view; the spec validates against the
            // element length, so this rejects with a RangeError before any pull.
            rs.getReader({ mode: "byob" }).read(new Float16Array(2), { min: 3 }).then(
                r => { globalThis.__out = `resolved:${r.value.constructor.name}`; },
                e => { globalThis.__out = `rejected:${e.constructor.name}`; },
            );
        }
        "#);
    assert_eq!(out, "rejected:RangeError");
}

/// Teeing a byte stream and doing a multi-byte-element BYOB read on a branch can
/// leave that branch's pending pull-into partially filled (e.g. 3 of 4 bytes for
/// an `Int32Array`). If the source then closes, the branch's
/// `ReadableByteStreamControllerClose` hits its "insufficient bytes" `TypeError`.
/// The spec marks that close `!` (infallible), but the partial fill makes it
/// reachable, so the branch read must reject with a `TypeError` rather than
/// aborting the runtime.
#[test]
fn byte_tee_branch_partial_fill_close_rejects_not_aborts() {
    let out = run(r#"
        globalThis.__out = "pending";
        let step = 0;
        const rs = new ReadableStream({
            type: "bytes",
            pull(c) {
                if (step === 0) {
                    step = 1;
                    const v = c.byobRequest.view;
                    v[0] = 1; v[1] = 2; v[2] = 3;
                    c.byobRequest.respond(3); // 3 of 4 bytes -> branch partial fill
                } else {
                    c.close();
                    c.byobRequest.respond(0);
                }
            },
        });
        const [b1, b2] = rs.tee();
        // Draining branch2 to completion discriminates the clean error path from
        // the (dev-mode-masked) abort: branch2 receives the 3-byte clone, then its
        // next read must reject (the source closed and the misaligned branch1 close
        // errored the tee). Aborting at branch1's close instead leaves branch2
        // unclosed, so that second read would hang forever.
        async function drainB2() {
            const r = b2.getReader();
            let n = 0;
            try {
                for (;;) {
                    const { done } = await r.read();
                    if (done) return "b2:done@" + n;
                    n++;
                }
            } catch (e) { return "b2:" + e.constructor.name + "@" + n; }
        }
        const r1 = b1.getReader({ mode: "byob" }).read(new Int32Array(1)).then(
            () => "b1:resolved", e => "b1:" + e.constructor.name);
        Promise.all([r1, drainB2()]).then(([a, b]) => { globalThis.__out = a + "," + b; });
        "#);
    assert_eq!(out, "b1:TypeError,b2:TypeError@1");
}

/// Reading a byte stream's BYOB reader after `close()` resolves with an empty
/// view and `done: true`.
#[test]
fn byob_read_after_close_is_done() {
    let out = run(r#"
        globalThis.__out = "pending";
        const rs = new ReadableStream({
            type: "bytes",
            start(c) { c.close(); },
        });
        rs.getReader({ mode: "byob" }).read(new Uint8Array(8)).then(r => {
            globalThis.__out = `${r.value.constructor.name},len=${r.value.length},${r.done}`;
        });
        "#);
    assert_eq!(out, "Uint8Array,len=0,true");
}

/// `respond(bytesWritten)` is `[EnforceRange] unsigned long long`: a negative
/// argument throws a `TypeError` at argument conversion, not a `RangeError` from
/// the length checks (which is what a wrapping conversion produces).
#[test]
fn respond_enforce_range_rejects_negative() {
    let out = run(r#"
        globalThis.__out = "pending";
        const rs = new ReadableStream({
            type: "bytes",
            pull(c) {
                try { c.byobRequest.respond(-1); globalThis.__out = "no-throw"; }
                catch (e) { globalThis.__out = "respond:" + e.constructor.name; }
            },
        });
        rs.getReader({ mode: "byob" }).read(new Uint8Array(4));
        "#);
    assert_eq!(out, "respond:TypeError");
}

/// WebIDL converts arguments left to right: `read(view, options)` must coerce
/// `view` to an `ArrayBufferView` (rejecting a non-view with a `TypeError`) before
/// the `options` dictionary is converted, so an author getter on `options.min` is
/// never invoked when the view is invalid.
#[test]
fn byob_read_validates_view_before_options() {
    let out = run(r#"
        globalThis.__out = "pending";
        let getterRan = false;
        const reader = new ReadableStream({ type: "bytes" }).getReader({ mode: "byob" });
        reader.read(42, { get min() { getterRan = true; throw new Error("boom"); } }).then(
            () => { globalThis.__out = "resolved"; },
            e => { globalThis.__out = `rejected:${e.constructor.name}|getterRan:${getterRan}`; },
        );
        "#);
    assert_eq!(out, "rejected:TypeError|getterRan:false");
}

/// `autoAllocateChunkSize` is `[EnforceRange] unsigned long long`: a negative
/// value throws a `TypeError` at construction, rather than wrapping to a huge
/// positive value and constructing successfully.
#[test]
fn auto_allocate_chunk_size_enforce_range() {
    let out = run(r#"
        let r;
        try { new ReadableStream({ type: "bytes", autoAllocateChunkSize: -1 }); r = "no-throw"; }
        catch (e) { r = e.constructor.name; }
        globalThis.__out = r;
        "#);
    assert_eq!(out, "TypeError");
}
