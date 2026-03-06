// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! WPT (Web Platform Tests) support globals.
//!
//! Provides `evalScript` — a global function that evaluates a script in a
//! non-syntactic scope, so that top-level `let` and `const` bindings are
//! shared across calls. This emulates the behavior of HTML `<script>` tags
//! and is required by the WPT test harness.

use js::gc::scope::Scope;
use js::native::RawJSContext;
use js::object::Object;

/// Install WPT-specific globals on the given global object.
///
/// Currently installs:
/// - `evalScript(source)` — evaluate a script in non-syntactic scope
///
/// # Safety
///
/// Must be called with a valid scope and global object.
pub unsafe fn add_to_global(scope: &Scope<'_>, global: Object<'_>) {
    js::function::define_function(
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

    let mut cx = js::native::JSContext::from_ptr(std::ptr::NonNull::new_unchecked(raw_cx));
    let scope = RootScope::from_current_realm(&mut cx);
    let args = js::native::CallArgs::from_vp(vp, argc);

    // Get the source string argument.
    if args.argc_ < 1 || !args.get(0).is_string() {
        core_runtime::class::throw_error(&scope, "evalScript: argument must be a string");
        return false;
    }
    let source_val = args.get(0);

    let source_str = std::ptr::NonNull::new(source_val.to_string());
    let source_str = match source_str {
        Some(s) => s,
        None => {
            core_runtime::class::throw_error(&scope, "evalScript: null string argument");
            return false;
        }
    };
    let source_str = scope.root_string(source_str);
    let source = match js::string::to_utf8(&scope, source_str) {
        Ok(s) => s,
        Err(_) => {
            core_runtime::class::throw_error(
                &scope,
                "evalScript: failed to convert string to UTF-8",
            );
            return false;
        }
    };

    // Evaluate in non-syntactic scope.
    match js::compile::evaluate_non_syntactic(&scope, &source, "evalScript", 1) {
        Ok(rval) => {
            args.rval().set(rval);
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
