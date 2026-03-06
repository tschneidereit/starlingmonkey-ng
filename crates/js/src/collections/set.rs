// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! JavaScript `Set` collection type.

use std::ptr::NonNull;

use crate::gc::scope::Scope;
use mozjs::gc::Handle;
use mozjs::jsapi::{JSObject, Value};
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2;
use mozjs::rust::{HandleObject, HandleValue};

use crate::builtins::{Is, To};
use crate::error::JSError;
use crate::object::Object;

/// A JavaScript `Set` object, rooted in a scope's pool.
///
/// `Set<'s>` wraps a `Handle<'s, *mut JSObject>` known to be a `Set`.
/// Construction automatically roots in the scope:
///
/// ```ignore
/// let set = Set::new(&scope)?;
/// set.add(&scope, val.handle())?;
/// ```
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct Set<'s>(pub(crate) Handle<'s, *mut JSObject>);

impl<'s> Set<'s> {
    /// Create a new empty `Set` object.
    pub fn new(scope: &'s Scope<'_>) -> Result<Self, JSError> {
        let obj = unsafe { wrappers2::NewSetObject(scope.cx_mut()) };
        NonNull::new(obj)
            .map(|nn| Set(scope.root_object(nn)))
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

    /// Wrap an existing rooted handle in a `Set`.
    pub fn from_handle(handle: Handle<'s, *mut JSObject>) -> Self {
        Set(handle)
    }

    /// Get the number of entries.
    pub fn size(&self, scope: &Scope<'_>) -> u32 {
        unsafe { wrappers2::SetSize(scope.cx(), self.0) }
    }

    /// Check whether the set contains a value.
    pub fn has(&self, scope: &Scope<'_>, key: HandleValue) -> Result<bool, JSError> {
        let mut result = false;
        let ok = unsafe { wrappers2::SetHas(scope.cx_mut(), self.0, key, &mut result) };
        JSError::check(ok)?;
        Ok(result)
    }

    /// Add a value to the set.
    pub fn add(&self, scope: &Scope<'_>, key: HandleValue) -> Result<(), JSError> {
        let ok = unsafe { wrappers2::SetAdd(scope.cx_mut(), self.0, key) };
        JSError::check(ok)
    }

    /// Delete a value. Returns whether the value was present.
    pub fn delete(&self, scope: &Scope<'_>, key: HandleValue) -> Result<bool, JSError> {
        let mut deleted = false;
        let ok = unsafe { wrappers2::SetDelete(scope.cx_mut(), self.0, key, &mut deleted) };
        JSError::check(ok)?;
        Ok(deleted)
    }

    /// Remove all entries from the set.
    pub fn clear(&self, scope: &Scope<'_>) -> Result<(), JSError> {
        let ok = unsafe { wrappers2::SetClear(scope.cx(), self.0) };
        JSError::check(ok)
    }

    /// Get an iterator over the set's keys (same as values for sets).
    pub fn keys(&self, scope: &Scope<'_>) -> Result<Value, JSError> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = unsafe { wrappers2::SetKeys(scope.cx_mut(), self.0, rval.handle_mut()) };
        JSError::check(ok)?;
        Ok(rval.get())
    }

    /// Get an iterator over the set's values.
    pub fn values(&self, scope: &Scope<'_>) -> Result<Value, JSError> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = unsafe { wrappers2::SetValues(scope.cx_mut(), self.0, rval.handle_mut()) };
        JSError::check(ok)?;
        Ok(rval.get())
    }

    /// Get an iterator over the set's entries.
    pub fn entries(&self, scope: &Scope<'_>) -> Result<Value, JSError> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = unsafe { wrappers2::SetEntries(scope.cx_mut(), self.0, rval.handle_mut()) };
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
        let ok = unsafe { wrappers2::SetForEach(scope.cx_mut(), self.0, callback_fn, this_val) };
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

impl Is for Set<'_> {
    fn is(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        Set::is_set(scope, obj)
    }
}

impl<'s> To<Set<'s>> for Object<'s> {
    fn to(&self, scope: &Scope<'_>) -> Result<Set<'s>, JSError> {
        if Set::is(scope, self.0)? {
            Ok(Set(self.0))
        } else {
            Err(JSError)
        }
    }
}

impl<'s> std::ops::Deref for Set<'s> {
    type Target = Object<'s>;

    fn deref(&self) -> &Object<'s> {
        // SAFETY: Set and Object are both repr(transparent) over
        // Handle<'s, *mut JSObject>.
        unsafe { &*(self as *const Set<'s> as *const Object<'s>) }
    }
}
