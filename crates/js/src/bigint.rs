// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! BigInt creation and conversion.
//!
//! BigInt is an ES2020 numeric type that can represent integers of arbitrary
//! precision. This module provides safe wrappers for creating BigInts from
//! various sources and converting them back.

use std::ptr::NonNull;

use crate::gc::scope::Scope;
use mozjs::gc::{Handle, HandleValue};
use mozjs::jsapi::{BigInt, JSString};
use mozjs::rust::wrappers2;

use super::error::JSError;

/// Create a `BigInt` from an `i64`.
pub fn from_i64<'s>(scope: &'s Scope<'_>, num: i64) -> Result<Handle<'s, *mut BigInt>, JSError> {
    let bi = unsafe { wrappers2::BigIntFromInt64(scope.cx_mut(), num) };
    NonNull::new(bi)
        .map(|p| scope.root_bigint(p))
        .ok_or(JSError)
}

/// Create a `BigInt` from a `u64`.
pub fn from_u64<'s>(scope: &'s Scope<'_>, num: u64) -> Result<Handle<'s, *mut BigInt>, JSError> {
    let bi = unsafe { wrappers2::BigIntFromUint64(scope.cx_mut(), num) };
    NonNull::new(bi)
        .map(|p| scope.root_bigint(p))
        .ok_or(JSError)
}

/// Create a `BigInt` from a `bool` (0n or 1n).
pub fn from_bool<'s>(scope: &'s Scope<'_>, b: bool) -> Result<Handle<'s, *mut BigInt>, JSError> {
    let bi = unsafe { wrappers2::BigIntFromBool(scope.cx_mut(), b) };
    NonNull::new(bi)
        .map(|p| scope.root_bigint(p))
        .ok_or(JSError)
}

/// Create a `BigInt` from an `f64`.
///
/// The number must be an integer value; otherwise a `RangeError` is thrown.
pub fn from_number<'s>(scope: &'s Scope<'_>, num: f64) -> Result<Handle<'s, *mut BigInt>, JSError> {
    let bi = unsafe { wrappers2::NumberToBigInt(scope.cx_mut(), num) };
    NonNull::new(bi)
        .map(|p| scope.root_bigint(p))
        .ok_or(JSError)
}

/// Convert a JS value to a `BigInt` (equivalent to `BigInt(val)` in JS).
pub fn to_bigint<'s>(
    scope: &'s Scope<'_>,
    val: HandleValue,
) -> Result<Handle<'s, *mut BigInt>, JSError> {
    let bi = unsafe { wrappers2::ToBigInt(scope.cx_mut(), val) };
    NonNull::new(bi)
        .map(|p| scope.root_bigint(p))
        .ok_or(JSError)
}

/// Convert a `BigInt` to a string in the given radix (2–36).
pub fn to_string<'s>(
    scope: &'s Scope<'_>,
    bi: Handle<*mut BigInt>,
    radix: u8,
) -> Result<Handle<'s, *mut JSString>, JSError> {
    let s = unsafe { wrappers2::BigIntToString(scope.cx_mut(), bi, radix) };
    NonNull::new(s).map(|p| scope.root_string(p)).ok_or(JSError)
}
