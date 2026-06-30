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

use std::hint::cold_path;

use mozjs::gc::{Handle, HandleFunction};
use mozjs::jsapi::{BigInt, JSObject, JSString};
use mozjs::jsval::{
    BigIntValue, BooleanValue, DoubleValue, Int32Value, JSVal, NullValue, ObjectOrNullValue,
    ObjectValue, PrivateValue, StringValue, UInt32Value, UndefinedValue,
};

pub use mozjs::jsval::JSVal as Value;

use crate::conversion::ToJSVal;
use crate::error::ExnThrown;

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
///
/// NaNs are canonicalized first (matching `JS::CanonicalizedDoubleValue`):
/// the engine reserves non-canonical NaN bit patterns for value tagging, so
/// a payload-carrying NaN, e.g. read from raw `Float64Array` bits, would
/// otherwise fail `DoubleValue`'s bit-pattern assertion and abort.
#[inline]
pub fn from_f64(f: f64) -> JSVal {
    let f = if f.is_nan() {
        cold_path();
        // SpiderMonkey's canonical NaN: positive quiet NaN with zero payload.
        const CANONICAL_NAN: f64 = f64::from_bits(0x7FF8_0000_0000_0000);
        CANONICAL_NAN
    } else {
        f
    };
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

/// Create a value from a rooted `BigInt` handle.
#[inline]
pub fn from_bigint(bi: Handle<*mut BigInt>) -> JSVal {
    // SAFETY: the `BigInt` is rooted via the handle, so the reference is valid
    // for the duration of this call.
    unsafe { BigIntValue(&*bi.get()) }
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
// Rooted HandleValueArray backing
// ---------------------------------------------------------------------------

/// Rooted backing buffer for a [`HandleValueArray`].
///
/// `Function::call*` and `Array::with_contents` take `&[HandleValue]` and
/// copy the values in here. The copies are traced for as long as the rooted
/// guard lives.
///
/// ```ignore
/// let mut args_root = ValueArrayRooter::new(args);
/// let args = args_root.root(scope);
/// some_jsapi_call(scope.cx_mut(), &args.handles());
/// ```
pub(crate) struct ValueArrayRooter(mozjs::gc::CustomAutoRooter<Vec<JSVal>>);

impl<'s> ValueArrayRooter {
    pub(crate) fn new(
        scope: &'s crate::gc::scope::Scope<'_>,
        values: &[impl ToJSVal<'s>],
    ) -> Result<Self, ExnThrown> {
        let mut raw_values: Vec<Value> = Vec::with_capacity(values.len());
        for v in values {
            raw_values.push(v.to_jsval_raw_trowing(scope)?);
        }
        Ok(Self(mozjs::gc::CustomAutoRooter::new(raw_values)))
    }

    pub(crate) fn root<'a>(
        &'a mut self,
        scope: &crate::gc::scope::Scope<'_>,
    ) -> RootedValueArray<'a> {
        // SAFETY: adding the rooter to the root stack performs no GC.
        RootedValueArray(self.0.root(unsafe { scope.raw_cx_no_gc() }))
    }
}

/// A rooted view over a [`ValueArrayRooter`]'s values; see there.
pub(crate) struct RootedValueArray<'a>(mozjs::gc::CustomAutoRooterGuard<'a, Vec<JSVal>>);

impl RootedValueArray<'_> {
    pub(crate) fn handles(&self) -> mozjs::jsapi::HandleValueArray {
        mozjs::jsapi::HandleValueArray {
            length_: self.0.len(),
            elements_: self.0.as_ptr(),
        }
    }
}
