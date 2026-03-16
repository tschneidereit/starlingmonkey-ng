// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! JavaScript `Map` collection type.

use std::ptr::NonNull;

use crate::builtins::JSType;
use crate::gc::handle::Stack;
use crate::gc::scope::Scope;
use mozjs::jsapi::Value;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2;
use mozjs::rust::{HandleObject, HandleValue};

use crate::error::ExnThrown;
use crate::Object;

/// Marker type for JavaScript `Map` objects.
///
/// Use the `js::Map` alias for [`Stack<'s, Map>`](crate::gc::handle::Stack)
/// as the scope-rooted handle type:
///
/// ```ignore
/// let map = js::Map::new(&scope)?;
/// map.insert(&scope, key.handle(), val.handle())?;
/// ```
pub struct Map;

impl JSType for Map {
    const JS_NAME: &'static str = "Map";

    fn js_class() -> *const mozjs::jsapi::JSClass {
        crate::class::proto_key_to_class(mozjs::jsapi::JSProtoKey::JSProto_Map)
    }
}

impl<'s> Stack<'s, Map> {
    /// Create a new empty `Map` object.
    pub fn new(scope: &'s Scope<'_>) -> Result<Self, ExnThrown> {
        let obj = unsafe { wrappers2::NewMapObject(scope.cx_mut()) };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(ExnThrown)
    }

    /// Get the number of entries.
    pub fn size(&self, scope: &Scope<'_>) -> u32 {
        unsafe { wrappers2::MapSize(scope.cx(), self.handle()) }
    }

    /// Look up a value by key.
    ///
    /// This is named `lookup` rather than `get` to avoid confusion with
    /// `Handle::get`.
    pub fn lookup(&self, scope: &Scope<'_>, key: HandleValue) -> Result<Value, ExnThrown> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok =
            unsafe { wrappers2::MapGet(scope.cx_mut(), self.handle(), key, rval.handle_mut()) };
        ExnThrown::check(ok)?;
        Ok(rval.get())
    }

    /// Check whether the map contains a key.
    pub fn has(&self, scope: &Scope<'_>, key: HandleValue) -> Result<bool, ExnThrown> {
        let mut result = false;
        let ok = unsafe { wrappers2::MapHas(scope.cx_mut(), self.handle(), key, &mut result) };
        ExnThrown::check(ok)?;
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
    ) -> Result<(), ExnThrown> {
        let ok = unsafe { wrappers2::MapSet(scope.cx_mut(), self.handle(), key, val) };
        ExnThrown::check(ok)
    }

    /// Delete a key. Returns whether the key was present.
    pub fn delete(&self, scope: &Scope<'_>, key: HandleValue) -> Result<bool, ExnThrown> {
        let mut deleted = false;
        let ok = unsafe { wrappers2::MapDelete(scope.cx_mut(), self.handle(), key, &mut deleted) };
        ExnThrown::check(ok)?;
        Ok(deleted)
    }

    /// Remove all entries from the map.
    pub fn clear(&self, scope: &Scope<'_>) -> Result<(), ExnThrown> {
        let ok = unsafe { wrappers2::MapClear(scope.cx(), self.handle()) };
        ExnThrown::check(ok)
    }

    /// Get an iterator over the map's keys.
    pub fn keys(&self, scope: &Scope<'_>) -> Result<Value, ExnThrown> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = unsafe { wrappers2::MapKeys(scope.cx_mut(), self.handle(), rval.handle_mut()) };
        ExnThrown::check(ok)?;
        Ok(rval.get())
    }

    /// Get an iterator over the map's values.
    pub fn values(&self, scope: &Scope<'_>) -> Result<Value, ExnThrown> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = unsafe { wrappers2::MapValues(scope.cx_mut(), self.handle(), rval.handle_mut()) };
        ExnThrown::check(ok)?;
        Ok(rval.get())
    }

    /// Get an iterator over the map's entries (key-value pairs).
    pub fn entries(&self, scope: &Scope<'_>) -> Result<Value, ExnThrown> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = unsafe { wrappers2::MapEntries(scope.cx_mut(), self.handle(), rval.handle_mut()) };
        ExnThrown::check(ok)?;
        Ok(rval.get())
    }

    /// Call a callback for each entry in the map.
    pub fn for_each(
        &self,
        scope: &Scope<'_>,
        callback_fn: HandleValue,
        this_val: HandleValue,
    ) -> Result<(), ExnThrown> {
        let ok =
            unsafe { wrappers2::MapForEach(scope.cx_mut(), self.handle(), callback_fn, this_val) };
        ExnThrown::check(ok)
    }

    /// Check whether an object is a `Map`.
    pub fn is_map(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, ExnThrown> {
        let mut result = false;
        // SAFETY: cx and obj are valid; IsMapObject writes to result.
        let ok = unsafe { wrappers2::IsMapObject(scope.cx_mut(), obj, &mut result) };
        ExnThrown::check(ok)?;
        Ok(result)
    }
}

impl<'s> std::ops::Deref for Stack<'s, Map> {
    type Target = Object<'s>;

    fn deref(&self) -> &Object<'s> {
        // SAFETY: Stack<Map> and Stack<Object> are both repr(transparent)
        // over Handle<'s, *mut JSObject>.
        unsafe { &*(self as *const Stack<'s, Map> as *const Object<'s>) }
    }
}
