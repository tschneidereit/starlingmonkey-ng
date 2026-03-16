// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! JS string creation, encoding, comparison, and atom operations.
//!
//! SpiderMonkey strings are GC-managed and encoded internally as either Latin1
//! or two-byte (UTF-16). Unlike `Object`, `Array`, or `Function`, JS strings
//! are a separate GC-managed type (`*mut JSString`), not JS objects.
//!
//! The [`Str<'s>`] newtype wraps a scope-rooted `Handle<'s, *mut JSString>`
//! and exposes all string operations as methods. The public type alias
//! [`JSString<'s>`](crate::JSString) is the preferred name.
//!
//! # Creating Strings
//!
//! ```ignore
//! let s = js::JSString::from_str(&scope, "hello")?;
//! let c = js::JSString::from_cstr(&scope, c"world")?;
//! let e = js::JSString::empty(&scope);
//! ```
//!
//! # Extracting Content
//!
//! ```ignore
//! let rust_str: String = s.to_utf8(&scope)?;
//! let ch: u16 = s.char_at(&scope, 0)?;
//! let len: usize = s.len();
//! ```

use std::cell::Cell;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr::NonNull;

use crate::gc::scope::Scope;
use mozjs::gc::Handle;
use mozjs::jsapi::{JSLinearString, JSString};
use mozjs::jsval::StringValue;
use mozjs::rust::wrappers2;

use super::error::ExnThrown;

// ---------------------------------------------------------------------------
// Str — scope-rooted JS string handle
// ---------------------------------------------------------------------------

/// A scope-rooted handle to a SpiderMonkey string.
///
/// Unlike [`Object`](crate::Object) and other builtin handle types, JS
/// strings are not JSObjects — they are a separate GC-managed type. `Str<'s>`
/// wraps a `Handle<'s, *mut JSString>` and provides all string operations as
/// methods.
///
/// The public type alias [`JSString<'s>`](crate::JSString) is the preferred
/// name for this type.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Str<'s> {
    handle: Handle<'s, *mut JSString>,
}

impl<'s> Str<'s> {
    // ---------------------------------------------------------------------------
    // Construction from a raw handle
    // ---------------------------------------------------------------------------

    /// Wrap a rooted string handle.
    pub fn from_handle(handle: Handle<'s, *mut JSString>) -> Self {
        Str { handle }
    }

    /// Get the underlying rooted handle.
    pub fn handle(self) -> Handle<'s, *mut JSString> {
        self.handle
    }

    /// Get the raw `*mut JSString` pointer.
    pub fn as_raw(self) -> *mut JSString {
        self.handle.get()
    }

    /// Convert to a JS `Value` containing this string.
    pub fn as_value(self) -> mozjs::jsapi::Value {
        // SAFETY: The string is rooted via the handle.
        unsafe { StringValue(&*self.handle.get()) }
    }

    // ---------------------------------------------------------------------------
    // String creation
    // ---------------------------------------------------------------------------

    /// Create a new JS string from a UTF-8 Rust `&str`.
    pub fn from_str(scope: &'s Scope<'_>, s: &str) -> Result<Self, ExnThrown> {
        let js_str = unsafe {
            wrappers2::JS_NewStringCopyN(scope.cx_mut(), s.as_ptr() as *const c_char, s.len())
        };
        NonNull::new(js_str)
            .map(|p| Str::from_handle(scope.root_string(p)))
            .ok_or(ExnThrown)
    }

    /// Create a new JS string from a null-terminated C string.
    pub fn from_cstr(scope: &'s Scope<'_>, s: &CStr) -> Result<Self, ExnThrown> {
        let js_str = unsafe { wrappers2::JS_NewStringCopyZ(scope.cx_mut(), s.as_ptr()) };
        NonNull::new(js_str)
            .map(|p| Str::from_handle(scope.root_string(p)))
            .ok_or(ExnThrown)
    }

    /// Create a new JS string from a UTF-16 slice.
    pub fn from_utf16(scope: &'s Scope<'_>, s: &[u16]) -> Result<Self, ExnThrown> {
        let js_str = unsafe { wrappers2::JS_NewUCStringCopyN(scope.cx_mut(), s.as_ptr(), s.len()) };
        NonNull::new(js_str)
            .map(|p| Str::from_handle(scope.root_string(p)))
            .ok_or(ExnThrown)
    }

    /// Get the empty string for this context.
    ///
    /// The empty string is permanently rooted by the runtime, so this never
    /// fails.
    pub fn empty(scope: &'s Scope<'_>) -> Self {
        let ptr = unsafe { wrappers2::JS_GetEmptyString(scope.cx()) };
        // SAFETY: The empty string is always present in a valid runtime.
        let nn = unsafe { NonNull::new_unchecked(ptr) };
        Str::from_handle(scope.root_string(nn))
    }

    /// Convert any JS value to a string via `JS::ToStringSlow`.
    ///
    /// Equivalent to the JS `String(value)` operation. Handles all value
    /// types including symbols and objects.
    pub fn from_value(
        scope: &'s Scope<'_>,
        val: mozjs::gc::HandleValue,
    ) -> Result<Self, ExnThrown> {
        let js_str = unsafe { mozjs::rust::ToString(scope.cx_mut().raw_cx(), val) };
        NonNull::new(js_str)
            .map(|p| Str::from_handle(scope.root_string(p)))
            .ok_or(ExnThrown)
    }

    // ---------------------------------------------------------------------------
    // String encoding / extraction
    // ---------------------------------------------------------------------------

    /// Encode this string to UTF-8, returning an owned Rust [`String`].
    pub fn to_utf8(&self, scope: &Scope<'_>) -> Result<String, ExnThrown> {
        thread_local! {
            static RESULT: Cell<Option<String>> = const { Cell::new(None) };
        }
        unsafe extern "C" fn cb(encoded: *const c_char) {
            if !encoded.is_null() {
                let cstr = CStr::from_ptr(encoded);
                RESULT.with(|r| r.set(Some(cstr.to_string_lossy().into_owned())));
            }
        }
        unsafe { wrappers2::EncodeStringToUTF8(scope.cx_mut(), self.handle, cb) };
        RESULT.with(|r| r.take()).ok_or(ExnThrown)
    }

    /// Get a single character (code unit) at the given index.
    pub fn char_at(&self, scope: &Scope<'_>, index: usize) -> Result<u16, ExnThrown> {
        let mut ch: u16 = 0;
        let ok =
            unsafe { wrappers2::JS_GetStringCharAt(scope.cx(), self.as_raw(), index, &mut ch) };
        ExnThrown::check(ok)?;
        Ok(ch)
    }

    // ---------------------------------------------------------------------------
    // Atom operations
    // ---------------------------------------------------------------------------

    /// Atomize a UTF-8 string (intern it for identity comparison).
    pub fn atomize(scope: &'s Scope<'_>, s: &str) -> Result<Self, ExnThrown> {
        let js_str = unsafe {
            wrappers2::JS_AtomizeStringN(scope.cx(), s.as_ptr() as *const c_char, s.len())
        };
        NonNull::new(js_str)
            .map(|p| Str::from_handle(scope.root_string(p)))
            .ok_or(ExnThrown)
    }

    /// Atomize and pin a UTF-8 string (keep it alive for the lifetime of the
    /// runtime).
    pub fn atomize_and_pin(scope: &'s Scope<'_>, s: &str) -> Result<Self, ExnThrown> {
        let js_str = unsafe {
            wrappers2::JS_AtomizeAndPinStringN(scope.cx(), s.as_ptr() as *const c_char, s.len())
        };
        NonNull::new(js_str)
            .map(|p| Str::from_handle(scope.root_string(p)))
            .ok_or(ExnThrown)
    }

    /// Check whether this atomized string has been pinned.
    pub fn has_been_pinned(&self, scope: &Scope<'_>) -> bool {
        unsafe { wrappers2::JS_StringHasBeenPinned(scope.cx(), self.as_raw()) }
    }

    // ---------------------------------------------------------------------------
    // String utilities
    // ---------------------------------------------------------------------------

    /// Concatenate this string with another, returning a new string.
    pub fn concat(&self, scope: &'s Scope<'_>, other: Str<'_>) -> Result<Self, ExnThrown> {
        let result =
            unsafe { wrappers2::JS_ConcatStrings(scope.cx_mut(), self.handle, other.handle) };
        NonNull::new(result)
            .map(|p| Str::from_handle(scope.root_string(p)))
            .ok_or(ExnThrown)
    }

    /// Compare this string with another, returning a result like `strcmp`.
    ///
    /// Returns `< 0` if `self < other`, `0` if equal, `> 0` if `self > other`.
    pub fn compare(&self, scope: &Scope<'_>, other: Str<'_>) -> Result<i32, ExnThrown> {
        let mut result: i32 = 0;
        let ok = unsafe {
            wrappers2::JS_CompareStrings(scope.cx(), self.as_raw(), other.as_raw(), &mut result)
        };
        ExnThrown::check(ok)?;
        Ok(result)
    }

    /// Check whether this string equals an ASCII string literal.
    pub fn equals_ascii(&self, scope: &Scope<'_>, ascii: &CStr) -> Result<bool, ExnThrown> {
        let mut matched = false;
        let ok = unsafe {
            wrappers2::JS_StringEqualsAscii(scope.cx(), self.as_raw(), ascii.as_ptr(), &mut matched)
        };
        ExnThrown::check(ok)?;
        Ok(matched)
    }

    /// Get the length in code units.
    pub fn len(&self) -> usize {
        unsafe { mozjs::jsapi::JS_GetStringLength(self.as_raw()) }
    }

    /// Check whether this string is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Check whether this string has a linear (flat) representation.
    pub fn is_linear(&self) -> bool {
        unsafe { mozjs::jsapi::JS_StringIsLinear(self.as_raw()) }
    }

    /// Ensure this string has a linear (flat) representation.
    ///
    /// Returns the linear string pointer, or an error if allocation fails.
    pub fn ensure_linear(&self, scope: &Scope<'_>) -> Result<NonNull<JSLinearString>, ExnThrown> {
        let result = unsafe { wrappers2::JS_EnsureLinearString(scope.cx(), self.as_raw()) };
        NonNull::new(result).ok_or(ExnThrown)
    }

    /// Create a dependent (substring) string.
    pub fn substring(
        &self,
        scope: &'s Scope<'_>,
        start: usize,
        length: usize,
    ) -> Result<Self, ExnThrown> {
        let result =
            unsafe { wrappers2::JS_NewDependentString(scope.cx(), self.handle, start, length) };
        NonNull::new(result)
            .map(|p| Str::from_handle(scope.root_string(p)))
            .ok_or(ExnThrown)
    }

    /// Get the encoding (byte) length when encoded to Latin-1.
    pub fn encoding_length(&self, scope: &Scope<'_>) -> usize {
        unsafe { wrappers2::JS_GetStringEncodingLength(scope.cx(), self.as_raw()) }
    }

    /// Create an external Latin1 string backed by caller-owned memory.
    ///
    /// # Safety
    ///
    /// - `chars` must remain valid for the lifetime of the string.
    /// - `callbacks` must handle deallocation correctly.
    pub unsafe fn new_external_latin1(
        scope: &'s Scope<'_>,
        chars: *const u8,
        length: usize,
        callbacks: *const mozjs::jsapi::JSExternalStringCallbacks,
    ) -> Result<Self, ExnThrown> {
        let result =
            wrappers2::JS_NewExternalStringLatin1(scope.cx_mut(), chars, length, callbacks);
        NonNull::new(result)
            .map(|p| Str::from_handle(scope.root_string(p)))
            .ok_or(ExnThrown)
    }

    /// Create an external two-byte string backed by caller-owned memory.
    ///
    /// # Safety
    ///
    /// - `chars` must remain valid for the lifetime of the string.
    /// - `callbacks` must handle deallocation correctly.
    pub unsafe fn new_external_uc(
        scope: &'s Scope<'_>,
        chars: *const u16,
        length: usize,
        callbacks: *const mozjs::jsapi::JSExternalStringCallbacks,
    ) -> Result<Self, ExnThrown> {
        let result = wrappers2::JS_NewExternalUCString(scope.cx_mut(), chars, length, callbacks);
        NonNull::new(result)
            .map(|p| Str::from_handle(scope.root_string(p)))
            .ok_or(ExnThrown)
    }
}

impl<'s> std::fmt::Debug for Str<'s> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JSString")
            .field("ptr", &self.as_raw())
            .finish()
    }
}
