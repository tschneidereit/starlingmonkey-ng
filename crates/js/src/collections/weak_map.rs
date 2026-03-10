// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! JavaScript `WeakMap` collection type.

use std::ptr::NonNull;

use crate::gc::handle::{JsType, Stack};
use crate::gc::scope::Scope;
use mozjs::jsapi::Value;
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2;
use mozjs::rust::{HandleObject, HandleValue};

use crate::builtins::{Is, IsValue, To};
use crate::error::JSError;
use crate::Object;

/// Marker type for JavaScript `WeakMap` objects.
///
/// Use the `js::WeakMap` alias for [`Stack<'s, WeakMap>`](crate::gc::handle::Stack) as the scope-rooted
/// handle type:
///
/// ```ignore
/// let wm = js::WeakMap::new(&scope)?;
/// wm.insert(&scope, key.handle(), val.handle())?;
/// ```
pub struct WeakMap;

impl JsType for WeakMap {
    const JS_NAME: &'static str = "WeakMap";
}

impl<'s> Stack<'s, WeakMap> {
    /// Create a new empty `WeakMap` object.
    pub fn new(scope: &'s Scope<'_>) -> Result<Self, JSError> {
        let obj = unsafe { wrappers2::NewWeakMapObject(scope.cx_mut()) };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(JSError)
    }

    /// Look up a value by key.
    ///
    /// This is named `lookup` rather than `get` to avoid confusion with
    /// `Handle::get`.
    pub fn lookup(&self, scope: &Scope<'_>, key: HandleValue) -> Result<Value, JSError> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
        let ok = unsafe {
            wrappers2::GetWeakMapEntry(scope.cx(), self.handle(), key, rval.handle_mut())
        };
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
        let ok = unsafe { wrappers2::SetWeakMapEntry(scope.cx_mut(), self.handle(), key, val) };
        JSError::check(ok)
    }

    /// Check whether an object is a `WeakMap`.
    pub fn is_weak_map(_scope: &Scope<'_>, obj: Object) -> Result<bool, JSError> {
        // SAFETY: IsWeakMapObject only inspects the object's class pointer.
        // It does not allocate, trigger GC, or use cx.
        let result = unsafe { mozjs::jsapi::JS::IsWeakMapObject(obj.as_raw()) };
        Ok(result)
    }
}

impl Is for WeakMap {
    fn is(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        Stack::<WeakMap>::is_weak_map(scope, Object::from_handle(obj))
    }
}

impl IsValue for WeakMap {
    fn is_value(scope: &Scope<'_>, val: HandleValue) -> Result<bool, JSError> {
        match Object::from_value(scope, val.get()) {
            Ok(obj) => Stack::<WeakMap>::is_weak_map(scope, obj),
            _ => Ok(false),
        }
    }
}

impl<'s> To<Stack<'s, WeakMap>> for Object<'s> {
    fn to(&self, scope: &Scope<'_>) -> Result<Stack<'s, WeakMap>, JSError> {
        if WeakMap::is(scope, self.handle())? {
            // SAFETY: We just verified the object is a WeakMap.
            Ok(unsafe { Stack::from_handle_unchecked(self.handle()) })
        } else {
            Err(JSError)
        }
    }
}

impl<'s> std::ops::Deref for Stack<'s, WeakMap> {
    type Target = Object<'s>;

    fn deref(&self) -> &Object<'s> {
        // SAFETY: Stack<WeakMap> and Stack<Object> are both repr(transparent)
        // over Handle<'s, *mut JSObject>.
        unsafe { &*(self as *const Stack<'s, WeakMap> as *const Object<'s>) }
    }
}
