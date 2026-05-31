// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Integration tests for the DOM Abort interfaces: AbortController, AbortSignal.
//!
//! Abort/reason/throwIfAborted/event semantics, `AbortSignal.any()`, and
//! `AbortSignal.timeout()` are thoroughly covered by the enabled, passing WPT
//! tests `dom/abort/event.any.js`, `dom/abort/AbortSignal.any.js`,
//! `dom/abort/abort-signal-any.any.js`, and `dom/abort/timeout.any.js` (see
//! `tests/wpt-harness/tests.json`), so that ground is not retread here. What
//! remains is coverage WPT does not give us in this harness:
//!
//! - `Symbol.toStringTag` brands (no idlharness is enabled).
//! - `onabort` getter identity and the set-to-null path, which `event.any.js`
//!   does not assert.

use core_runtime::test_util::eval_with_setup;

fn setup() {
    core_runtime::runtime::register_global_initializer(|scope, global| {
        web_globals::add_to_global(scope, global);
    });
}

fn eval(code: &str) -> String {
    eval_with_setup(setup, code)
}

// ── Brands / interface shape ──

#[test]
fn abort_controller_to_string_tag() {
    assert_eq!(
        eval("Object.prototype.toString.call(new AbortController())"),
        "[object AbortController]"
    );
}

#[test]
fn abort_controller_has_signal() {
    assert_eq!(
        eval("new AbortController().signal instanceof AbortSignal"),
        "true"
    );
}

#[test]
fn signal_inherits_event_target() {
    assert_eq!(
        eval("new AbortController().signal instanceof EventTarget"),
        "true"
    );
}

#[test]
fn signal_to_string_tag() {
    assert_eq!(
        eval("Object.prototype.toString.call(new AbortController().signal)"),
        "[object AbortSignal]"
    );
}

// ── onabort event handler ──

#[test]
fn onabort_initially_null() {
    assert_eq!(eval("new AbortController().signal.onabort"), "null");
}

#[test]
fn onabort_getter_returns_handler() {
    assert_eq!(
        eval(
            r#"
            var ac = new AbortController();
            var fn = function() {};
            ac.signal.onabort = fn;
            ac.signal.onabort === fn
            "#
        ),
        "true"
    );
}

#[test]
fn onabort_set_null_removes_handler() {
    assert_eq!(
        eval(
            r#"
            var ac = new AbortController();
            var called = false;
            ac.signal.onabort = function() { called = true; };
            ac.signal.onabort = null;
            ac.abort();
            called
            "#
        ),
        "false"
    );
}
