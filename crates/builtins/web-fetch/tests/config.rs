// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Keystone test for server-side (permissive) Fetch configuration.
//!
//! The browser-security constraints — forbidden request headers, forbidden
//! methods, no-CORS safelisting — are enforced by default (so WPT passes), but
//! StarlingMonkey is primarily a server runtime and must be able to disable
//! them. WPT only ever exercises the enforced path, so this validates the
//! permissive path: with restrictions off, a forbidden header (`Host`) and a
//! forbidden method (`CONNECT`) are accepted; with them on, they are
//! filtered/rejected.

// This file contains nothing platform-specific, so skip it on wasm32.
#![cfg(not(target_arch = "wasm32"))]

use core_runtime::config::RuntimeConfig;
use core_runtime::runtime::{clear_global_initializers, Runtime};
use js::conversion::FromJSVal;

/// Evaluate `code` with request restrictions set to `enforce`, returning
/// `String(globalThis.__out)`.
fn run(enforce: bool, code: &str) -> String {
    clear_global_initializers();
    libstarling::register_builtins();
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();
    web_fetch::config::set_enforce_request_restrictions(enforce);
    if js::compile::evaluate_with_filename(&scope, code, "test.js", 1).is_err() {
        panic!(
            "evaluation threw: {:?}",
            js::error::ExnThrown::capture(&scope)
        );
    }
    // Restore the default so the thread-local does not leak into other tests.
    web_fetch::config::set_enforce_request_restrictions(true);
    let out = js::compile::evaluate_with_filename(&scope, "String(globalThis.__out)", "out.js", 1)
        .expect("reading __out threw");
    String::from_jsval(&scope, out, ()).unwrap()
}

/// With restrictions enforced (the default), a forbidden request header is
/// dropped and a forbidden method throws.
#[test]
fn restrictions_enforced_by_default() {
    let out = run(
        true,
        r#"
        const r = new Request("https://example.com/", { headers: { "Host": "evil" } });
        let methodThrew = false;
        try { new Request("https://example.com/", { method: "CONNECT" }); }
        catch (e) { methodThrew = e instanceof TypeError; }
        globalThis.__out = `host=${r.headers.get("Host")},connectThrew=${methodThrew}`;
        "#,
    );
    assert_eq!(out, "host=null,connectThrew=true");
}

/// With restrictions disabled (server-side mode), the forbidden header is kept
/// and the forbidden method is accepted.
#[test]
fn restrictions_disabled_allows_forbidden_header_and_method() {
    let out = run(
        false,
        r#"
        const r = new Request("https://example.com/", {
            method: "CONNECT",
            headers: { "Host": "example.com" },
        });
        globalThis.__out = `host=${r.headers.get("Host")},method=${r.method}`;
        "#,
    );
    assert_eq!(out, "host=example.com,method=CONNECT");
}
