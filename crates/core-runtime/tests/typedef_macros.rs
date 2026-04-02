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
            self.data().detail.clone()
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
                    math_ns::add_to_global(scope, global);
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
                    css_ns::add_to_global(scope, global);
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
            let data = self.data_mut();
            data.kind = kind;
            Ok(())
        }

        #[getter]
        fn kind(&self) -> String {
            self.data().kind.clone()
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
            let data = self.data_mut();
            data.parent = PetImpl {
                kind: "The very cutest".to_string(),
            };
            data.cuteness = cuteness;
            Ok(())
        }

        #[getter]
        fn cuteness(&self) -> f64 {
            self.data().cuteness
        }

        #[method]
        fn sound(&self) -> String {
            "The cutest sound!".to_string()
        }

        #[method]
        fn describe(&self) -> String {
            let d = self.data();
            format!("{} Lily, cuteness: {}/5", d.parent.kind, d.cuteness)
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

/// Tests for `#[webidl_dictionary]` — WebIDL dictionary conversion.
mod webidl_dictionary_tests {
    use core_runtime::test_util::{eval_with_setup, throws_with_setup};
    use core_runtime::{jsmethods, webidl_dictionary, webidl_interface};
    use js::Object;

    // A dictionary with all required members.
    #[webidl_dictionary]
    pub struct PersonInit {
        pub name: String,
        pub age: f64,
    }

    // A dictionary with optional members.
    #[webidl_dictionary]
    pub struct GreetingOptions {
        pub prefix: Option<String>,
        pub excited: Option<bool>,
    }

    // A dictionary with a default value.
    #[webidl_dictionary]
    pub struct ConfigInit {
        #[webidl(default = 10.0)]
        pub timeout: f64,
        pub label: Option<String>,
    }

    // A lifetimed dictionary with scope-rooted types.
    #[webidl_dictionary]
    pub struct StreamOptions<'a> {
        pub high_water_mark: f64,
        pub source: Option<Object<'a>>,
    }

    // A dictionary with a custom JS name on a field.
    #[webidl_dictionary]
    pub struct CustomNamed {
        #[webidl(name = "mySpecialField")]
        pub special: String,
    }

    // A class that uses dictionary parameters.
    #[webidl_interface]
    struct Greeter {
        greeting: String,
    }

    #[jsmethods]
    impl Greeter {
        #[constructor]
        fn new(person: PersonInit, options: Option<GreetingOptions>) -> Self {
            let prefix = options
                .as_ref()
                .and_then(|o| o.prefix.clone())
                .unwrap_or_else(|| "Hello".to_string());
            let excited = options.as_ref().and_then(|o| o.excited).unwrap_or(false);
            let suffix = if excited { "!" } else { "." };
            Self {
                greeting: format!("{}, {} (age {}){}", prefix, person.name, person.age, suffix),
            }
        }

        #[getter]
        fn greeting(&self) -> String {
            self.data().greeting.clone()
        }
    }

    // A class testing default values.
    #[webidl_interface]
    struct Config {
        timeout: f64,
        label: String,
    }

    #[jsmethods]
    impl Config {
        #[constructor]
        fn new(init: ConfigInit) -> Self {
            Self {
                timeout: init.timeout,
                label: init.label.unwrap_or_else(|| "default".to_string()),
            }
        }

        #[getter]
        fn timeout(&self) -> f64 {
            self.data().timeout
        }

        #[getter]
        fn label(&self) -> String {
            self.data().label.clone()
        }
    }

    fn setup() {
        core_runtime::runtime::register_global_initializer(|scope, global| {
            Greeter::add_to_global(scope, global);
            Config::add_to_global(scope, global);
        });
    }

    fn eval(code: &str) -> String {
        eval_with_setup(setup, code)
    }

    fn throws(code: &str) -> bool {
        throws_with_setup(setup, code)
    }

    // Required members

    #[test]
    fn required_members_extracted() {
        assert_eq!(
            eval("new Greeter({ name: 'Alice', age: 30 }).greeting"),
            "Hello, Alice (age 30)."
        );
    }

    #[test]
    fn required_member_missing_throws() {
        // Missing 'age' should throw.
        assert!(throws("new Greeter({ name: 'Alice' })"));
    }

    #[test]
    fn required_member_all_missing_throws() {
        assert!(throws("new Greeter({})"));
    }

    #[test]
    fn non_object_dict_throws() {
        assert!(throws("new Greeter(42)"));
    }

    // Optional members

    #[test]
    fn optional_members_present() {
        assert_eq!(
            eval("new Greeter({ name: 'Bob', age: 25 }, { prefix: 'Hi', excited: true }).greeting"),
            "Hi, Bob (age 25)!"
        );
    }

    #[test]
    fn optional_members_absent() {
        assert_eq!(
            eval("new Greeter({ name: 'Charlie', age: 20 }).greeting"),
            "Hello, Charlie (age 20)."
        );
    }

    #[test]
    fn optional_dict_param_null() {
        // Second param (GreetingOptions) is optional and null → treated as None.
        assert_eq!(
            eval("new Greeter({ name: 'Dana', age: 18 }, null).greeting"),
            "Hello, Dana (age 18)."
        );
    }

    // Default values

    #[test]
    fn default_value_used_when_missing() {
        assert_eq!(eval("new Config({}).timeout"), "10");
    }

    #[test]
    fn default_value_overridden() {
        assert_eq!(eval("new Config({ timeout: 42 }).timeout"), "42");
    }

    #[test]
    fn default_and_optional_combined() {
        assert_eq!(eval("new Config({}).label"), "default");
        assert_eq!(eval("new Config({ label: 'custom' }).label"), "custom");
    }

    // Empty/null/undefined dictionaries

    #[test]
    fn null_dict_uses_defaults() {
        // null → all members get their defaults.
        assert_eq!(eval("new Config(null).timeout"), "10");
    }

    #[test]
    fn undefined_dict_uses_defaults() {
        assert_eq!(eval("new Config(undefined).timeout"), "10");
    }
}
