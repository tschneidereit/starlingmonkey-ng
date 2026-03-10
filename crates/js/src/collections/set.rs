// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! JavaScript `Set` collection type.

use std::ptr::NonNull;

use crate::gc::handle::{JsType, Stack};
use crate::gc::scope::Scope;
use mozjs::jsapi::Value;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2;
use mozjs::rust::{HandleObject, HandleValue};

use crate::builtins::{Is, To};
use crate::error::JSError;
use crate::Object;

/// Marker type for JavaScript `Set` objects.
///
/// Use the `js::Set` alias for [`Stack<'s, Set>`](crate::gc::handle::Stack)
/// as the scope-rooted handle type:
///
/// ```ignore
/// let set = js::Set::new(&scope)?;
/// set.add(&scope, val.handle())?;
/// ```
pub struct Set;

impl JsType for Set {
    const JS_NAME: &'static str = "Set";
}

impl<'s> Stack<'s, Set> {
    /// Create a new empty `Set` object.
    pub fn new(scope: &'s Scope<'_>) -> Result<Self, JSError> {
        let obj = unsafe { wrappers2::NewSetObject(scope.cx_mut()) };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(JSError)
    }

    /// Get the number of entries.
    pub fn size(&self, scope: &Scope<'_>) -> u32 {
        unsafe { wrappers2::SetSize(scope.cx(), self.handle()) }
    }

    /// Check whether the set contains a value.
    pub fn has(&self, scope: &Scope<'_>, key: HandleValue) -> Result<bool, JSError> {
        let mut result = false;
        let ok = unsafe { wrappers2::SetHas(scope.cx_mut(), self.handle(), key, &mut result) };
        JSError::check(ok)?;
        Ok(result)
    }

    /// Add a value to the set.
    pub fn add(&self, scope: &Scope<'_>, key: HandleValue) -> Result<(), JSError> {
        let ok = unsafe { wrappers2::SetAdd(scope.cx_mut(), self.handle(), key) };
        JSError::check(ok)
    }

    /// Delete a value. Returns whether the value was present.
    pub fn delete(&self, scope: &Scope<'_>, key: HandleValue) -> Result<bool, JSError> {
        let mut deleted = false;
        let ok = unsafe { wrappers2::SetDelete(scope.cx_mut(), self.handle(), key, &mut deleted) };
        JSError::check(ok)?;
        Ok(deleted)
    }

    /// Remove all entries from the set.
    pub fn clear(&self, scope: &Scope<'_>) -> Result<(), JSError> {
        let ok = unsafe { wrappers2::SetClear(scope.cx(), self.handle()) };
        JSError::check(ok)
    }

    /// Get an iterator over the set's keys (same as values for sets).
    pub fn keys(&self, scope: &Scope<'_>) -> Result<Value, JSError> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = unsafe { wrappers2::SetKeys(scope.cx_mut(), self.handle(), rval.handle_mut()) };
        JSError::check(ok)?;
        Ok(rval.get())
    }

    /// Get an iterator over the set's values.
    pub fn values(&self, scope: &Scope<'_>) -> Result<Value, JSError> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = unsafe { wrappers2::SetValues(scope.cx_mut(), self.handle(), rval.handle_mut()) };
        JSError::check(ok)?;
        Ok(rval.get())
    }

    /// Get an iterator over the set's entries.
    pub fn entries(&self, scope: &Scope<'_>) -> Result<Value, JSError> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = unsafe { wrappers2::SetEntries(scope.cx_mut(), self.handle(), rval.handle_mut()) };
        JSError::check(ok)?;
        Ok(rval.get())
    }

    /// Call a callback for each value in the set.
    pub fn for_each(
        &self,
        scope: &Scope<'_>,
        callback_fn: HandleValue,
        this_val: HandleValue,
    ) -> Result<(), JSError> {
        let ok =
            unsafe { wrappers2::SetForEach(scope.cx_mut(), self.handle(), callback_fn, this_val) };
        JSError::check(ok)
    }

    /// Check whether an object is a `Set`.
    pub fn is_set(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        let mut result = false;
        // SAFETY: cx and obj are valid; IsSetObject writes to result.
        let ok = unsafe { wrappers2::IsSetObject(scope.cx_mut(), obj, &mut result) };
        JSError::check(ok)?;
        Ok(result)
    }
}

impl Is for Set {
    fn is(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        Stack::<Set>::is_set(scope, obj)
    }
}

impl<'s> To<Stack<'s, Set>> for Object<'s> {
    fn to(&self, scope: &Scope<'_>) -> Result<Stack<'s, Set>, JSError> {
        if Set::is(scope, self.handle())? {
            // SAFETY: We just verified the object is a Set.
            Ok(unsafe { Stack::from_handle_unchecked(self.handle()) })
        } else {
            Err(JSError)
        }
    }
}

impl<'s> std::ops::Deref for Stack<'s, Set> {
    type Target = Object<'s>;

    fn deref(&self) -> &Object<'s> {
        // SAFETY: Stack<Set> and Stack<Object> are both repr(transparent)
        // over Handle<'s, *mut JSObject>.
        unsafe { &*(self as *const Stack<'s, Set> as *const Object<'s>) }
    }
}
