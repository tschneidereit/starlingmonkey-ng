// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Keystone test for the request-restrictions configuration.
//!
//! The browser-security constraints — forbidden request headers, forbidden
//! methods, no-CORS safelisting, origin/mode enforcement — are off by default:
//! StarlingMonkey is primarily a server runtime, where a script must be able to
//! set `Host` or use `CONNECT`. WPT mode turns them on by default, and an
//! explicit `--enforce-fetch-restrictions[=bool]` overrides either default.
//! `Runtime::init` applies the configured value, which is what these tests
//! exercise end to end: with restrictions off, a forbidden header (`Host`) and
//! a forbidden method (`CONNECT`) are accepted; with them on, they are
//! filtered/rejected.

// This file contains nothing platform-specific, so skip it on wasm32.
#![cfg(not(target_arch = "wasm32"))]

use core_runtime::config::RuntimeConfig;
use core_runtime::runtime::{clear_global_initializers, Runtime};
use js::conversion::FromJSVal;

/// Evaluate `code` in a runtime initialized from `config`, returning
/// `String(globalThis.__out)`.
fn run(config: &RuntimeConfig, code: &str) -> String {
    clear_global_initializers();
    libstarling::register_builtins();
    let rt = Runtime::init(config);
    let scope = rt.default_global();
    if js::compile::evaluate_with_filename(&scope, code, "test.js", 1).is_err() {
        panic!(
            "evaluation threw: {:?}",
            js::error::ExnThrown::capture(&scope)
        );
    }
    let out = js::compile::evaluate_with_filename(&scope, "String(globalThis.__out)", "out.js", 1)
        .expect("reading __out threw");
    String::from_jsval(&scope, out, ()).unwrap()
}

const PERMISSIVE_PROBE: &str = r#"
    const r = new Request("https://example.com/", {
        method: "CONNECT",
        headers: { "Host": "example.com" },
    });
    globalThis.__out = `host=${r.headers.get("Host")},method=${r.method}`;
"#;

const ENFORCED_PROBE: &str = r#"
    const r = new Request("https://example.com/", { headers: { "Host": "evil" } });
    let methodThrew = false;
    try { new Request("https://example.com/", { method: "CONNECT" }); }
    catch (e) { methodThrew = e instanceof TypeError; }
    globalThis.__out = `host=${r.headers.get("Host")},connectThrew=${methodThrew}`;
"#;

/// By default the restrictions are off: the forbidden header is kept and the
/// forbidden method accepted.
#[test]
fn restrictions_off_by_default() {
    let out = run(&RuntimeConfig::default(), PERMISSIVE_PROBE);
    assert_eq!(out, "host=example.com,method=CONNECT");
}

/// WPT mode defaults the restrictions to on: the forbidden header is dropped
/// and the forbidden method throws.
#[test]
fn wpt_mode_enforces_restrictions() {
    let config = RuntimeConfig::from_arg_string("--wpt-mode").unwrap();
    let out = run(&config, ENFORCED_PROBE);
    assert_eq!(out, "host=null,connectThrew=true");
}

/// An explicit `--enforce-fetch-restrictions=false` overrides WPT mode's
/// default, and `--enforce-fetch-restrictions` (bare) enables them without
/// WPT mode.
#[test]
fn explicit_flag_overrides_either_default() {
    let config =
        RuntimeConfig::from_arg_string("--wpt-mode --enforce-fetch-restrictions=false").unwrap();
    let out = run(&config, PERMISSIVE_PROBE);
    assert_eq!(out, "host=example.com,method=CONNECT");

    let config = RuntimeConfig::from_arg_string("--enforce-fetch-restrictions").unwrap();
    let out = run(&config, ENFORCED_PROBE);
    assert_eq!(out, "host=null,connectThrew=true");
}

/// The runtime state stays overridable programmatically after init, for
/// embedders that decide per-invocation.
#[test]
fn setter_overrides_after_init() {
    clear_global_initializers();
    libstarling::register_builtins();
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();
    core_runtime::config::set_enforce_fetch_restrictions(true);
    assert!(core_runtime::config::enforce_fetch_restrictions());
    let out = js::compile::evaluate_with_filename(&scope, ENFORCED_PROBE, "test.js", 1);
    assert!(out.is_ok());
    let out = js::compile::evaluate_with_filename(&scope, "String(globalThis.__out)", "out.js", 1)
        .expect("reading __out threw");
    assert_eq!(
        String::from_jsval(&scope, out, ()).unwrap(),
        "host=null,connectThrew=true"
    );
    // Restore for other tests on this thread.
    core_runtime::config::set_enforce_fetch_restrictions(false);
}
