// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Type conversion and introspection operations (ECMAScript abstract operations).
//!
//! Includes the ECMAScript abstract coercion operations (`ToNumber`,
//! `ToBoolean`, `ToString`, `ToInt32`, `ToUint32`, etc.) as well as type
//! introspection helpers (`typeof`, `GetBuiltinClass`, `HasInstance`,
//! `ToPrimitive`).
//!
//! For type-safe extraction from `JSVal` without coercion, see
//! [`super::value::TryFromJSVal`].

use std::ptr::NonNull;

use crate::gc::scope::Scope;
use mozjs::gc::{Handle, HandleObject};
use mozjs::jsapi::{ESClass, JSString, JSType, Value};
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2;
use mozjs::rust::HandleValue;

use super::error::JSError;

/// Convert a JS value to a `u32` (`ToUint32`).
///
/// May trigger type coercion and can throw.
pub fn to_uint32(scope: &Scope<'_>, v: HandleValue) -> Result<u32, JSError> {
    // Fast path: non-negative int32.
    let val = unsafe { *v.ptr.as_ptr() };
    if val.is_int32() {
        let i = val.to_int32() as i64;
        return Ok(i as u32);
    }

    let mut out: u32 = 0;
    let ok = unsafe { wrappers2::ToUint32Slow(scope.cx_mut(), v, &mut out) };
    JSError::check(ok)?;
    Ok(out)
}

/// Convert a JS value to an `i64` (`ToInt64`).
///
/// May trigger type coercion and can throw.
pub fn to_int64(scope: &Scope<'_>, v: HandleValue) -> Result<i64, JSError> {
    // Fast path: already an int32.
    let val = unsafe { *v.ptr.as_ptr() };
    if val.is_int32() {
        return Ok(val.to_int32() as i64);
    }

    let mut out: i64 = 0;
    let ok = unsafe { wrappers2::ToInt64Slow(scope.cx_mut(), v, &mut out) };
    JSError::check(ok)?;
    Ok(out)
}

/// Convert a JS value to a `u64` (`ToUint64`).
///
/// May trigger type coercion and can throw.
pub fn to_uint64(scope: &Scope<'_>, v: HandleValue) -> Result<u64, JSError> {
    // Fast path: non-negative int32.
    let val = unsafe { *v.ptr.as_ptr() };
    if val.is_int32() {
        let i = val.to_int32() as i64;
        return Ok(i as u64);
    }

    let mut out: u64 = 0;
    let ok = unsafe { wrappers2::ToUint64Slow(scope.cx_mut(), v, &mut out) };
    JSError::check(ok)?;
    Ok(out)
}

/// Convert a JS value to a string (`ToString`).
///
/// May trigger type coercion and can throw.
pub fn to_string<'s>(
    scope: &'s Scope<'_>,
    v: HandleValue,
) -> Result<Handle<'s, *mut JSString>, JSError> {
    // Fast path: already a string.
    let val = unsafe { *v.ptr.as_ptr() };
    if val.is_string() {
        // SAFETY: is_string() guarantees to_string() returns a valid pointer.
        return Ok(scope.root_string(unsafe { NonNull::new_unchecked(val.to_string()) }));
    }

    let s = unsafe { wrappers2::ToStringSlow(scope.cx_mut(), v) };
    NonNull::new(s).map(|p| scope.root_string(p)).ok_or(JSError)
}

// ---------------------------------------------------------------------------
// Type introspection
// ---------------------------------------------------------------------------

/// Get the `typeof` of a JS value.
///
/// Corresponds to the ECMAScript `typeof` operator. Returns a [`JSType`]
/// discriminant such as `JSType::JSTYPE_NUMBER`, `JSType::JSTYPE_STRING`, etc.
pub fn typeof_value(scope: &Scope<'_>, v: HandleValue) -> JSType {
    unsafe { wrappers2::JS_TypeOfValue(scope.cx(), v.into()) }
}

/// Get the built-in ECMAScript class of an object (e.g., `Array`, `Date`,
/// `RegExp`, `Map`, …).
///
/// Returns [`ESClass`] which discriminates standard built-in classes.
pub fn get_builtin_class(scope: &Scope<'_>, obj: HandleObject) -> Result<ESClass, JSError> {
    let mut cls = ESClass::Other;
    let ok = unsafe { wrappers2::GetBuiltinClass(scope.cx_mut(), obj, &mut cls) };
    JSError::check(ok)?;
    Ok(cls)
}

/// Check whether `v instanceof obj` (`HasInstance`).
///
/// `obj` must be callable or have a `[Symbol.hasInstance]` method; otherwise
/// an exception is thrown.
pub fn has_instance(scope: &Scope<'_>, obj: HandleObject, v: HandleValue) -> Result<bool, JSError> {
    let mut result = false;
    let ok = unsafe { wrappers2::JS_HasInstance(scope.cx_mut(), obj, v.into(), &mut result) };
    JSError::check(ok)?;
    Ok(result)
}

/// Convert a JS value to its source representation (`uneval`).
///
/// Returns a `JSString` containing the source text representation of the
/// value. The result must be rooted immediately.
pub fn value_to_source<'s>(
    scope: &'s Scope<'_>,
    v: HandleValue,
) -> Result<Handle<'s, *mut JSString>, JSError> {
    let s = unsafe { wrappers2::JS_ValueToSource(scope.cx_mut(), v.into()) };
    NonNull::new(s).map(|p| scope.root_string(p)).ok_or(JSError)
}

/// Convert an object to a primitive value (`ToPrimitive`).
///
/// `hint` specifies the preferred type:
/// - `JSType::JSTYPE_STRING` — prefer string
/// - `JSType::JSTYPE_NUMBER` — prefer number
/// - `JSType::JSTYPE_UNDEFINED` — no preference
pub fn to_primitive(scope: &Scope<'_>, obj: HandleObject, hint: JSType) -> Result<Value, JSError> {
    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut vp = UndefinedValue());
    let ok = unsafe { wrappers2::ToPrimitive(scope.cx_mut(), obj, hint, vp.handle_mut().into()) };
    JSError::check(ok)?;
    Ok(vp.get())
}
