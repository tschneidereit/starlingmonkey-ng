// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! JS string creation, encoding, comparison, and atom operations.
//!
//! SpiderMonkey strings are GC-managed and encoded internally as either Latin1
//! or two-byte (UTF-16). This module provides safe wrappers for creating
//! strings from Rust types and extracting their content.

use std::cell::Cell;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr::NonNull;

use crate::gc::scope::Scope;
use mozjs::gc::{Handle, HandleString};
use mozjs::jsapi::{JSLinearString, JSString};
use mozjs::rust::wrappers2;

use super::error::JSError;

// ---------------------------------------------------------------------------
// String creation
// ---------------------------------------------------------------------------

/// Create a new JS string from a UTF-8 Rust `&str`.
pub fn from_str<'s>(scope: &'s Scope<'_>, s: &str) -> Result<Handle<'s, *mut JSString>, JSError> {
    let js_str = unsafe {
        wrappers2::JS_NewStringCopyN(scope.cx_mut(), s.as_ptr() as *const c_char, s.len())
    };
    NonNull::new(js_str)
        .map(|p| scope.root_string(p))
        .ok_or(JSError)
}

/// Create a new JS string from a null-terminated C string.
pub fn from_cstr<'s>(scope: &'s Scope<'_>, s: &CStr) -> Result<Handle<'s, *mut JSString>, JSError> {
    let js_str = unsafe { wrappers2::JS_NewStringCopyZ(scope.cx_mut(), s.as_ptr()) };
    NonNull::new(js_str)
        .map(|p| scope.root_string(p))
        .ok_or(JSError)
}

/// Create a new JS string from a UTF-16 slice.
pub fn from_utf16<'s>(
    scope: &'s Scope<'_>,
    s: &[u16],
) -> Result<Handle<'s, *mut JSString>, JSError> {
    let js_str = unsafe { wrappers2::JS_NewUCStringCopyN(scope.cx_mut(), s.as_ptr(), s.len()) };
    NonNull::new(js_str)
        .map(|p| scope.root_string(p))
        .ok_or(JSError)
}

/// Get the empty string for this context.
///
/// The returned handle is rooted in the scope. (The empty string is also
/// permanently rooted by the runtime.)
pub fn empty<'s>(scope: &'s Scope<'_>) -> Handle<'s, *mut JSString> {
    let ptr = unsafe { wrappers2::JS_GetEmptyString(scope.cx()) };
    // SAFETY: The empty string is always present in a valid runtime.
    let nn = unsafe { NonNull::new_unchecked(ptr) };
    scope.root_string(nn)
}

// ---------------------------------------------------------------------------
// String encoding / extraction
// ---------------------------------------------------------------------------

/// Encode a JS string to UTF-8, returning an owned [`String`].
///
/// This allocates a Rust string and copies the content.
pub fn to_utf8(scope: &Scope<'_>, s: HandleString) -> Result<String, JSError> {
    thread_local! {
        static RESULT: Cell<Option<String>> = const { Cell::new(None) };
    }
    unsafe extern "C" fn cb(encoded: *const c_char) {
        if !encoded.is_null() {
            let cstr = CStr::from_ptr(encoded);
            RESULT.with(|r| r.set(Some(cstr.to_string_lossy().into_owned())));
        }
    }
    unsafe { wrappers2::EncodeStringToUTF8(scope.cx_mut(), s, cb) };
    RESULT.with(|r| r.take()).ok_or(JSError)
}

/// Get a single character at the given index.
pub fn char_at(scope: &Scope<'_>, s: NonNull<JSString>, index: usize) -> Result<u16, JSError> {
    let mut ch: u16 = 0;
    let ok = unsafe { wrappers2::JS_GetStringCharAt(scope.cx(), s.as_ptr(), index, &mut ch) };
    JSError::check(ok)?;
    Ok(ch)
}

// ---------------------------------------------------------------------------
// Atom operations
// ---------------------------------------------------------------------------

/// Atomize a UTF-8 string (intern it for identity comparison).
///
/// Returns an interned string, rooted in the scope.
pub fn atomize<'s>(scope: &'s Scope<'_>, s: &str) -> Result<Handle<'s, *mut JSString>, JSError> {
    let js_str =
        unsafe { wrappers2::JS_AtomizeStringN(scope.cx(), s.as_ptr() as *const c_char, s.len()) };
    NonNull::new(js_str)
        .map(|p| scope.root_string(p))
        .ok_or(JSError)
}

/// Atomize and pin a UTF-8 string (keep it alive for the lifetime of the
/// runtime).
pub fn atomize_and_pin<'s>(
    scope: &'s Scope<'_>,
    s: &str,
) -> Result<Handle<'s, *mut JSString>, JSError> {
    let js_str = unsafe {
        wrappers2::JS_AtomizeAndPinStringN(scope.cx(), s.as_ptr() as *const c_char, s.len())
    };
    NonNull::new(js_str)
        .map(|p| scope.root_string(p))
        .ok_or(JSError)
}

/// Check whether an atomized string has been pinned.
pub fn has_been_pinned(scope: &Scope<'_>, s: NonNull<JSString>) -> bool {
    unsafe { wrappers2::JS_StringHasBeenPinned(scope.cx(), s.as_ptr()) }
}

// ---------------------------------------------------------------------------
// String utilities
// ---------------------------------------------------------------------------

/// Concatenate two JS strings.
pub fn concat<'s>(
    scope: &'s Scope<'_>,
    left: HandleString,
    right: HandleString,
) -> Result<Handle<'s, *mut JSString>, JSError> {
    let result = unsafe { wrappers2::JS_ConcatStrings(scope.cx_mut(), left, right) };
    NonNull::new(result)
        .map(|p| scope.root_string(p))
        .ok_or(JSError)
}

/// Compare two JS strings, returning a comparison result like `strcmp`.
///
/// Returns `< 0` if `s1 < s2`, `0` if equal, `> 0` if `s1 > s2`.
pub fn compare(
    scope: &Scope<'_>,
    s1: NonNull<JSString>,
    s2: NonNull<JSString>,
) -> Result<i32, JSError> {
    let mut result: i32 = 0;
    let ok =
        unsafe { wrappers2::JS_CompareStrings(scope.cx(), s1.as_ptr(), s2.as_ptr(), &mut result) };
    JSError::check(ok)?;
    Ok(result)
}

/// Check whether a JS string equals an ASCII string literal.
pub fn equals_ascii(
    scope: &Scope<'_>,
    s: NonNull<JSString>,
    ascii: &CStr,
) -> Result<bool, JSError> {
    let mut matched = false;
    let ok = unsafe {
        wrappers2::JS_StringEqualsAscii(scope.cx(), s.as_ptr(), ascii.as_ptr(), &mut matched)
    };
    JSError::check(ok)?;
    Ok(matched)
}

/// Get the length of a JS string in code units.
pub fn length(s: NonNull<JSString>) -> usize {
    unsafe { mozjs::jsapi::JS_GetStringLength(s.as_ptr()) }
}

/// Check whether a JS string is linear (flat representation).
pub fn is_linear(s: NonNull<JSString>) -> bool {
    unsafe { mozjs::jsapi::JS_StringIsLinear(s.as_ptr()) }
}

/// Ensure a string has a linear (flat) representation.
///
/// Returns the linear string pointer, or an error if allocation fails.
pub fn ensure_linear(
    scope: &Scope<'_>,
    s: NonNull<JSString>,
) -> Result<NonNull<JSLinearString>, JSError> {
    let result = unsafe { wrappers2::JS_EnsureLinearString(scope.cx(), s.as_ptr()) };
    NonNull::new(result).ok_or(JSError)
}

/// Create a dependent (substring) string.
pub fn substring<'s>(
    scope: &'s Scope<'_>,
    s: HandleString,
    start: usize,
    length: usize,
) -> Result<Handle<'s, *mut JSString>, JSError> {
    let result = unsafe { wrappers2::JS_NewDependentString(scope.cx(), s, start, length) };
    NonNull::new(result)
        .map(|p| scope.root_string(p))
        .ok_or(JSError)
}

/// Get the encoding (byte) length of a string when encoded to Latin-1.
pub fn get_encoding_length(scope: &Scope<'_>, s: NonNull<JSString>) -> usize {
    unsafe { wrappers2::JS_GetStringEncodingLength(scope.cx(), s.as_ptr()) }
}

/// Create an external Latin1 string backed by caller-owned memory.
///
/// # Safety
///
/// - `chars` must remain valid for the lifetime of the string.
/// - `callbacks` must handle deallocation correctly.
pub unsafe fn new_external_latin1<'s>(
    scope: &'s Scope<'_>,
    chars: *const u8,
    length: usize,
    callbacks: *const mozjs::jsapi::JSExternalStringCallbacks,
) -> Result<Handle<'s, *mut JSString>, JSError> {
    let result = wrappers2::JS_NewExternalStringLatin1(scope.cx_mut(), chars, length, callbacks);
    NonNull::new(result)
        .map(|p| scope.root_string(p))
        .ok_or(JSError)
}

/// Create an external two-byte string backed by caller-owned memory.
///
/// # Safety
///
/// - `chars` must remain valid for the lifetime of the string.
/// - `callbacks` must handle deallocation correctly.
pub unsafe fn new_external_uc<'s>(
    scope: &'s Scope<'_>,
    chars: *const u16,
    length: usize,
    callbacks: *const mozjs::jsapi::JSExternalStringCallbacks,
) -> Result<Handle<'s, *mut JSString>, JSError> {
    let result = wrappers2::JS_NewExternalUCString(scope.cx_mut(), chars, length, callbacks);
    NonNull::new(result)
        .map(|p| scope.root_string(p))
        .ok_or(JSError)
}

/// Convert any JS value to a string via `JS::ToStringSlow`.
///
/// This is the general-purpose value-to-string conversion, equivalent to
/// the JS `String(value)` operation. It handles all value types including
/// symbols and objects.
// TODO: remove this once we don't need it for Error reporting anymore.
pub fn to_string_slow<'s>(
    scope: &'s Scope<'_>,
    val: mozjs::gc::HandleValue,
) -> Result<Handle<'s, *mut JSString>, JSError> {
    let js_str = unsafe { wrappers2::ToStringSlow(scope.cx_mut(), val) };
    NonNull::new(js_str)
        .map(|p| scope.root_string(p))
        .ok_or(JSError)
}
