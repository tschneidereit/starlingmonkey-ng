// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! WPT (Web Platform Tests) support globals.
//!
//! Provides `evalScript` — a global function that evaluates a script in a
//! non-syntactic scope, so that top-level `let` and `const` bindings are
//! shared across calls. This emulates the behavior of HTML `<script>` tags
//! and is required by the WPT test harness.

use js::conversion::ConversionError;
use js::error::throw_error;
use js::exception::check_fn_return;
use js::gc::scope::Scope;
use js::native::{Handle, RawJSContext};
use js::prelude::FromJSVal;
use js::Object;

/// Install WPT-specific globals on the given global object.
///
/// Currently installs:
/// - `evalScript(source)` — evaluate a script in non-syntactic scope
/// - `__setLocation(url)` — configure the URL backing `globalThis.location`
///
/// # Safety
///
/// Must be called with a valid scope and global object.
pub fn add_to_global(scope: &Scope<'_>, global: Object<'_>) {
    js::Function::define(
        scope,
        global.handle(),
        c"evalScript",
        Some(eval_script_native),
        1,
        0,
    )
    .expect("failed to define evalScript");
    js::Function::define(
        scope,
        global.handle(),
        c"__setLocation",
        Some(set_location_native),
        1,
        0,
    )
    .expect("failed to define __setLocation");
}

/// JSNative implementation of `evalScript(source)`.
///
/// Evaluates the given string as a script in a non-syntactic scope, making
/// top-level `let`/`const` bindings visible to subsequent calls. This is
/// how the WPT harness loads `META: script=...` dependencies.
unsafe extern "C" fn eval_script_native(
    raw_cx: *mut RawJSContext,
    argc: u32,
    vp: *mut js::native::Value,
) -> bool {
    use js::prelude::RootScope;

    let scope = RootScope::from_current_realm(raw_cx);
    let args = js::native::CallArgs::from_vp(vp, argc);
    let source =
        match String::from_jsval(&scope, Handle::from_raw(args.get(0)), ()).inspect_err(|e| {
            if let ConversionError::Failure(_) = e {
                throw_error(&scope, "evalScript: argument must be a string");
            }
        }) {
            Ok(source) => source,
            Err(_) => return false,
        };

    // Evaluate in non-syntactic scope.
    match js::compile::evaluate_non_syntactic(&scope, &source, "evalScript", 1) {
        Ok(rval) => {
            args.rval().set(rval.get());
            true
        }
        Err(_) => {
            println!("Eval failed");
            check_fn_return(&scope, false, "evalScript")
        }
    }
}

/// JSNative implementation of `__setLocation(url)`.
///
/// Parses `url` and installs it as the URL backing `globalThis.location`. The
/// WPT harness calls this once per test with the test's canonical
/// `http://web-platform.test:8000/...` URL so that tests querying
/// `location.origin`, `location.href`, etc. observe the same values they
/// would when loaded from a real WPT HTTP server.
unsafe extern "C" fn set_location_native(
    raw_cx: *mut RawJSContext,
    argc: u32,
    vp: *mut js::native::Value,
) -> bool {
    use js::prelude::RootScope;

    let scope = RootScope::from_current_realm(raw_cx);
    let args = js::native::CallArgs::from_vp(vp, argc);
    let raw = match String::from_jsval(&scope, Handle::from_raw(args.get(0)), ()) {
        Ok(s) => s,
        Err(_) => {
            throw_error(&scope, "__setLocation: argument must be a string");
            return false;
        }
    };
    let url = match url::Url::parse(&raw) {
        Ok(u) => u,
        Err(e) => {
            throw_error(&scope, &format!("__setLocation: invalid URL {raw:?}: {e}"));
            return false;
        }
    };
    web_globals::worker_location::set_init_location(url);
    args.rval().set(js::value::undefined());
    true
}

#[cfg(test)]
mod tests {
    mod wpt_integration {
        use core_runtime::test_util::eval_with_setup;

        fn eval_wpt(code: &str) -> String {
            eval_with_setup(
                || {
                    libstarling::register_builtins();
                    libstarling::runtime::register_global_initializer(crate::add_to_global);
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
    }
}
