// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Array creation and element access.
//!
//! SpiderMonkey arrays are ordinary objects with a special `length` property.
//! The [`Array`] marker type implements [`JsType`](crate::gc::handle::JsType),
//! enabling [`Stack<'s, Array>`](crate::gc::handle::Stack) as the scope-rooted
//! handle type. It implements `Deref` to [`Stack<Object>`](crate::Object),
//! so all property and prototype methods are available directly.

use std::os::raw::c_uint;
use std::ptr::NonNull;

use crate::gc::handle::{JsType, Stack};
use crate::gc::scope::Scope;
use crate::Object;
use mozjs::jsapi::HandleValueArray;
use mozjs::rust::wrappers2;
use mozjs::rust::{HandleObject, HandleValue};

use super::builtins::{Is, IsValue, To};
use super::error::JSError;

/// Marker type for JavaScript `Array` objects.
///
/// Use the `js::Array` alias for [`Stack<'s, Array>`](crate::gc::handle::Stack)
/// as the scope-rooted handle type:
///
/// ```ignore
/// let arr = js::Array::new(&scope, 5)?;
/// let len = arr.length(&scope)?;
/// ```
pub struct Array;

impl JsType for Array {
    const JS_NAME: &'static str = "Array";
}

impl<'s> Stack<'s, Array> {
    /// Create a new empty array with the given initial length.
    pub fn new(scope: &'s Scope<'_>, length: usize) -> Result<Self, JSError> {
        let obj = unsafe { wrappers2::NewArrayObject1(scope.cx_mut(), length) };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(JSError)
    }

    /// Create a new array pre-populated with the given values.
    pub fn with_contents(
        scope: &'s Scope<'_>,
        contents: &HandleValueArray,
    ) -> Result<Self, JSError> {
        let obj = unsafe { wrappers2::NewArrayObject(scope.cx_mut(), contents) };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(JSError)
    }

    /// Get the `length` of this array.
    pub fn length(&self, scope: &Scope<'_>) -> Result<u32, JSError> {
        let mut len: u32 = 0;
        let ok = unsafe { wrappers2::GetArrayLength(scope.cx_mut(), self.handle(), &mut len) };
        JSError::check(ok)?;
        Ok(len)
    }

    /// Set the `length` of this array.
    pub fn set_length(&self, scope: &Scope<'_>, length: u32) -> Result<(), JSError> {
        let ok = unsafe { wrappers2::SetArrayLength(scope.cx_mut(), self.handle(), length) };
        JSError::check(ok)
    }

    /// Define an element by index with attribute flags.
    pub fn define_element(
        &self,
        scope: &Scope<'_>,
        index: u32,
        value: HandleValue,
        attrs: c_uint,
    ) -> Result<(), JSError> {
        let ok = unsafe {
            wrappers2::JS_DefineElement(scope.cx_mut(), self.handle(), index, value, attrs)
        };
        JSError::check(ok)
    }

    /// Check whether an object is an `Array`.
    pub fn is_array(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        let mut result = false;
        let ok = unsafe { wrappers2::IsArrayObject1(scope.cx_mut(), obj, &mut result) };
        JSError::check(ok)?;
        Ok(result)
    }

    /// Check whether a value is an `Array`.
    pub fn is_array_value(scope: &Scope<'_>, value: HandleValue) -> Result<bool, JSError> {
        let mut result = false;
        let ok = unsafe { wrappers2::IsArrayObject(scope.cx_mut(), value, &mut result) };
        JSError::check(ok)?;
        Ok(result)
    }
}

impl Is for Array {
    fn is(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        Stack::<Array>::is_array(scope, obj)
    }
}

impl IsValue for Array {
    fn is_value(scope: &Scope<'_>, val: HandleValue) -> Result<bool, JSError> {
        Stack::<Array>::is_array_value(scope, val)
    }
}

impl<'s> To<Stack<'s, Array>> for Object<'s> {
    fn to(&self, scope: &Scope<'_>) -> Result<Stack<'s, Array>, JSError> {
        if Array::is(scope, self.handle())? {
            // SAFETY: We just verified the object is an Array.
            Ok(unsafe { Stack::from_handle_unchecked(self.handle()) })
        } else {
            Err(JSError)
        }
    }
}

impl<'s> std::ops::Deref for Stack<'s, Array> {
    type Target = Object<'s>;

    fn deref(&self) -> &Object<'s> {
        // SAFETY: Stack<Array> and Stack<Object> are both repr(transparent)
        // over Handle<'s, *mut JSObject>, so they have identical layout.
        unsafe { &*(self as *const Stack<'s, Array> as *const Object<'s>) }
    }
}
