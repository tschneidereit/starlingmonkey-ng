// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Array creation and element access.
//!
//! SpiderMonkey arrays are ordinary objects with a special `length` property.
//! The [`Array`] newtype wraps a scope-rooted `Handle<'s, *mut JSObject>`
//! that is known to be an array. It implements `Deref` to [`Object`], so all
//! property and prototype methods are available directly.

use std::os::raw::c_uint;
use std::ptr::NonNull;

use crate::gc::scope::Scope;
use crate::object::Object;
use mozjs::gc::Handle;
use mozjs::jsapi::{HandleValueArray, JSObject};
use mozjs::rust::wrappers2;
use mozjs::rust::{HandleObject, HandleValue};

use super::builtins::{Is, IsValue, To};
use super::error::JSError;

/// A JavaScript `Array` object, rooted in a scope's pool.
///
/// `Array<'s>` wraps a `Handle<'s, *mut JSObject>` known to be an `Array`.
/// Construction automatically roots in the scope:
///
/// ```ignore
/// let arr = Array::new(&scope, 5)?;
/// let len = arr.length(&scope)?;
/// ```
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct Array<'s>(pub(crate) Handle<'s, *mut JSObject>);

impl<'s> Array<'s> {
    /// Create a new empty array with the given initial length.
    pub fn new(scope: &'s Scope<'_>, length: usize) -> Result<Self, JSError> {
        let obj = unsafe { wrappers2::NewArrayObject1(scope.cx_mut(), length) };
        NonNull::new(obj)
            .map(|nn| Array(scope.root_object(nn)))
            .ok_or(JSError)
    }

    /// Create a new array pre-populated with the given values.
    pub fn with_contents(
        scope: &'s Scope<'_>,
        contents: &HandleValueArray,
    ) -> Result<Self, JSError> {
        let obj = unsafe { wrappers2::NewArrayObject(scope.cx_mut(), contents) };
        NonNull::new(obj)
            .map(|nn| Array(scope.root_object(nn)))
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

    /// Wrap an existing rooted handle in an `Array`.
    ///
    /// The caller should ensure the handle points to an actual Array object.
    pub fn from_handle(handle: Handle<'s, *mut JSObject>) -> Self {
        Array(handle)
    }

    /// Get the `length` of this array.
    pub fn length(&self, scope: &Scope<'_>) -> Result<u32, JSError> {
        let mut len: u32 = 0;
        let ok = unsafe { wrappers2::GetArrayLength(scope.cx_mut(), self.0, &mut len) };
        JSError::check(ok)?;
        Ok(len)
    }

    /// Set the `length` of this array.
    pub fn set_length(&self, scope: &Scope<'_>, length: u32) -> Result<(), JSError> {
        let ok = unsafe { wrappers2::SetArrayLength(scope.cx_mut(), self.0, length) };
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
        let ok =
            unsafe { wrappers2::JS_DefineElement(scope.cx_mut(), self.0, index, value, attrs) };
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

impl Is for Array<'_> {
    fn is(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        Array::is_array(scope, obj)
    }
}

impl IsValue for Array<'_> {
    fn is_value(scope: &Scope<'_>, val: HandleValue) -> Result<bool, JSError> {
        Array::is_array_value(scope, val)
    }
}

impl<'s> To<Array<'s>> for Object<'s> {
    fn to(&self, scope: &Scope<'_>) -> Result<Array<'s>, JSError> {
        if Array::is(scope, self.0)? {
            // SAFETY: Array and Object are both repr(transparent) over
            // Handle<'s, *mut JSObject>, so the conversion is valid.
            Ok(Array(self.0))
        } else {
            Err(JSError)
        }
    }
}

impl<'s> std::ops::Deref for Array<'s> {
    type Target = Object<'s>;

    fn deref(&self) -> &Object<'s> {
        // SAFETY: Array and Object are both repr(transparent) over
        // Handle<'s, *mut JSObject>, so they have identical layout.
        unsafe { &*(self as *const Array<'s> as *const Object<'s>) }
    }
}
