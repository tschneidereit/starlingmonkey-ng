// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Integration tests for the `js` API.
//!
//! Tests that need a JS runtime are grouped in a single test because
//! `JSEngine` can only be initialized once per process.

/// Tests for `#[jsclass(js_proto = "Error")]` — inheriting from a JS built-in prototype.
mod js_proto_tests {
    use core_runtime::jsclass;
    use core_runtime::jsmethods;
    use core_runtime::test_util::eval_with_setup;

    /// Minimal class inheriting from Error.prototype via `js_proto`.
    #[jsclass(js_proto = "Error", to_string_tag = "CustomError")]
    struct CustomError {
        detail: String,
    }

    #[jsmethods]
    impl CustomError {
        #[constructor]
        fn construct(detail: String) -> Self {
            Self { detail }
        }

        #[getter]
        pub fn detail(&self) -> String {
            self.detail.clone()
        }
    }

    fn eval(code: &str) -> String {
        eval_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    CustomError::add_to_global(scope, global);
                });
            },
            code,
        )
    }

    #[test]
    fn inherits_from_error_prototype() {
        assert_eq!(eval("new CustomError('x') instanceof Error"), "true");
    }

    #[test]
    fn is_custom_error_instance() {
        assert_eq!(eval("new CustomError('x') instanceof CustomError"), "true");
    }

    #[test]
    fn prototype_chain() {
        assert_eq!(
            eval("Object.getPrototypeOf(CustomError.prototype) === Error.prototype"),
            "true"
        );
    }

    #[test]
    fn getter_works() {
        assert_eq!(eval("new CustomError('test detail').detail"), "test detail");
    }

    #[test]
    fn not_array_instance() {
        assert_eq!(eval("new CustomError('x') instanceof Array"), "false");
    }

    #[test]
    fn to_string_tag() {
        assert_eq!(
            eval("Object.prototype.toString.call(new CustomError('x'))"),
            "[object CustomError]"
        );
    }

    #[test]
    fn to_string_tag_on_prototype() {
        assert_eq!(
            eval("CustomError.prototype[Symbol.toStringTag]"),
            "CustomError"
        );
    }

    #[test]
    fn has_stack_property() {
        assert_eq!(eval("typeof new CustomError('x').stack"), "string");
    }

    #[test]
    fn stack_is_nonempty() {
        assert_eq!(eval("new CustomError('x').stack.length > 0"), "true");
    }
}

mod constant_tests {
    use core_runtime::jsclass;
    use core_runtime::jsmethods;
    use core_runtime::test_util::eval_with_setup;

    /// Class with pub const items exposed as constructor constants.
    #[jsclass]
    struct StatusCode {}

    #[jsmethods]
    impl StatusCode {
        pub const OK: u16 = 200;
        pub const NOT_FOUND: u16 = 404;
        pub const INTERNAL_SERVER_ERROR: u16 = 500;

        #[constructor]
        fn construct() -> Self {
            Self {}
        }
    }

    fn eval(code: &str) -> String {
        eval_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    StatusCode::add_to_global(scope, global);
                });
            },
            code,
        )
    }

    #[test]
    fn constant_on_constructor() {
        assert_eq!(eval("StatusCode.OK"), "200");
    }

    #[test]
    fn constant_not_found() {
        assert_eq!(eval("StatusCode.NOT_FOUND"), "404");
    }

    #[test]
    fn constant_internal_server_error() {
        assert_eq!(eval("StatusCode.INTERNAL_SERVER_ERROR"), "500");
    }

    #[test]
    fn constant_is_readonly() {
        assert_eq!(
            eval("'use strict'; try { StatusCode.OK = 999; 'changed' } catch(e) { 'error' }"),
            "error"
        );
    }

    #[test]
    fn constant_is_enumerable() {
        assert_eq!(eval("Object.keys(StatusCode).includes('OK')"), "true");
    }

    #[test]
    fn constant_not_on_prototype() {
        assert_eq!(eval("StatusCode.prototype.OK"), "undefined");
    }

    #[test]
    fn constant_not_on_instance() {
        assert_eq!(eval("new StatusCode().OK"), "undefined");
    }
}

/// Tests for `#[webidl_interface]`: auto Symbol.toStringTag and
/// constants on both constructor AND prototype.
mod webidl_interface_tests {
    use core_runtime::jsmethods;
    use core_runtime::test_util::eval_with_setup;
    use core_runtime::webidl_interface;

    /// A WebIDL-style interface with constants and methods.
    #[webidl_interface]
    struct MediaError {}

    #[jsmethods]
    impl MediaError {
        pub const MEDIA_ERR_ABORTED: u16 = 1;
        pub const MEDIA_ERR_NETWORK: u16 = 2;
        pub const MEDIA_ERR_DECODE: u16 = 3;
        pub const MEDIA_ERR_SRC_NOT_SUPPORTED: u16 = 4;

        #[constructor]
        fn construct() -> Self {
            Self {}
        }
    }

    fn eval(code: &str) -> String {
        eval_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    MediaError::add_to_global(scope, global);
                });
            },
            code,
        )
    }

    // --- Auto Symbol.toStringTag ---

    #[test]
    fn auto_to_string_tag() {
        assert_eq!(
            eval("Object.prototype.toString.call(new MediaError())"),
            "[object MediaError]"
        );
    }

    #[test]
    fn to_string_tag_on_prototype() {
        assert_eq!(
            eval("MediaError.prototype[Symbol.toStringTag]"),
            "MediaError"
        );
    }

    // --- Constants on constructor ---

    #[test]
    fn constant_on_constructor() {
        assert_eq!(eval("MediaError.MEDIA_ERR_ABORTED"), "1");
    }

    #[test]
    fn constant_network_on_constructor() {
        assert_eq!(eval("MediaError.MEDIA_ERR_NETWORK"), "2");
    }

    // --- Constants on prototype (WebIDL §3.7.3) ---

    #[test]
    fn constant_on_prototype() {
        assert_eq!(eval("MediaError.prototype.MEDIA_ERR_ABORTED"), "1");
    }

    #[test]
    fn constant_decode_on_prototype() {
        assert_eq!(eval("MediaError.prototype.MEDIA_ERR_DECODE"), "3");
    }

    #[test]
    fn constant_src_not_supported_on_prototype() {
        assert_eq!(
            eval("MediaError.prototype.MEDIA_ERR_SRC_NOT_SUPPORTED"),
            "4"
        );
    }

    // --- Constants on instances (inherited via prototype) ---

    #[test]
    fn constant_on_instance() {
        assert_eq!(eval("new MediaError().MEDIA_ERR_ABORTED"), "1");
    }

    // --- Constants are read-only ---

    #[test]
    fn constant_readonly_on_constructor() {
        assert_eq!(
            eval("'use strict'; try { MediaError.MEDIA_ERR_ABORTED = 99; 'changed' } catch(e) { 'error' }"),
            "error"
        );
    }

    #[test]
    fn constant_readonly_on_prototype() {
        assert_eq!(
            eval("'use strict'; try { MediaError.prototype.MEDIA_ERR_ABORTED = 99; 'changed' } catch(e) { 'error' }"),
            "error"
        );
    }
}

/// Tests for `#[jsnamespace]`: plain singleton object on global.
mod jsnamespace_tests {
    use core_runtime::jsnamespace;
    use core_runtime::test_util::eval_with_setup;

    #[jsnamespace(name = "math")]
    mod math_ns {
        use js::gc::scope::Scope;

        pub fn add(a: f64, b: f64) -> f64 {
            a + b
        }

        pub fn multiply(a: f64, b: f64) -> f64 {
            a * b
        }

        pub fn greet(scope: &Scope<'_>, name: String) -> String {
            let _ = scope; // Verify scope is passed through
            format!("Hello, {name}!")
        }
    }

    fn eval(code: &str) -> String {
        eval_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    unsafe { math_ns::add_to_global(scope, global) };
                });
            },
            code,
        )
    }

    #[test]
    fn namespace_exists() {
        assert_eq!(eval("typeof math"), "object");
    }

    #[test]
    fn namespace_function_call() {
        assert_eq!(eval("math.add(2, 3)"), "5");
    }

    #[test]
    fn namespace_multiply() {
        assert_eq!(eval("math.multiply(4, 5)"), "20");
    }

    #[test]
    fn namespace_function_with_scope() {
        assert_eq!(eval("math.greet('World')"), "Hello, World!");
    }

    #[test]
    fn namespace_not_constructable() {
        assert_eq!(
            eval("try { new math.add(); 'ok' } catch(e) { 'error' }"),
            "error"
        );
    }

    #[test]
    fn namespace_no_to_string_tag() {
        assert_eq!(
            eval("Object.prototype.toString.call(math)"),
            "[object Object]"
        );
    }
}

/// Tests for `#[webidl_namespace]`: namespace with auto Symbol.toStringTag.
mod webidl_namespace_tests {
    use core_runtime::{test_util::eval_with_setup, webidl_namespace};

    #[webidl_namespace(name = "CSS")]
    mod css_ns {
        pub fn escape(value: String) -> String {
            // Simplified CSS.escape for testing
            value.replace('\\', "\\\\")
        }
    }

    fn eval(code: &str) -> String {
        eval_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    unsafe { css_ns::add_to_global(scope, global) };
                });
            },
            code,
        )
    }

    #[test]
    fn namespace_exists() {
        assert_eq!(eval("typeof CSS"), "object");
    }

    #[test]
    fn namespace_function_call() {
        assert_eq!(eval("CSS.escape('hello')"), "hello");
    }

    #[test]
    fn auto_to_string_tag() {
        assert_eq!(eval("Object.prototype.toString.call(CSS)"), "[object CSS]");
    }

    #[test]
    fn to_string_tag_on_namespace() {
        assert_eq!(eval("CSS[Symbol.toStringTag]"), "CSS");
    }
}

/// Tests for setup-style constructors (using `&self`) with `extends` inheritance.
mod setup_style_inheritance {
    use core_runtime::jsclass;
    use core_runtime::jsmethods;
    use core_runtime::test_util::eval_with_setup;

    #[jsclass]
    struct Pet {
        kind: String,
    }

    #[jsmethods]
    impl Pet {
        #[constructor]
        fn new(&self, kind: String) -> Result<(), String> {
            let data = unsafe { self.data_mut().unwrap() };
            data.kind = kind;
            Ok(())
        }

        #[getter]
        fn kind(&self) -> String {
            self.kind.clone()
        }

        #[method]
        fn sound(&self) -> String {
            "<generic pet sound>".to_string()
        }
    }

    #[jsclass(extends = Pet)]
    struct Lily {
        parent: Pet,
        cuteness: f64,
    }

    #[jsmethods]
    impl Lily {
        #[constructor]
        fn new(&self, cuteness: f64) -> Result<(), String> {
            let data = unsafe { self.data_mut().unwrap() };
            data.parent = PetImpl {
                kind: "The very cutest".to_string(),
            };
            data.cuteness = cuteness;
            Ok(())
        }

        #[getter]
        fn cuteness(&self) -> f64 {
            self.cuteness
        }

        #[method]
        fn sound(&self) -> String {
            "The cutest sound!".to_string()
        }

        #[method]
        fn describe(&self) -> String {
            format!("{} Lily, cuteness: {}/5", self.parent.kind, self.cuteness)
        }
    }

    fn eval(code: &str) -> String {
        eval_with_setup(
            || {
                core_runtime::runtime::register_global_initializer(|scope, global| {
                    Pet::add_to_global(scope, global);
                    Lily::add_to_global(scope, global);
                });
            },
            code,
        )
    }

    #[test]
    fn pet_constructor() {
        assert_eq!(eval("new Pet('Cat').kind"), "Cat");
    }

    #[test]
    fn pet_method() {
        assert_eq!(eval("new Pet('Some dog').sound()"), "<generic pet sound>");
    }

    #[test]
    fn lily_constructor() {
        assert_eq!(eval("new Lily(5).kind"), "The very cutest");
    }

    #[test]
    fn lily_payload() {
        assert_eq!(eval("new Lily(5).cuteness"), "5");
    }

    #[test]
    fn lily_overrides_method() {
        assert_eq!(eval("new Lily(5).sound()"), "The cutest sound!");
    }

    #[test]
    fn lily_instanceof() {
        assert_eq!(eval("new Lily(5) instanceof Pet"), "true");
    }

    #[test]
    fn lily_describe() {
        assert_eq!(
            eval("new Lily(5).describe()"),
            "The very cutest Lily, cuteness: 5/5"
        );
    }
}
