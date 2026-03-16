// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! JS value creation and inspection.
//!
//! This module provides ergonomic constructors for SpiderMonkey's [`JSVal`]
//! type. It does **not** define a new wrapper type — it works directly with the
//! existing [`JSVal`] and [`HandleValue`] / [`MutableHandleValue`] types.
//!
//! For type conversions between Rust and JS values, see [`crate::conversion`].
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
//! `val.to_int32()`).

use mozjs::gc::HandleFunction;
use mozjs::jsapi::{JSObject, JSString};
use mozjs::jsval::{
    BooleanValue, DoubleValue, Int32Value, JSVal, NullValue, ObjectOrNullValue, ObjectValue,
    PrivateValue, StringValue, UInt32Value, UndefinedValue,
};

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
