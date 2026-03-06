use mozjs::jsapi::{JSContext, JSObject};
use mozjs::rust::HandleObject;

unsafe extern "C" {
    fn console_install(cx: *mut JSContext, global: *mut JSObject) -> bool;
}

/// Install all C++ builtins on the given global object.
///
/// # Safety
/// `cx` and `global` must be valid SpiderMonkey context/global pointers
/// with an active realm.
pub unsafe fn install(cx: *mut JSContext, global: HandleObject) -> bool {
    unsafe { console_install(cx, global.get()) }
}


#[cfg(test)]
mod tests {
    mod console_integration {
        use core_runtime::test_util::throws_with_setup;

        fn eval(code: &str) -> bool {
            libstarling::register_builtins();
            let rt =
                libstarling::runtime::Runtime::init(&libstarling::config::RuntimeConfig::default());
            let scope = rt.default_global();
            let rval = js::compile::evaluate_with_filename(&scope, code, "test.js", 1)
                .expect("eval failed");
            rval.is_undefined() || rval.to_boolean()
        }

        fn eval_throws(code: &str) -> bool {
            throws_with_setup(libstarling::register_builtins, code)
        }

        #[test]
        fn console_log_exists() {
            assert!(eval("typeof console === 'object'"));
        }

        #[test]
        fn console_methods_exist() {
            assert!(eval("typeof console.log === 'function'"));
            assert!(eval("typeof console.warn === 'function'"));
            assert!(eval("typeof console.error === 'function'"));
            assert!(eval("typeof console.info === 'function'"));
            assert!(eval("typeof console.debug === 'function'"));
        }

        #[test]
        fn console_log_returns_undefined() {
            assert!(eval("console.log('test') === undefined"));
        }

        #[test]
        fn console_log_no_args() {
            // Should not throw with zero arguments.
            assert!(!eval_throws("console.log()"));
        }

        #[test]
        fn console_log_multiple_args() {
            assert!(!eval_throws("console.log('a', 'b', 'c')"));
        }

        #[test]
        fn console_log_non_string_args() {
            assert!(!eval_throws(
                "console.log(42, true, null, undefined, {x: 1})"
            ));
        }
    }
}