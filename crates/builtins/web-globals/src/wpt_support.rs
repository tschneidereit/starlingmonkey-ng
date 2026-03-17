// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! WPT (Web Platform Tests) support globals.
//!
//! Provides `evalScript` — a global function that evaluates a script in a
//! non-syntactic scope, so that top-level `let` and `const` bindings are
//! shared across calls. This emulates the behavior of HTML `<script>` tags
//! and is required by the WPT test harness.

use js::conversion::ConversionError;
use js::error::throw_error;
use js::gc::scope::Scope;
use js::native::{Handle, RawJSContext};
use js::prelude::FromJSVal;
use js::Object;

/// Install WPT-specific globals on the given global object.
///
/// Currently installs:
/// - `evalScript(source)` — evaluate a script in non-syntactic scope
///
/// # Safety
///
/// Must be called with a valid scope and global object.
pub unsafe fn add_to_global(scope: &Scope<'_>, global: Object<'_>) {
    js::Function::define(
        scope,
        global.handle(),
        c"evalScript",
        Some(eval_script_native),
        1,
        0,
    )
    .expect("failed to define evalScript");
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
            // Exception is already pending on the context.
            false
        }
    }
}

#[cfg(test)]
mod tests {
    mod wpt_integration {
        use core_runtime::test_util::eval_with_setup;

        fn eval_wpt(code: &str) -> String {
            eval_with_setup(
                || {
                    libstarling::register_builtins();
                    libstarling::register_wpt_builtins();
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
    }
}
