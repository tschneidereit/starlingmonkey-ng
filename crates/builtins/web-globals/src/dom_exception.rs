// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! `DOMException` implementation.
//!
//! Implements the [`DOMException`] interface from the WebIDL specification,
//! including the constructor, prototype accessors, legacy error code constants,
//! and the `Symbol.toStringTag` property.
//!
//! Uses `#[webidl_interface]` and `#[jsmethods]` proc macros for declarative
//! class registration.
//!
//! [`DOMException`]: https://webidl.spec.whatwg.org/#idl-DOMException

use std::ptr::NonNull;

use js::error::ThrowException;
use js::gc::scope::Scope;
use js::native::{ExceptionStackBehavior, HandleValueArray};

// ---------------------------------------------------------------------------
// DOMException names table
// ---------------------------------------------------------------------------

/// https://webidl.spec.whatwg.org/#dfn-error-names-table
///
/// Maps DOMException error names to their legacy error code values.
/// Names not present in this table have legacy code 0.
const ERROR_NAMES: &[(&str, u16)] = &[
    ("IndexSizeError", 1),
    ("HierarchyRequestError", 3),
    ("WrongDocumentError", 4),
    ("InvalidCharacterError", 5),
    ("NoModificationAllowedError", 7),
    ("NotFoundError", 8),
    ("NotSupportedError", 9),
    ("InUseAttributeError", 10),
    ("InvalidStateError", 11),
    ("SyntaxError", 12),
    ("InvalidModificationError", 13),
    ("NamespaceError", 14),
    ("InvalidAccessError", 15),
    ("TypeMismatchError", 17),
    ("SecurityError", 18),
    ("NetworkError", 19),
    ("AbortError", 20),
    ("URLMismatchError", 21),
    ("QuotaExceededError", 22),
    ("TimeoutError", 23),
    ("InvalidNodeTypeError", 24),
    ("DataCloneError", 25),
];

/// Look up the legacy error code for a DOMException name.
///
/// https://webidl.spec.whatwg.org/#dom-domexception-code
/// "Return the legacy code indicated in the error names table for this
/// DOMException object's name, or 0 if no such entry exists."
fn legacy_code_for_name(name: &str) -> u16 {
    for &(n, code) in ERROR_NAMES {
        if n == name {
            return code;
        }
    }
    0
}

// ---------------------------------------------------------------------------
// DOMException class
// ---------------------------------------------------------------------------

/// Internal data for a `DOMException` instance.
///
/// Stores the exception name and message as Rust `String`s. The legacy
/// error code is computed from the name on demand via `legacy_code_for_name`.
#[core_runtime::webidl_interface(name = "DOMException", js_proto = "Error")]
pub struct DOMException {
    name: String,
    message: String,
}

/// Coerce a JS argument to a Rust `String` via the JS `ToString` operation.
///
/// If the argument is already a string, extracts it directly. Otherwise
/// calls SpiderMonkey's `ToStringSlow` for proper type coercion.
///
/// Returns `None` if an exception is pending (e.g., `ToString` on a Symbol).
unsafe fn coerce_arg_to_string(
    scope: &Scope<'_>,
    args: &js::native::CallArgs,
    index: u32,
) -> Option<String> {
    let val = *args.get(index);

    if val.is_string() {
        let s = val.to_string();
        let rooted = scope.root_string(NonNull::new_unchecked(s));
        return js::string::to_utf8(scope, rooted).ok();
    }

    let rooted_val = scope.root_value(val);
    let rooted_str = js::string::to_string_slow(scope, rooted_val).ok()?;
    js::string::to_utf8(scope, rooted_str).ok()
}

#[core_runtime::jsmethods]
impl DOMException {
    // -----------------------------------------------------------------------
    // Legacy error code constants (WebIDL §2.8.1)
    // -----------------------------------------------------------------------
    pub const INDEX_SIZE_ERR: u16 = 1;
    pub const DOMSTRING_SIZE_ERR: u16 = 2;
    pub const HIERARCHY_REQUEST_ERR: u16 = 3;
    pub const WRONG_DOCUMENT_ERR: u16 = 4;
    pub const INVALID_CHARACTER_ERR: u16 = 5;
    pub const NO_DATA_ALLOWED_ERR: u16 = 6;
    pub const NO_MODIFICATION_ALLOWED_ERR: u16 = 7;
    pub const NOT_FOUND_ERR: u16 = 8;
    pub const NOT_SUPPORTED_ERR: u16 = 9;
    pub const INUSE_ATTRIBUTE_ERR: u16 = 10;
    pub const INVALID_STATE_ERR: u16 = 11;
    pub const SYNTAX_ERR: u16 = 12;
    pub const INVALID_MODIFICATION_ERR: u16 = 13;
    pub const NAMESPACE_ERR: u16 = 14;
    pub const INVALID_ACCESS_ERR: u16 = 15;
    pub const VALIDATION_ERR: u16 = 16;
    pub const TYPE_MISMATCH_ERR: u16 = 17;
    pub const SECURITY_ERR: u16 = 18;
    pub const NETWORK_ERR: u16 = 19;
    pub const ABORT_ERR: u16 = 20;
    pub const URL_MISMATCH_ERR: u16 = 21;
    pub const QUOTA_EXCEEDED_ERR: u16 = 22;
    pub const TIMEOUT_ERR: u16 = 23;
    pub const INVALID_NODE_TYPE_ERR: u16 = 24;
    pub const DATA_CLONE_ERR: u16 = 25;

    // -----------------------------------------------------------------------
    // Constructor
    // -----------------------------------------------------------------------

    /// https://webidl.spec.whatwg.org/#dom-domexception-domexception
    ///
    /// `constructor(optional DOMString message = "", optional DOMString name = "Error")`
    ///
    /// Both parameters are optional with defaults. Uses the raw `&CallArgs`
    /// pattern to handle optional parameter extraction with defaults.
    #[constructor]
    fn new(scope: &Scope<'_>, args: &js::native::CallArgs) -> Self {
        // Parse the `message` argument (default: "").
        let message = if args.argc_ >= 1 && !(*args.get(0)).is_undefined() {
            unsafe { coerce_arg_to_string(scope, args, 0) }.unwrap_or_default()
        } else {
            String::new()
        };

        // Parse the `name` argument (default: "Error").
        let name = if args.argc_ >= 2 && !(*args.get(1)).is_undefined() {
            unsafe { coerce_arg_to_string(scope, args, 1) }.unwrap_or_else(|| "Error".to_string())
        } else {
            "Error".to_string()
        };

        Self { name, message }
    }

    // -----------------------------------------------------------------------
    // Getters
    // -----------------------------------------------------------------------

    /// https://webidl.spec.whatwg.org/#dom-domexception-name
    #[getter]
    fn name(&self) -> String {
        self.name.clone()
    }

    /// https://webidl.spec.whatwg.org/#dom-domexception-message
    #[getter]
    fn message(&self) -> String {
        self.message.clone()
    }

    /// https://webidl.spec.whatwg.org/#dom-domexception-code
    ///
    /// Returns the legacy code indicated in the error names table for this
    /// DOMException object's name, or 0 if no such entry exists.
    #[getter]
    fn code(&self) -> i32 {
        legacy_code_for_name(&self.name) as i32
    }
}

// ---------------------------------------------------------------------------
// Public API: throw_dom_exception
// ---------------------------------------------------------------------------

/// Throw a DOMException with the given name and message.
///
/// This creates a new DOMException object via the JS constructor and sets it
/// as the pending exception. Returns `false` to indicate an exception has
/// been thrown (for use in JSNative return values).
///
/// # Safety
///
/// Must be called with a valid scope.
pub unsafe fn throw_dom_exception(scope: &Scope<'_>, name: &str, message: &str) -> bool {
    // Build the constructor arguments: [message, name].
    let msg_str = match js::string::from_str(scope, message) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let name_str = match js::string::from_str(scope, name) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let msg_val = js::value::from_string_raw(msg_str.get());
    let name_val = js::value::from_string_raw(name_str.get());

    let argv = [msg_val, name_val];
    let hva = HandleValueArray {
        length_: 2,
        elements_: argv.as_ptr(),
    };

    // Get the DOMException constructor from the global.
    let global = scope.global();
    let ctor_val = match global.get_property(scope, c"DOMException") {
        Ok(v) => v,
        Err(_) => return false,
    };

    if !ctor_val.is_object() {
        js::error::throw_type_error(scope.cx_mut(), c"DOMException constructor not found");
        return false;
    }

    let ctor_handle = scope.root_value(ctor_val);
    let exception = match js::function::construct(scope, ctor_handle, &hva) {
        Ok(obj) => obj,
        Err(_) => return false,
    };

    // Set the created DOMException as the pending exception.
    let exc_val = js::value::from_object(exception.as_raw());
    let exc_handle = scope.root_value(exc_val);
    js::exception::set_pending(scope, exc_handle, ExceptionStackBehavior::DoNotCapture);
    false
}

// ---------------------------------------------------------------------------
// DOMExceptionError — typed error for use with ThrowException
// ---------------------------------------------------------------------------

/// A typed error representing a `DOMException` to be thrown.
///
/// Use this as the `Err` type in `Result<T, DOMExceptionError>` in
/// `#[jsmethods]`, `#[jsglobals]`, etc. The proc macro's error dispatch
/// will construct a JavaScript `DOMException` object and set it as the
/// pending exception.
///
/// # Example
///
/// ```rust,ignore
/// use web_globals::dom_exception::DOMExceptionError;
///
/// #[method]
/// fn decode(input: String) -> Result<String, DOMExceptionError> {
///     if !is_valid(&input) {
///         return Err(DOMExceptionError::new("InvalidCharacterError", "invalid input"));
///     }
///     Ok(do_decode(&input))
/// }
/// ```
#[derive(Debug, Clone)]
pub struct DOMExceptionError {
    /// The DOMException error name (e.g. "InvalidCharacterError", "NotFoundError").
    pub name: &'static str,
    /// The error message.
    pub message: String,
}

impl DOMExceptionError {
    /// Create a new `DOMExceptionError` with the given name and message.
    pub fn new(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for DOMExceptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.message)
    }
}

impl std::error::Error for DOMExceptionError {}

impl ThrowException for DOMExceptionError {
    unsafe fn throw(self, scope: &Scope<'_>) {
        throw_dom_exception(scope, self.name, &self.message);
    }
}

#[cfg(test)]
mod dom_exception_integration {
    use core_runtime::test_util::eval_with_setup;

    fn eval(code: &str) -> String {
        eval_with_setup(libstarling::register_builtins, code)
    }

    #[test]
    fn constructor_no_args() {
        assert_eq!(eval("let e = new DOMException(); e.name"), "Error");
        assert_eq!(eval("let e = new DOMException(); e.message"), "");
        assert_eq!(eval("let e = new DOMException(); e.code"), "0");
    }

    #[test]
    fn constructor_message_only() {
        assert_eq!(eval("new DOMException('hello').message"), "hello");
        assert_eq!(eval("new DOMException('hello').name"), "Error");
    }

    #[test]
    fn constructor_message_and_name() {
        assert_eq!(
            eval("new DOMException('msg', 'NotFoundError').name"),
            "NotFoundError"
        );
        assert_eq!(eval("new DOMException('msg', 'NotFoundError').code"), "8");
    }

    #[test]
    fn instanceof() {
        assert_eq!(eval("new DOMException() instanceof DOMException"), "true");
        assert_eq!(eval("new DOMException() instanceof Error"), "true");
    }

    #[test]
    fn prototype_chain() {
        assert_eq!(
            eval("Object.getPrototypeOf(DOMException.prototype) === Error.prototype"),
            "true"
        );
    }

    #[test]
    fn to_string_tag() {
        assert_eq!(
            eval("Object.prototype.toString.call(new DOMException())"),
            "[object DOMException]"
        );
    }

    #[test]
    fn constants_on_constructor() {
        assert_eq!(eval("DOMException.INDEX_SIZE_ERR"), "1");
        assert_eq!(eval("DOMException.NOT_FOUND_ERR"), "8");
        assert_eq!(eval("DOMException.NOT_SUPPORTED_ERR"), "9");
        assert_eq!(eval("DOMException.INVALID_STATE_ERR"), "11");
        assert_eq!(eval("DOMException.SYNTAX_ERR"), "12");
    }

    #[test]
    fn constants_on_prototype() {
        assert_eq!(eval("DOMException.prototype.INDEX_SIZE_ERR"), "1");
        assert_eq!(eval("DOMException.prototype.NOT_FOUND_ERR"), "8");
    }

    #[test]
    fn code_not_affected_by_name_shadow() {
        assert_eq!(
            eval(
                "let e = new DOMException('msg', 'InvalidCharacterError'); \
                    Object.defineProperty(e, 'name', { value: 'WrongDocumentError' }); \
                    e.code"
            ),
            "5"
        );
    }

    #[test]
    fn brand_check_rejects_non_instance() {
        // Calling getter on a non-DOMException should throw TypeError.
        assert_eq!(
            eval(
                "let getter = Object.getOwnPropertyDescriptor(\
                        DOMException.prototype, 'name').get; \
                    try { getter.call({}); 'no-throw' } catch(e) { e instanceof TypeError }"
            ),
            "true"
        );
    }

    #[test]
    fn requires_new() {
        // DOMException() without `new` should throw TypeError.
        assert_eq!(
            eval("try { DOMException(); 'no-throw' } catch(e) { e instanceof TypeError }"),
            "true"
        );
    }

    #[test]
    fn stack_property() {
        assert_eq!(eval("typeof (new DOMException()).stack"), "string");
    }

    #[test]
    fn inherits_tostring_from_error() {
        assert_eq!(
            eval("new DOMException('hello', 'TestError').toString()"),
            "TestError: hello"
        );
    }
}

/// Tests for typed error dispatch via `ThrowException` trait.
///
/// These tests use `#[jsglobals]` to register functions that return
/// `Result<T, TypeError>`, `Result<T, RangeError>`, `Result<T, SyntaxError>`,
/// `Result<T, DOMExceptionError>`, and `Result<T, String>`.
#[cfg(test)]
mod throw_exception_integration {
    use crate::dom_exception::DOMExceptionError;
    use core_runtime::{jsglobals, test_util::eval_with_setup};
    use js::error::{RangeError, SyntaxError, TypeError};

    #[jsglobals]
    mod test_error_globals {
        use super::*;

        /// Throws a TypeError.
        pub fn throw_type_error() -> Result<(), TypeError> {
            Err(TypeError("test type error".into()))
        }

        /// Throws a RangeError.
        pub fn throw_range_error() -> Result<(), RangeError> {
            Err(RangeError("test range error".into()))
        }

        /// Throws a SyntaxError.
        pub fn throw_syntax_error() -> Result<(), SyntaxError> {
            Err(SyntaxError("test syntax error".into()))
        }

        /// Throws a TypeError from a String error.
        pub fn throw_string_error() -> Result<(), String> {
            Err("test string error".into())
        }

        /// Throws a DOMExceptionError.
        pub fn throw_dom_exception_error() -> Result<(), DOMExceptionError> {
            Err(DOMExceptionError::new(
                "NotFoundError",
                "test dom exception",
            ))
        }

        /// Succeeds.
        pub fn no_error() -> Result<i32, TypeError> {
            Ok(42)
        }
    }

    fn eval(code: &str) -> String {
        eval_with_setup(
            || {
                libstarling::register_builtins();
                core_runtime::runtime::register_global_initializer(|scope, global| unsafe {
                    test_error_globals::add_to_global(scope, global);
                });
            },
            code,
        )
    }

    #[test]
    fn type_error_thrown() {
        assert_eq!(
            eval("try { throwTypeError(); 'no' } catch(e) { (e instanceof TypeError) + ',' + e.message }"),
            "true,test type error"
        );
    }

    #[test]
    fn range_error_thrown() {
        assert_eq!(
            eval("try { throwRangeError(); 'no' } catch(e) { (e instanceof RangeError) + ',' + e.message }"),
            "true,test range error"
        );
    }

    #[test]
    fn syntax_error_thrown() {
        assert_eq!(
            eval("try { throwSyntaxError(); 'no' } catch(e) { (e instanceof SyntaxError) + ',' + e.message }"),
            "true,test syntax error"
        );
    }

    #[test]
    fn string_error_throws_type_error() {
        // String errors throw TypeError.
        assert_eq!(
            eval("try { throwStringError(); 'no' } catch(e) { (e instanceof TypeError) + ',' + e.message }"),
            "true,test string error"
        );
    }

    #[test]
    fn dom_exception_error_thrown() {
        assert_eq!(
            eval(
                "try { throwDomExceptionError(); 'no' } catch(e) { \
                    (e instanceof DOMException) + ',' + e.name + ',' + e.message }"
            ),
            "true,NotFoundError,test dom exception"
        );
    }

    #[test]
    fn success_returns_value() {
        assert_eq!(eval("noError()"), "42");
    }
}
