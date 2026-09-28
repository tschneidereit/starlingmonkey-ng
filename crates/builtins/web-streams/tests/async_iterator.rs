// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Keystone integration tests for `ReadableStream` async iteration
//! (`values()` / `[Symbol.asyncIterator]`): `for await` drains a stream, an
//! early `break` cancels the source (unless `preventCancel`), and the iterator's
//! prototype chains to `%AsyncIteratorPrototype%`. Covered far more thoroughly by
//! `streams/readable-streams/async-iterator.any.js`; this is the fast,
//! design-validating (and GC-rooting-validating) smoke test.

// This file contains nothing platform-specific, so skip it on wasm32.
#![cfg(not(target_arch = "wasm32"))]

use core_runtime::runtime::{clear_global_initializers, register_global_initializer};
use core_runtime::test_util::eval_out_with_setup;

fn run(code: &str) -> String {
    eval_out_with_setup(
        || {
            clear_global_initializers();
            register_global_initializer(web_streams::add_to_global);
        },
        code,
    )
}

/// `for await` drains all of the stream's chunks.
#[test]
fn for_await_drains_stream() {
    let out = run(r#"
        globalThis.__out = "pending";
        const rs = new ReadableStream({ start(c) { c.enqueue("a"); c.enqueue("b"); c.enqueue("c"); c.close(); } });
        (async () => {
            let acc = "";
            for await (const chunk of rs) acc += chunk;
            globalThis.__out = acc;
        })();
        "#);
    assert_eq!(out, "abc");
}

/// A promise chunk is adopted, not yielded as-is: the spec's "get the next
/// iteration result" resolves its promise with the raw chunk, so a thenable chunk
/// is unwrapped before the `{ value, done }` result is built. `for await` of a
/// stream that enqueues `Promise.resolve(5)` yields `5`, not the promise object.
#[test]
fn for_await_adopts_thenable_chunk() {
    let out = run(r#"
        globalThis.__out = "pending";
        const rs = new ReadableStream({ start(c) { c.enqueue(Promise.resolve(5)); c.close(); } });
        (async () => {
            const vals = [];
            for await (const x of rs) vals.push(x);
            globalThis.__out = vals.join(",") + "|" + typeof vals[0];
        })();
        "#);
    assert_eq!(out, "5|number");
}

/// A rejected-thenable chunk rejects `next()` and finishes the iterator (the
/// adopted rejection propagates), rather than yielding the rejected promise.
#[test]
fn for_await_rejected_thenable_chunk_throws() {
    let out = run(r#"
        globalThis.__out = "pending";
        const rs = new ReadableStream({ start(c) { c.enqueue(Promise.reject(new TypeError("boom"))); c.close(); } });
        (async () => {
            try {
                for await (const x of rs) { globalThis.__out = "yielded:" + x; return; }
                globalThis.__out = "done-no-throw";
            } catch (e) { globalThis.__out = "threw:" + e.constructor.name + ":" + e.message; }
        })();
        "#);
    assert_eq!(out, "threw:TypeError:boom");
}

/// Breaking out of `for await` cancels the source (the stream becomes unlocked).
#[test]
fn break_cancels_source() {
    let out = run(r#"
        globalThis.__out = "pending";
        let cancelled = false;
        const rs = new ReadableStream({
            start(c) { c.enqueue(1); c.enqueue(2); c.enqueue(3); },
            cancel() { cancelled = true; },
        });
        (async () => {
            for await (const chunk of rs) { if (chunk === 2) break; }
            globalThis.__out = "cancelled:" + cancelled + ",locked:" + rs.locked;
        })();
        "#);
    assert_eq!(out, "cancelled:true,locked:false");
}

/// `values({ preventCancel: true })` leaves the source uncancelled after break.
#[test]
fn prevent_cancel_leaves_source_open() {
    let out = run(r#"
        globalThis.__out = "pending";
        let cancelled = false;
        const rs = new ReadableStream({
            start(c) { c.enqueue(1); c.enqueue(2); },
            cancel() { cancelled = true; },
        });
        (async () => {
            for await (const chunk of rs.values({ preventCancel: true })) { break; }
            globalThis.__out = "cancelled:" + cancelled;
        })();
        "#);
    assert_eq!(out, "cancelled:false");
}

/// The default async iterator is not a WebIDL interface, so it has no interface
/// object: `ReadableStreamAsyncIterator` must not leak onto the global, and the
/// prototype's class string is `"ReadableStream AsyncIterator"` (with a space),
/// per WebIDL §3.7.10.
#[test]
fn async_iterator_has_no_global_and_spaced_string_tag() {
    let out = run(r#"
        const leaked = "ReadableStreamAsyncIterator" in globalThis;
        const tag = Object.prototype.toString.call(new ReadableStream().values());
        globalThis.__out = `leaked=${leaked},tag=${tag}`;
        "#);
    assert_eq!(
        out,
        "leaked=false,tag=[object ReadableStream AsyncIterator]"
    );
}

/// The iterator's prototype chains to `%AsyncIteratorPrototype%` and exposes only
/// `next`/`return`.
#[test]
fn iterator_prototype_shape() {
    let out = run(r#"
        const it = new ReadableStream().values();
        const proto = Object.getPrototypeOf(it);
        const AsyncIteratorPrototype = Object.getPrototypeOf(Object.getPrototypeOf(async function* () {}).prototype);
        const chained = Object.getPrototypeOf(proto) === AsyncIteratorPrototype;
        const names = Object.getOwnPropertyNames(proto).sort().join(",");
        const aliased = ReadableStream.prototype[Symbol.asyncIterator] === ReadableStream.prototype.values;
        globalThis.__out = `${chained},${names},${aliased}`;
        "#);
    assert_eq!(out, "true,next,return,true");
}

/// `next()` clears the ongoing promise when the previous call settles, so a call
/// made after that runs the next steps directly instead of chaining behind the
/// settled promise. Both calls then settle after the same number of ticks.
#[test]
fn settled_next_does_not_delay_the_following_call() {
    let out = run(r#"
        globalThis.__out = "pending";
        const rs = new ReadableStream({ start(c) { c.enqueue(1); c.enqueue(2); } });
        const it = rs.values();
        function ticksUntil(p) {
            let done = false;
            p.then(() => { done = true; });
            return new Promise(resolve => {
                let n = 0;
                (function spin() {
                    if (done) { resolve(n); } else { n++; Promise.resolve().then(spin); }
                })();
            });
        }
        (async () => {
            const first = await ticksUntil(it.next());
            const second = await ticksUntil(it.next());
            globalThis.__out = `${first},${second}`;
        })();
    "#);
    let (first, second) = out.split_once(',').expect("two tick counts");
    assert_eq!(first, second, "ticks: {out}");
}
