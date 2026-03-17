// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! JavaScript `WeakMap` collection type.

use std::ptr::NonNull;

use crate::builtins::JSType;
use crate::gc::handle::Stack;
use crate::gc::scope::Scope;
use mozjs::jsval::UndefinedValue;
use mozjs::rust::wrappers2;
use mozjs::rust::HandleValue;

use crate::error::ExnThrown;
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

impl JSType for WeakMap {
    const JS_NAME: &'static str = "WeakMap";

    fn js_class() -> *const mozjs::jsapi::JSClass {
        crate::class::proto_key_to_class(mozjs::jsapi::JSProtoKey::JSProto_WeakMap)
    }
}

impl<'s> Stack<'s, WeakMap> {
    /// Create a new empty `WeakMap` object.
    pub fn new(scope: &'s Scope<'_>) -> Result<Self, ExnThrown> {
        let obj = unsafe { wrappers2::NewWeakMapObject(scope.cx_mut()) };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(ExnThrown)
    }

    /// Look up a value by key.
    ///
    /// This is named `lookup` rather than `get` to avoid confusion with
    /// `Handle::get`.
    pub fn lookup<'r>(&self, scope: &'r Scope<'_>, key: HandleValue<'r>) -> Result<HandleValue<'r>, ExnThrown> {
        let mut rval = scope.root_value_mut(UndefinedValue());
        let ok = unsafe {
            wrappers2::GetWeakMapEntry(scope.cx(), self.handle(), key, rval.reborrow())
        };
        ExnThrown::check(ok)?;
        Ok(rval.handle())
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
        let ok = unsafe { wrappers2::SetWeakMapEntry(scope.cx_mut(), self.handle(), key, val) };
        ExnThrown::check(ok)
    }

    /// Check whether an object is a `WeakMap`.
    pub fn is_weak_map(_scope: &Scope<'_>, obj: Object) -> Result<bool, ExnThrown> {
        // SAFETY: IsWeakMapObject only inspects the object's class pointer.
        // It does not allocate, trigger GC, or use cx.
        let result = unsafe { mozjs::jsapi::JS::IsWeakMapObject(obj.as_raw()) };
        Ok(result)
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
