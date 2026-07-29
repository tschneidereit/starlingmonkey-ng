// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Integration tests for the WPT support globals (`evalScript`, `__setLocation`).

// This file contains nothing platform-specific, so skip it on wasm32.
#![cfg(not(target_arch = "wasm32"))]

use core_runtime::test_util::eval_with_setup;

fn eval_wpt(code: &str) -> String {
    eval_with_setup(
        || {
            libstarling::register_builtins();
            libstarling::runtime::register_global_initializer(wpt_support::add_to_global);
        },
        code,
    )
}

#[test]
fn eval_script_basic() {
    assert_eq!(eval_wpt("evalScript('1 + 2').toString()"), "3");
}

#[test]
fn eval_script_shares_bindings() {
    // evalScript should place `let` bindings in a non-syntactic scope
    // so they are visible to subsequent evalScript calls.
    assert_eq!(
        eval_wpt("evalScript('let wptFoo = 42;'); evalScript('wptFoo.toString()')"),
        "42"
    );
}

#[test]
fn eval_script_available_in_wpt_mode() {
    // evalScript should be available as a global function.
    assert_eq!(eval_wpt("typeof evalScript"), "function");
}

#[test]
fn set_location_configures_worker_location() {
    // After __setLocation runs, every WorkerLocation accessor must
    // report the configured URL's components.
    assert_eq!(
        eval_wpt(
            "__setLocation('http://web-platform.test:8000/foo/bar.any.js?x=1'); \
             [location.href, location.origin, location.pathname, location.search].join('|')"
        ),
        "http://web-platform.test:8000/foo/bar.any.js?x=1|\
         http://web-platform.test:8000|/foo/bar.any.js|?x=1"
    );
}

#[test]
fn set_location_rejects_invalid_url() {
    assert_eq!(
        eval_wpt(
            "try { __setLocation('not a url'); 'no-throw' } \
             catch(e) { (e instanceof TypeError) + ',' + e.message.includes('invalid URL') }"
        ),
        "true,true"
    );
}
