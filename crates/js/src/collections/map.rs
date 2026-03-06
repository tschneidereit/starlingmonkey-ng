// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! JavaScript `Map` collection type.

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

/// A JavaScript `Map` object, rooted in a scope's pool.
///
/// `Map<'s>` wraps a `Handle<'s, *mut JSObject>` known to be a `Map`.
/// Construction automatically roots in the scope:
///
/// ```ignore
/// let map = Map::new(&scope)?;
/// map.insert(&scope, key.handle(), val.handle())?;
/// ```
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct Map<'s>(pub(crate) Handle<'s, *mut JSObject>);

impl<'s> Map<'s> {
    /// Create a new empty `Map` object.
    pub fn new(scope: &'s Scope<'_>) -> Result<Self, JSError> {
        let obj = unsafe { wrappers2::NewMapObject(scope.cx_mut()) };
        NonNull::new(obj)
            .map(|nn| Map(scope.root_object(nn)))
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

    /// Wrap an existing rooted handle in a `Map`.
    pub fn from_handle(handle: Handle<'s, *mut JSObject>) -> Self {
        Map(handle)
    }

    /// Get the number of entries.
    pub fn size(&self, scope: &Scope<'_>) -> u32 {
        unsafe { wrappers2::MapSize(scope.cx(), self.0) }
    }

    /// Look up a value by key.
    ///
    /// This is named `lookup` rather than `get` to avoid confusion with
    /// `Handle::get`.
    pub fn lookup(&self, scope: &Scope<'_>, key: HandleValue) -> Result<Value, JSError> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = unsafe { wrappers2::MapGet(scope.cx_mut(), self.0, key, rval.handle_mut()) };
        JSError::check(ok)?;
        Ok(rval.get())
    }

    /// Check whether the map contains a key.
    pub fn has(&self, scope: &Scope<'_>, key: HandleValue) -> Result<bool, JSError> {
        let mut result = false;
        let ok = unsafe { wrappers2::MapHas(scope.cx_mut(), self.0, key, &mut result) };
        JSError::check(ok)?;
        Ok(result)
    }

    /// Insert a key-value pair.
    ///
    /// This is named `insert` rather than `set` to avoid confusion with
    /// `Handle::set`.
    pub fn insert(
        &self,
        scope: &Scope<'_>,
        key: HandleValue,
        val: HandleValue,
    ) -> Result<(), JSError> {
        let ok = unsafe { wrappers2::MapSet(scope.cx_mut(), self.0, key, val) };
        JSError::check(ok)
    }

    /// Delete a key. Returns whether the key was present.
    pub fn delete(&self, scope: &Scope<'_>, key: HandleValue) -> Result<bool, JSError> {
        let mut deleted = false;
        let ok = unsafe { wrappers2::MapDelete(scope.cx_mut(), self.0, key, &mut deleted) };
        JSError::check(ok)?;
        Ok(deleted)
    }

    /// Remove all entries from the map.
    pub fn clear(&self, scope: &Scope<'_>) -> Result<(), JSError> {
        let ok = unsafe { wrappers2::MapClear(scope.cx(), self.0) };
        JSError::check(ok)
    }

    /// Get an iterator over the map's keys.
    pub fn keys(&self, scope: &Scope<'_>) -> Result<Value, JSError> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = unsafe { wrappers2::MapKeys(scope.cx_mut(), self.0, rval.handle_mut()) };
        JSError::check(ok)?;
        Ok(rval.get())
    }

    /// Get an iterator over the map's values.
    pub fn values(&self, scope: &Scope<'_>) -> Result<Value, JSError> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = unsafe { wrappers2::MapValues(scope.cx_mut(), self.0, rval.handle_mut()) };
        JSError::check(ok)?;
        Ok(rval.get())
    }

    /// Get an iterator over the map's entries (key-value pairs).
    pub fn entries(&self, scope: &Scope<'_>) -> Result<Value, JSError> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = unsafe { wrappers2::MapEntries(scope.cx_mut(), self.0, rval.handle_mut()) };
        JSError::check(ok)?;
        Ok(rval.get())
    }

    /// Call a callback for each entry in the map.
    pub fn for_each(
        &self,
        scope: &Scope<'_>,
        callback_fn: HandleValue,
        this_val: HandleValue,
    ) -> Result<(), JSError> {
        let ok = unsafe { wrappers2::MapForEach(scope.cx_mut(), self.0, callback_fn, this_val) };
        JSError::check(ok)
    }

    /// Check whether an object is a `Map`.
    pub fn is_map(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        let mut result = false;
        // SAFETY: cx and obj are valid; IsMapObject writes to result.
        let ok = unsafe { wrappers2::IsMapObject(scope.cx_mut(), obj, &mut result) };
        JSError::check(ok)?;
        Ok(result)
    }
}

impl Is for Map<'_> {
    fn is(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        Map::is_map(scope, obj)
    }
}

impl<'s> To<Map<'s>> for Object<'s> {
    fn to(&self, scope: &Scope<'_>) -> Result<Map<'s>, JSError> {
        if Map::is(scope, self.0)? {
            Ok(Map(self.0))
        } else {
            Err(JSError)
        }
    }
}

impl<'s> std::ops::Deref for Map<'s> {
    type Target = Object<'s>;

    fn deref(&self) -> &Object<'s> {
        // SAFETY: Map and Object are both repr(transparent) over
        // Handle<'s, *mut JSObject>.
        unsafe { &*(self as *const Map<'s> as *const Object<'s>) }
    }
}
