// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Regular expression creation, execution, and inspection.
//!
//! The [`RegExp`] marker type implements [`JSType`](crate::gc::handle::JSType),
//! enabling [`RegExp<'s>`](crate::RegExp) as the scope-rooted
//! handle type. It provides methods for creating and testing regular
//! expressions.

use std::ffi::CStr;
use std::ptr::NonNull;

use crate::builtins::JSType;
use crate::gc::handle::Stack;
use crate::gc::scope::Scope;
use mozjs::jsapi::{JSString, RegExpFlags, Value};
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2;
use mozjs::rust::HandleObject;

use super::error::ExnThrown;
use crate::Object;

/// Marker type for JavaScript `RegExp` objects.
///
/// [`RegExp<'s>`](crate::RegExp) is the scope-rooted handle type:
///
/// ```ignore
/// let re = js::RegExp::new(&scope, c"\\d+", flags)?;
/// let src = re.source(&scope)?;
/// ```
pub struct RegExp;

impl JSType for RegExp {
    const JS_NAME: &'static str = "RegExp";

    fn js_class() -> *const mozjs::jsapi::JSClass {
        crate::class::proto_key_to_class(mozjs::jsapi::JSProtoKey::JSProto_RegExp)
    }
}

impl<'s> Stack<'s, RegExp> {
    /// Create a new `RegExp` object from a Latin-1 (byte) pattern and flags.
    pub fn new(
        scope: &'s Scope<'_>,
        pattern: &CStr,
        flags: RegExpFlags,
    ) -> Result<Self, ExnThrown> {
        let bytes = pattern.as_ptr();
        let len = pattern.to_bytes().len();
        let obj = unsafe { wrappers2::NewRegExpObject(scope.cx_mut(), bytes, len, flags) };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(ExnThrown)
    }

    /// Create a new `RegExp` object from a UTF-16 pattern and flags.
    pub fn from_utf16(
        scope: &'s Scope<'_>,
        chars: &[u16],
        flags: RegExpFlags,
    ) -> Result<Self, ExnThrown> {
        let obj = unsafe {
            wrappers2::NewUCRegExpObject(scope.cx_mut(), chars.as_ptr(), chars.len(), flags)
        };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(ExnThrown)
    }

    /// Check whether an object is a `RegExp`.
    pub fn is_regexp(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, ExnThrown> {
        let mut result = false;
        // SAFETY: cx and obj are valid; ObjectIsRegExp writes to result.
        let ok = unsafe { wrappers2::ObjectIsRegExp(scope.cx_mut(), obj, &mut result) };
        ExnThrown::check(ok)?;
        Ok(result)
    }

    /// Get the source pattern string of this `RegExp`.
    pub fn source<'a>(
        &self,
        scope: &'a Scope<'_>,
    ) -> Result<mozjs::gc::Handle<'a, *mut JSString>, ExnThrown> {
        let s = unsafe { wrappers2::GetRegExpSource(scope.cx_mut(), self.handle()) };
        NonNull::new(s)
            .map(|p| scope.root_string(p))
            .ok_or(ExnThrown)
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
    ) -> Result<Value, ExnThrown> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = wrappers2::ExecuteRegExpNoStatics(
            scope.cx_mut(),
            self.handle(),
            chars.as_ptr(),
            chars.len(),
            indexp,
            test,
            rval.handle_mut(),
        );
        ExnThrown::check(ok)?;
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
    ) -> Result<Value, ExnThrown> {
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
        ExnThrown::check(ok)?;
        Ok(error.get())
    }
}

impl<'s> std::ops::Deref for Stack<'s, RegExp> {
    type Target = Object<'s>;

    fn deref(&self) -> &Object<'s> {
        // SAFETY: Stack<RegExp> and Stack<Object> are both repr(transparent)
        // over Handle<'s, *mut JSObject>.
        unsafe { &*(self as *const Stack<'s, RegExp> as *const Object<'s>) }
    }
}
