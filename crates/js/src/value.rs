// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! JS value creation, inspection, and conversion.
//!
//! This module provides ergonomic constructors and [`FromJSVal`] / [`TryFromJSVal`]
//! implementations for SpiderMonkey's [`JSVal`] type. It does **not** define a
//! new wrapper type — it works directly with the existing [`JSVal`] and
//! [`HandleValue`] / [`MutableHandleValue`] types.
//!
//! # Creating values
//!
//! ```ignore
//! use crate::value;
//!
//! let v = value::undefined();
//! let v = value::null();
//! let v = value::from_bool(true);
//! let v = value::from_i32(42);
//! let v = value::from_f64(3.14);
//! ```
//!
//! # Inspecting values
//!
//! The type-checking methods live directly on [`JSVal`] (e.g., `val.is_int32()`,
//! `val.to_int32()`). This module re-exports them for discoverability and adds
//! higher-level conversions via [`TryFromJSVal`].

use mozjs::gc::HandleFunction;
use mozjs::jsapi::{JSObject, JSString};
use mozjs::jsval::{
    BooleanValue, DoubleValue, Int32Value, JSVal, NullValue, ObjectOrNullValue, ObjectValue,
    PrivateValue, StringValue, UInt32Value, UndefinedValue,
};

use super::error::ConversionError;

/// Create an `undefined` value.
#[inline]
pub fn undefined() -> JSVal {
    UndefinedValue()
}

/// Create a `null` value.
#[inline]
pub fn null() -> JSVal {
    NullValue()
}

/// Create a boolean value.
#[inline]
pub fn from_bool(b: bool) -> JSVal {
    BooleanValue(b)
}

/// Create an `int32` value.
#[inline]
pub fn from_i32(i: i32) -> JSVal {
    Int32Value(i)
}

/// Create a numeric value from a `u32`.
///
/// If the value fits in an `int32`, an `int32` value is produced; otherwise a
/// `double`.
#[inline]
pub fn from_u32(u: u32) -> JSVal {
    UInt32Value(u)
}

/// Create a `double` value.
#[inline]
pub fn from_f64(f: f64) -> JSVal {
    DoubleValue(f)
}

/// Create an object value, or `null` if the pointer is null.
///
/// # Safety
///
/// `obj` must be either null or a valid, rooted `JSObject` pointer.
#[inline]
pub unsafe fn from_object_or_null(obj: *mut JSObject) -> JSVal {
    ObjectOrNullValue(obj)
}

/// Create an object value.
///
/// # Safety
///
/// `obj` must be a valid, rooted, non-null `JSObject` pointer.
#[inline]
pub unsafe fn from_object(obj: *mut JSObject) -> JSVal {
    ObjectValue(obj)
}

/// Create a value from a rooted function handle.
///
/// This is safe because the function is already rooted via the handle.
#[inline]
pub fn from_function(fun: HandleFunction) -> JSVal {
    // SAFETY: The function is rooted via the handle. JS_GetFunctionObject
    // returns a non-null pointer for any valid JSFunction.
    unsafe { ObjectValue(mozjs::jsapi::JS_GetFunctionObject(fun.get())) }
}

/// Create a private value from a pointer.
///
/// Private values store opaque pointers in JS values. The pointer is not
/// traced by the GC — this is for storing Rust data, not JS objects.
///
/// # Safety
///
/// `ptr` can be any pointer; it will be stored opaquely. The caller must
/// manage the pointer's lifetime independently of the GC.
#[inline]
pub unsafe fn from_private(ptr: *const std::ffi::c_void) -> JSVal {
    PrivateValue(ptr)
}

/// Create a JS string value from a raw `JSString` pointer.
///
/// # Safety
///
/// `s` must be a valid, non-null, rooted `JSString` pointer.
#[inline]
pub unsafe fn from_string_raw(s: *mut JSString) -> JSVal {
    StringValue(&*s)
}

// ---------------------------------------------------------------------------
// Conversion traits for JSVal
// ---------------------------------------------------------------------------

/// Extension trait for converting Rust types into [`JSVal`].
///
/// This mirrors `From<T>` but avoids orphan rules since `JSVal` is defined in
/// another crate.
///
/// # Examples
///
/// ```ignore
/// use crate::value::IntoJSVal;
/// let v: JSVal = true.into_jsval();
/// let v: JSVal = 42i32.into_jsval();
/// ```
pub trait IntoJSVal {
    /// Convert `self` into a [`JSVal`].
    fn into_jsval(self) -> JSVal;
}

impl IntoJSVal for bool {
    #[inline]
    fn into_jsval(self) -> JSVal {
        from_bool(self)
    }
}

impl IntoJSVal for i32 {
    #[inline]
    fn into_jsval(self) -> JSVal {
        from_i32(self)
    }
}

impl IntoJSVal for u32 {
    #[inline]
    fn into_jsval(self) -> JSVal {
        from_u32(self)
    }
}

impl IntoJSVal for f64 {
    #[inline]
    fn into_jsval(self) -> JSVal {
        from_f64(self)
    }
}

impl IntoJSVal for () {
    /// `()` maps to `undefined`.
    #[inline]
    fn into_jsval(self) -> JSVal {
        undefined()
    }
}

impl<T: IntoJSVal> IntoJSVal for Option<T> {
    /// `None` maps to `null`, `Some(v)` delegates to `v.into_jsval()`.
    #[inline]
    fn into_jsval(self) -> JSVal {
        match self {
            Some(v) => v.into_jsval(),
            None => null(),
        }
    }
}

impl IntoJSVal for i8 {
    #[inline]
    fn into_jsval(self) -> JSVal {
        from_i32(self as i32)
    }
}

impl IntoJSVal for i16 {
    #[inline]
    fn into_jsval(self) -> JSVal {
        from_i32(self as i32)
    }
}

impl IntoJSVal for u8 {
    #[inline]
    fn into_jsval(self) -> JSVal {
        from_i32(self as i32)
    }
}

impl IntoJSVal for u16 {
    #[inline]
    fn into_jsval(self) -> JSVal {
        from_i32(self as i32)
    }
}

impl IntoJSVal for f32 {
    #[inline]
    fn into_jsval(self) -> JSVal {
        from_f64(self as f64)
    }
}

// ---------------------------------------------------------------------------
// TryFromJSVal — extract typed data from a JSVal
// ---------------------------------------------------------------------------

/// Extension trait for extracting Rust types from a [`JSVal`].
///
/// This mirrors `TryFrom<JSVal>` but avoids orphan rules.
pub trait TryFromJSVal: Sized {
    /// Try to extract a value of type `Self` from a [`JSVal`].
    fn try_from_jsval(val: JSVal) -> Result<Self, ConversionError>;
}

impl TryFromJSVal for bool {
    /// Extract a `bool` from a JS value.
    ///
    /// Fails if the value is not a boolean.
    #[inline]
    fn try_from_jsval(val: JSVal) -> Result<Self, ConversionError> {
        if val.is_boolean() {
            Ok(val.to_boolean())
        } else {
            Err(ConversionError("expected a boolean value"))
        }
    }
}

impl TryFromJSVal for i32 {
    /// Extract an `i32` from a JS value.
    ///
    /// Fails if the value is not an `int32`.
    #[inline]
    fn try_from_jsval(val: JSVal) -> Result<Self, ConversionError> {
        if val.is_int32() {
            Ok(val.to_int32())
        } else {
            Err(ConversionError("expected an int32 value"))
        }
    }
}

impl TryFromJSVal for u32 {
    /// Extract a `u32` from a JS value.
    ///
    /// Succeeds for `int32` values >= 0.
    #[inline]
    fn try_from_jsval(val: JSVal) -> Result<Self, ConversionError> {
        if val.is_int32() {
            let i = val.to_int32();
            if i >= 0 {
                Ok(i as u32)
            } else {
                Err(ConversionError("expected a non-negative int32 value"))
            }
        } else {
            Err(ConversionError("expected an int32 value"))
        }
    }
}

impl TryFromJSVal for f64 {
    /// Extract an `f64` from a JS value.
    ///
    /// Succeeds for both `int32` and `double` values.
    #[inline]
    fn try_from_jsval(val: JSVal) -> Result<Self, ConversionError> {
        if val.is_double() {
            Ok(val.to_double())
        } else if val.is_int32() {
            Ok(val.to_int32() as f64)
        } else {
            Err(ConversionError("expected a numeric value"))
        }
    }
}

impl TryFromJSVal for *mut JSObject {
    /// Extract a `*mut JSObject` from a JS value.
    ///
    /// Succeeds if the value is an object (including null-object).
    #[inline]
    fn try_from_jsval(val: JSVal) -> Result<Self, ConversionError> {
        if val.is_object() {
            Ok(val.to_object())
        } else if val.is_null() {
            Ok(std::ptr::null_mut())
        } else {
            Err(ConversionError("expected an object value"))
        }
    }
}
