// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! JavaScript `WeakMap` collection type.

use std::ptr::NonNull;

use crate::gc::scope::Scope;
use mozjs::gc::Handle;
use mozjs::jsapi::{JSObject, Value};
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2;
use mozjs::rust::{HandleObject, HandleValue};

use crate::builtins::{Is, IsValue, To};
use crate::error::JSError;
use crate::object::Object;

/// A JavaScript `WeakMap` object, rooted in a scope's pool.
///
/// `WeakMap<'s>` wraps a `Handle<'s, *mut JSObject>` known to be a `WeakMap`.
/// Construction automatically roots in the scope:
///
/// ```ignore
/// let wm = WeakMap::new(&scope)?;
/// wm.insert(&scope, key.handle(), val.handle())?;
/// ```
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct WeakMap<'s>(pub(crate) Handle<'s, *mut JSObject>);

impl<'s> WeakMap<'s> {
    /// Create a new empty `WeakMap` object.
    pub fn new(scope: &'s Scope<'_>) -> Result<Self, JSError> {
        let obj = unsafe { wrappers2::NewWeakMapObject(scope.cx_mut()) };
        NonNull::new(obj)
            .map(|nn| WeakMap(scope.root_object(nn)))
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

    /// Wrap an existing rooted handle in a `WeakMap`.
    pub fn from_handle(handle: Handle<'s, *mut JSObject>) -> Self {
        WeakMap(handle)
    }

    /// Look up a value by key.
    ///
    /// This is named `lookup` rather than `get` to avoid confusion with
    /// `Handle::get`.
    pub fn lookup(&self, scope: &Scope<'_>, key: HandleValue) -> Result<Value, JSError> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = unsafe { wrappers2::GetWeakMapEntry(scope.cx(), self.0, key, rval.handle_mut()) };
        JSError::check(ok)?;
        Ok(rval.get())
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
        let ok = unsafe { wrappers2::SetWeakMapEntry(scope.cx_mut(), self.0, key, val) };
        JSError::check(ok)
    }

    /// Check whether an object is a `WeakMap`.
    pub fn is_weak_map(_scope: &Scope<'_>, obj: Object) -> Result<bool, JSError> {
        // SAFETY: IsWeakMapObject only inspects the object's class pointer.
        // It does not allocate, trigger GC, or use cx.
        let result = unsafe { mozjs::jsapi::JS::IsWeakMapObject(obj.get()) };
        Ok(result)
    }
}

impl Is for WeakMap<'_> {
    fn is(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        WeakMap::is_weak_map(scope, Object::from_handle(obj))
    }
}

impl IsValue for WeakMap<'_> {
    fn is_value(scope: &Scope<'_>, val: HandleValue) -> Result<bool, JSError> {
        match Object::from_value(scope, val.get()) {
            Ok(obj) => WeakMap::is_weak_map(scope, obj),
            _ => Ok(false),
        }
    }
}

impl<'s> To<WeakMap<'s>> for Object<'s> {
    fn to(&self, scope: &Scope<'_>) -> Result<WeakMap<'s>, JSError> {
        if WeakMap::is(scope, self.0)? {
            Ok(WeakMap(self.0))
        } else {
            Err(JSError)
        }
    }
}

impl<'s> std::ops::Deref for WeakMap<'s> {
    type Target = Object<'s>;

    fn deref(&self) -> &Object<'s> {
        // SAFETY: WeakMap and Object are both repr(transparent) over
        // Handle<'s, *mut JSObject>.
        unsafe { &*(self as *const WeakMap<'s> as *const Object<'s>) }
    }
}
