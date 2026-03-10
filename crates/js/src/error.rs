// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Error types for the `mozjs::js` API.
//!
//! This module provides:
//!
//! - [`JSError`] — a lightweight unit struct indicating that a JavaScript
//!   exception is pending on the context. Used as the error type in
//!   `Result<T, JSError>` throughout the safe API.
//! - [`CapturedError`] — rich error details captured from a pending exception
//!   via [`JSError::capture`]. Includes the message, source location, stack
//!   trace, and (when available) source line and column range.
//! - [`ConversionError`] — error type for pure Rust type-extraction failures
//!   (e.g., extracting an `i32` from a non-integer `JSVal`).
//!
//! # Throwing errors
//!
//! Use [`throw_type_error`], [`throw_range_error`], or [`throw_internal_error`]
//! to set a pending exception on the context and return `Err(JSError)`.

use std::ffi::{CStr, CString};
use std::fmt;
use std::ptr;

use crate::gc::scope::Scope;
use crate::Object;
use mozjs::context::JSContext;
use mozjs::jsapi::{
    JSErrorFormatString, JSExnType, JSString, JS_ClearPendingException, JS_GetPendingException,
    JS_IsExceptionPending, JS_ReportErrorNumberUTF8, StackFormat,
};
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2;

/// A lightweight marker indicating that a JavaScript exception is pending on
/// the context.
///
/// This error type carries no data — it simply signals that a SpiderMonkey
/// operation failed and an exception is pending. Use [`JSError::capture`] to
/// extract details into a [`CapturedError`].
///
/// # Why a unit struct?
///
/// Most code only needs to propagate errors with `?`. The heavy lifting of
/// extracting error messages, filenames, and stack traces is deferred to the
/// few call sites that actually need it (e.g., error reporters, REPL shells).
/// This keeps `Result<T, JSError>` zero-cost in the common case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JSError;

impl JSError {
    /// Check whether a SpiderMonkey call succeeded and convert its boolean
    /// result into `Result<(), JSError>`.
    ///
    /// If `ok` is `true`, returns `Ok(())`. Otherwise returns `Err(JSError)`.
    ///
    /// This is the primary bridge between SpiderMonkey's `bool`-returning C++
    /// API and Rust's `Result` type.
    #[inline]
    pub fn check(ok: bool) -> Result<(), JSError> {
        if ok {
            Ok(())
        } else {
            Err(JSError)
        }
    }

    /// Capture the pending exception from the context, clearing it.
    ///
    /// Returns a [`CapturedError`] with whatever information can be extracted
    /// from the pending exception. If no exception is pending, returns a
    /// [`CapturedError`] with default (empty) fields.
    // TODO: use CapturedJSStack here.
    pub fn capture(scope: &Scope<'_>) -> CapturedError {
        // SAFETY: all FFI calls here require a realm to be entered, which is
        // guaranteed by the `Scope` parameter.
        unsafe {
            let raw = scope.cx_mut().raw_cx();
            if !JS_IsExceptionPending(raw) {
                return CapturedError::default();
            }

            rooted!(in(raw) let mut exc_val = UndefinedValue());
            if !JS_GetPendingException(raw, exc_val.handle_mut().into()) {
                return CapturedError::default();
            }
            JS_ClearPendingException(raw);

            // Try to extract the error report if the exception is an Error object.
            let exc = exc_val.get();
            if exc.is_object() {
                let exc_obj = Object::from_value(scope, exc).unwrap();
                let report = mozjs::jsapi::JS_ErrorFromException(raw, exc_obj.handle().into());

                // Try to extract the stack trace from the error object.
                let maybe_stack =
                    Object::from_raw(scope, wrappers2::ExceptionStackOrNull(exc_obj.handle()));
                let stack = if let Some(stack) = maybe_stack {
                    rooted!(in(raw) let mut stack_str: *mut JSString = ptr::null_mut());
                    let ok = mozjs::rust::wrappers::BuildStackString(
                        raw,
                        ptr::null_mut(),
                        stack.handle(),
                        stack_str.handle_mut(),
                        0,
                        StackFormat::Default,
                    );
                    if ok && !stack_str.get().is_null() {
                        // Convert the JS stack string to a Rust String via
                        // the glue callback, using wrappers (not wrappers2)
                        // since we only have a raw context pointer.
                        use std::cell::Cell;
                        thread_local! {
                            static STACK_RESULT: Cell<Option<String>> =
                                const { Cell::new(None) };
                        }
                        unsafe extern "C" fn stack_cb(encoded: *const std::ffi::c_char) {
                            if !encoded.is_null() {
                                let cstr = CStr::from_ptr(encoded);
                                STACK_RESULT
                                    .with(|r| r.set(Some(cstr.to_string_lossy().into_owned())));
                            }
                        }
                        mozjs::rust::wrappers::EncodeStringToUTF8(
                            raw,
                            stack_str.handle(),
                            stack_cb,
                        );
                        STACK_RESULT.with(|r| r.take())
                    } else {
                        None
                    }
                } else {
                    None
                };

                if !report.is_null() {
                    let report = &*report;
                    let msg_ptr = report._base.message_.data_;
                    let message = if msg_ptr.is_null() {
                        None
                    } else {
                        Some(CStr::from_ptr(msg_ptr).to_string_lossy().into_owned())
                    };
                    let fn_ptr = report._base.filename.data_;
                    let filename = if fn_ptr.is_null() {
                        None
                    } else {
                        Some(CStr::from_ptr(fn_ptr).to_string_lossy().into_owned())
                    };
                    return CapturedError {
                        message,
                        filename,
                        lineno: report._base.lineno,
                        column: report._base.column._base,
                        stack,
                    };
                }
            }

            // The exception was not an Error object; return a generic capture.
            CapturedError {
                message: Some("(non-Error exception)".into()),
                filename: None,
                lineno: 0,
                column: 0,
                stack: None,
            }
        } // unsafe
    }
}

impl fmt::Display for JSError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JavaScript exception pending")
    }
}

impl std::error::Error for JSError {}

impl From<()> for JSError {
    /// Convert the unit error from the existing `Result<T, ()>` API.
    fn from((): ()) -> Self {
        JSError
    }
}

/// Captured details from a JavaScript exception.
///
/// Created by [`JSError::capture`]. Contains the message, source location,
/// and stack trace extracted from the pending exception.
#[derive(Debug, Clone, Default)]
pub struct CapturedError {
    /// The error message, if available.
    pub message: Option<String>,
    /// The source filename, if available.
    pub filename: Option<String>,
    /// The 1-based line number in the source, or 0 if unknown.
    pub lineno: u32,
    /// The 0-based column number in the source, or 0 if unknown.
    pub column: u32,
    /// The stack trace string, if available.
    pub stack: Option<String>,
}

impl fmt::Display for CapturedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(msg) = &self.message {
            write!(f, "{msg}")?;
        } else {
            write!(f, "JavaScript exception")?;
        }
        if let Some(file) = &self.filename {
            write!(f, " at {file}:{}", self.lineno)?;
        }
        if let Some(s) = &self.stack {
            write!(f, "\n{s}")?;
        }
        Ok(())
    }
}

impl std::error::Error for CapturedError {}

/// Error type for type-conversion mismatches when extracting Rust values from
/// a [`JSVal`](mozjs::jsval::JSVal).
///
/// This is a pure Rust error — no JavaScript exception is involved.
#[derive(Debug, Clone)]
pub struct ConversionError(pub &'static str);

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ConversionError {}

// ---------------------------------------------------------------------------
// Throw helpers
// ---------------------------------------------------------------------------

/// Format string used to throw JavaScript errors from Rust.
static ERROR_FORMAT_STRING: &CStr = c"{0}";

/// Format string struct for `TypeError`.
static mut TYPE_ERROR_FMT: JSErrorFormatString = JSErrorFormatString {
    name: c"RUSTMSG_TYPE_ERROR".as_ptr(),
    format: ERROR_FORMAT_STRING.as_ptr(),
    argCount: 1,
    exnType: JSExnType::JSEXN_TYPEERR as i16,
};

/// Format string struct for `RangeError`.
static mut RANGE_ERROR_FMT: JSErrorFormatString = JSErrorFormatString {
    name: c"RUSTMSG_RANGE_ERROR".as_ptr(),
    format: ERROR_FORMAT_STRING.as_ptr(),
    argCount: 1,
    exnType: JSExnType::JSEXN_RANGEERR as i16,
};

/// Format string struct for `InternalError`.
static mut INTERNAL_ERROR_FMT: JSErrorFormatString = JSErrorFormatString {
    name: c"RUSTMSG_INTERNAL_ERROR".as_ptr(),
    format: ERROR_FORMAT_STRING.as_ptr(),
    argCount: 1,
    exnType: JSExnType::JSEXN_INTERNALERR as i16,
};

/// Format string struct for `SyntaxError`.
static mut SYNTAX_ERROR_FMT: JSErrorFormatString = JSErrorFormatString {
    name: c"RUSTMSG_SYNTAX_ERROR".as_ptr(),
    format: ERROR_FORMAT_STRING.as_ptr(),
    argCount: 1,
    exnType: JSExnType::JSEXN_SYNTAXERR as i16,
};

/// Callback for [`JS_ReportErrorNumberUTF8`].
unsafe extern "C" fn get_error_message(
    _user_ref: *mut std::os::raw::c_void,
    error_number: std::os::raw::c_uint,
) -> *const JSErrorFormatString {
    let num: JSExnType = std::mem::transmute(error_number);
    match num {
        JSExnType::JSEXN_TYPEERR => &raw const TYPE_ERROR_FMT,
        JSExnType::JSEXN_RANGEERR => &raw const RANGE_ERROR_FMT,
        JSExnType::JSEXN_INTERNALERR => &raw const INTERNAL_ERROR_FMT,
        JSExnType::JSEXN_SYNTAXERR => &raw const SYNTAX_ERROR_FMT,
        _ => panic!("Bad error number: {error_number}"),
    }
}

/// Throw a `TypeError` with the given message and return `JSError`.
///
/// The message must be a valid C string (no interior NUL bytes).
///
/// # Safety
///
/// A realm must be entered on `cx`.
pub unsafe fn throw_type_error(cx: &mut JSContext, error: &CStr) -> JSError {
    throw_js_error(cx, error, JSExnType::JSEXN_TYPEERR as u32);
    JSError
}

/// Throw a `RangeError` with the given message and return `JSError`.
///
/// # Safety
///
/// A realm must be entered on `cx`.
pub unsafe fn throw_range_error(cx: &mut JSContext, error: &CStr) -> JSError {
    throw_js_error(cx, error, JSExnType::JSEXN_RANGEERR as u32);
    JSError
}

/// Throw an `InternalError` with the given message and return `JSError`.
///
/// # Safety
///
/// A realm must be entered on `cx`.
pub unsafe fn throw_internal_error(cx: &mut JSContext, error: &CStr) -> JSError {
    throw_js_error(cx, error, JSExnType::JSEXN_INTERNALERR as u32);
    JSError
}

/// Throw a `SyntaxError` with the given message and return `JSError`.
///
/// # Safety
///
/// A realm must be entered on `cx`.
pub unsafe fn throw_syntax_error(cx: &mut JSContext, error: &CStr) -> JSError {
    throw_js_error(cx, error, JSExnType::JSEXN_SYNTAXERR as u32);
    JSError
}

unsafe fn throw_js_error(cx: &mut JSContext, error: &CStr, error_number: u32) {
    JS_ReportErrorNumberUTF8(
        cx.raw_cx(),
        Some(get_error_message),
        std::ptr::null_mut(),
        error_number,
        error.as_ptr(),
    );
}

/// Report a simple ASCII error message, setting a pending exception.
///
/// This is a lightweight alternative to the typed error functions above.
/// The message is reported as a generic `Error` (not `TypeError`, etc.).
///
/// # Safety
///
/// A realm must be entered on `cx`.
pub unsafe fn report_error_ascii(cx: &mut JSContext, msg: &CStr) {
    mozjs::rust::wrappers2::ReportErrorASCII(cx, msg.as_ptr());
}

// ---------------------------------------------------------------------------
// Newtype error wrappers
// ---------------------------------------------------------------------------

/// A JavaScript `TypeError` with a message.
///
/// Use this as the `Err` type in `Result<T, TypeError>` to throw a
/// `TypeError` when a function returns an error. The proc macro system
/// (`#[jsmethods]`, `#[jsglobals]`, etc.) dispatches via the
/// [`ThrowException`](core_runtime::class::ThrowException) trait to call the
/// appropriate SpiderMonkey error API.
///
/// # Example
///
/// ```rust,ignore
/// use js::error::TypeError;
///
/// #[method]
/// fn parse(input: String) -> Result<i32, TypeError> {
///     input.parse().map_err(|_| TypeError(format!("invalid integer: {input}")))
/// }
/// ```
#[derive(Debug, Clone)]
pub struct TypeError(pub String);

impl TypeError {
    /// Throw this error as a pending JavaScript `TypeError` exception.
    ///
    /// # Safety
    ///
    /// A realm must be entered on the scope's context.
    pub unsafe fn throw(&self, scope: &Scope<'_>) {
        let c_msg =
            CString::new(self.0.as_str()).unwrap_or_else(|_| CString::new("type error").unwrap());
        throw_type_error(scope.cx_mut(), &c_msg);
    }
}

impl fmt::Display for TypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TypeError: {}", self.0)
    }
}

impl std::error::Error for TypeError {}

/// A JavaScript `RangeError` with a message.
///
/// See [`TypeError`] for usage patterns.
#[derive(Debug, Clone)]
pub struct RangeError(pub String);

impl RangeError {
    /// Throw this error as a pending JavaScript `RangeError` exception.
    ///
    /// # Safety
    ///
    /// A realm must be entered on the scope's context.
    pub unsafe fn throw(&self, scope: &Scope<'_>) {
        let c_msg =
            CString::new(self.0.as_str()).unwrap_or_else(|_| CString::new("range error").unwrap());
        throw_range_error(scope.cx_mut(), &c_msg);
    }
}

impl fmt::Display for RangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RangeError: {}", self.0)
    }
}

impl std::error::Error for RangeError {}

/// A JavaScript `SyntaxError` with a message.
///
/// See [`TypeError`] for usage patterns.
#[derive(Debug, Clone)]
pub struct SyntaxError(pub String);

impl SyntaxError {
    /// Throw this error as a pending JavaScript `SyntaxError` exception.
    ///
    /// # Safety
    ///
    /// A realm must be entered on the scope's context.
    pub unsafe fn throw(&self, scope: &Scope<'_>) {
        let c_msg =
            CString::new(self.0.as_str()).unwrap_or_else(|_| CString::new("syntax error").unwrap());
        throw_syntax_error(scope.cx_mut(), &c_msg);
    }
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SyntaxError: {}", self.0)
    }
}

impl std::error::Error for SyntaxError {}

// ---------------------------------------------------------------------------
// ThrowException trait — typed error dispatch for proc macros
// ---------------------------------------------------------------------------

/// Trait for converting a Rust error value into a pending JavaScript exception.
///
/// The `#[jsmethods]`, `#[jsglobals]`, and `#[jsmodule]` proc macros use this
/// trait to dispatch `Err` values to the correct SpiderMonkey error API. For
/// example, returning `Err(TypeError("bad argument".into()))` throws a
/// JavaScript `TypeError`, while returning `Err(String)` throws a `TypeError`.
///
/// # Implementing
///
/// Custom error types (like `DOMExceptionError`) can implement this trait to
/// throw domain-specific exception objects.
///
/// # Safety
///
/// Implementations must set a pending exception on the context. The scope
/// guarantees that a realm is entered.
pub trait ThrowException {
    /// Set a pending JavaScript exception from this error value.
    ///
    /// After this call, a JS exception must be pending on the context.
    ///
    /// # Safety
    ///
    /// A realm must be entered on the scope's context.
    unsafe fn throw(self, scope: &Scope<'_>);
}

impl ThrowException for String {
    /// Throw a `TypeError` with this string as the message.
    unsafe fn throw(self, scope: &Scope<'_>) {
        throw_error(scope, &self);
    }
}

impl ThrowException for TypeError {
    unsafe fn throw(self, scope: &Scope<'_>) {
        TypeError::throw(&self, scope);
    }
}

impl ThrowException for RangeError {
    unsafe fn throw(self, scope: &Scope<'_>) {
        RangeError::throw(&self, scope);
    }
}

impl ThrowException for SyntaxError {
    unsafe fn throw(self, scope: &Scope<'_>) {
        SyntaxError::throw(&self, scope);
    }
}

impl ThrowException for JSError {
    /// No-op: `JSError` indicates an exception is already pending on the
    /// context, so there is nothing additional to throw.
    unsafe fn throw(self, _scope: &Scope<'_>) {}
}

// ---------------------------------------------------------------------------
// throw_error — convenience helper for proc macro codegen
// ---------------------------------------------------------------------------

/// Throw a `TypeError` with the given message.
///
/// This is used by the `#[jsmethods]` macro to convert Rust `Err` values
/// into JS exceptions. The error message is converted to a `CString`.
///
/// # Safety
///
/// A realm must be entered on the scope's context.
pub unsafe fn throw_error(scope: &Scope<'_>, msg: &str) {
    let c_msg = CString::new(msg).unwrap_or_else(|_| CString::new("unknown error").unwrap());
    throw_type_error(scope.cx_mut(), &c_msg);
}

// ---------------------------------------------------------------------------
// capture_stack_from_error
// ---------------------------------------------------------------------------

/// Capture the current call stack from a temporary `Error` object and set it
/// as the `stack` property on `obj`.
///
/// This creates `new Error()` to let SpiderMonkey capture the stack trace,
/// reads the resulting `stack` property, and copies it onto `obj`.
///
/// Used by error-like classes (e.g. DOMException, or any class with
/// `js_proto = "Error"`) that need `[[ErrorData]]` behavior.
///
/// Silently returns without setting `stack` if any step fails.
///
/// # Safety
///
/// Must be called within a valid scope with an active realm.
pub unsafe fn capture_stack_from_error(scope: &Scope<'_>, obj: &Object<'_>) {
    use crate::class_spec::JSProtoKey;
    use crate::native::HandleValueArray;

    // Create `new Error()` to capture the current stack.
    let error_ctor = match crate::class::get_class_object(scope, JSProtoKey::JSProto_Error) {
        Ok(ctor) => ctor,
        Err(_) => return,
    };

    let ctor_val = scope.root_value(unsafe { crate::value::from_object(error_ctor.get()) });
    let empty_args = HandleValueArray {
        length_: 0,
        elements_: ptr::null(),
    };

    let error_obj = match crate::function::construct(scope, ctor_val, &empty_args) {
        Ok(obj) => obj,
        Err(_) => return,
    };

    // Read the `stack` property from the Error object.
    let stack_val = match error_obj.get_property(scope, c"stack") {
        Ok(v) => v,
        Err(_) => return,
    };

    // Set the stack as an own property on the target object.
    let stack_handle = scope.root_value(stack_val);
    let _ = obj.set_property(scope, c"stack", stack_handle);
}
