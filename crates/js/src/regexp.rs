// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Regular expression creation, execution, and inspection.
//!
//! The [`RegExp`] newtype wraps a scope-rooted `Handle<'s, *mut JSObject>`
//! known to be a RegExp object. It provides methods for creating and testing
//! regular expressions.

use std::ffi::CStr;
use std::ptr::NonNull;

use crate::gc::scope::Scope;
use mozjs::gc::Handle;
use mozjs::jsapi::{JSObject, JSString, RegExpFlags, Value};
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2;
use mozjs::rust::HandleObject;

use super::builtins::{Is, To};
use super::error::JSError;
use super::object::Object;

/// A JavaScript `RegExp` object, rooted in a scope's pool.
///
/// `RegExp<'s>` wraps a `Handle<'s, *mut JSObject>` known to be a `RegExp`.
/// Construction automatically roots in the scope:
///
/// ```ignore
/// let re = RegExp::new(&scope, c"\\d+", flags)?;
/// let src = re.source(&scope)?;
/// ```
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct RegExp<'s>(pub(crate) Handle<'s, *mut JSObject>);

impl<'s> RegExp<'s> {
    /// Create a new `RegExp` object from a Latin-1 (byte) pattern and flags.
    pub fn new(scope: &'s Scope<'_>, pattern: &CStr, flags: RegExpFlags) -> Result<Self, JSError> {
        let bytes = pattern.as_ptr();
        let len = pattern.to_bytes().len();
        let obj = unsafe { wrappers2::NewRegExpObject(scope.cx_mut(), bytes, len, flags) };
        NonNull::new(obj)
            .map(|nn| RegExp(scope.root_object(nn)))
            .ok_or(JSError)
    }

    /// Create a new `RegExp` object from a UTF-16 pattern and flags.
    pub fn from_utf16(
        scope: &'s Scope<'_>,
        chars: &[u16],
        flags: RegExpFlags,
    ) -> Result<Self, JSError> {
        let obj = unsafe {
            wrappers2::NewUCRegExpObject(scope.cx_mut(), chars.as_ptr(), chars.len(), flags)
        };
        NonNull::new(obj)
            .map(|nn| RegExp(scope.root_object(nn)))
            .ok_or(JSError)
    }

    /// Get the rooted handle to the underlying `JSObject`.
    pub fn handle(&self) -> HandleObject<'s> {
        self.0
    }

    /// Get a raw `NonNull` pointer to the underlying `JSObject`.
    pub fn as_non_null(self) -> Option<NonNull<JSObject>> {
        NonNull::new(self.0.get())
    }

    /// Get the raw `*mut JSObject` pointer.
    pub fn as_raw(self) -> *mut JSObject {
        self.0.get()
    }

    /// Wrap an existing rooted handle in a `RegExp`.
    pub fn from_handle(handle: Handle<'s, *mut JSObject>) -> Self {
        RegExp(handle)
    }

    /// Check whether an object is a `RegExp`.
    pub fn is_regexp(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        let mut result = false;
        // SAFETY: cx and obj are valid; ObjectIsRegExp writes to result.
        let ok = unsafe { wrappers2::ObjectIsRegExp(scope.cx_mut(), obj, &mut result) };
        JSError::check(ok)?;
        Ok(result)
    }

    /// Get the source pattern string of this `RegExp`.
    pub fn source<'a>(&self, scope: &'a Scope<'_>) -> Result<Handle<'a, *mut JSString>, JSError> {
        let s = unsafe { wrappers2::GetRegExpSource(scope.cx_mut(), self.0) };
        NonNull::new(s).map(|p| scope.root_string(p)).ok_or(JSError)
    }

    /// Execute this `RegExp` against a UTF-16 string without modifying statics.
    ///
    /// If `test` is true, only tests for a match (the result value is a boolean).
    /// If `test` is false, returns the match result array.
    ///
    /// `indexp` is the byte index to start searching from (updated on return).
    ///
    /// # Safety
    ///
    /// The `chars` slice must be a valid UTF-16 string and must remain valid for
    /// the duration of this call.
    pub unsafe fn execute_no_statics(
        &self,
        scope: &Scope<'_>,
        chars: &[u16],
        indexp: &mut usize,
        test: bool,
    ) -> Result<Value, JSError> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = wrappers2::ExecuteRegExpNoStatics(
            scope.cx_mut(),
            self.0,
            chars.as_ptr(),
            chars.len(),
            indexp,
            test,
            rval.handle_mut(),
        );
        JSError::check(ok)?;
        Ok(rval.get())
    }

    /// Check whether a regular expression pattern is syntactically valid.
    ///
    /// If the pattern is invalid, returns the error value. If valid,
    /// returns `undefined`.
    pub fn check_syntax(
        scope: &Scope<'_>,
        chars: &[u16],
        flags: RegExpFlags,
    ) -> Result<Value, JSError> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut error = UndefinedValue());
        let ok = unsafe {
            wrappers2::CheckRegExpSyntax(
                scope.cx_mut(),
                chars.as_ptr(),
                chars.len(),
                flags,
                error.handle_mut(),
            )
        };
        JSError::check(ok)?;
        Ok(error.get())
    }
}

impl Is for RegExp<'_> {
    fn is(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        RegExp::is_regexp(scope, obj)
    }
}

impl<'s> To<RegExp<'s>> for Object<'s> {
    fn to(&self, scope: &Scope<'_>) -> Result<RegExp<'s>, JSError> {
        if RegExp::is(scope, self.0)? {
            Ok(RegExp(self.0))
        } else {
            Err(JSError)
        }
    }
}

impl<'s> std::ops::Deref for RegExp<'s> {
    type Target = Object<'s>;

    fn deref(&self) -> &Object<'s> {
        // SAFETY: RegExp and Object are both repr(transparent) over
        // Handle<'s, *mut JSObject>.
        unsafe { &*(self as *const RegExp<'s> as *const Object<'s>) }
    }
}
