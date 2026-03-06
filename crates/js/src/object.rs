// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Object creation, property access, prototype chain, and object utilities.
//!
//! The [`Object`] newtype wraps a scope-rooted `Handle<'s, *mut JSObject>`,
//! making it safe by construction — no manual `rooted!` needed. All other
//! builtins (Array, Promise, etc.) implement `Deref` to `Object`, so these
//! methods are available on every builtin type.
//!
//! # Property Access
//!
//! Properties can be accessed by name (`&CStr`) or by index (`u32`):
//!
//! ```ignore
//! use crate::object::Object;
//!
//! let obj = Object::new(&scope, None)?;
//! obj.set_property(&scope, c"foo", val)?;
//! let val = obj.get_property(&scope, c"foo")?;
//! ```

use std::ffi::CStr;
use std::os::raw::c_uint;
use std::ptr::NonNull;

use crate::error::ConversionError;
use crate::gc::scope::Scope;
use crate::value;
use mozjs::conversions::ToJSValConvertible;
use mozjs::gc::{
    Handle, HandleId, HandleObject, HandleValue, MutableHandle, MutableHandleObject,
    MutableHandleValue,
};
use mozjs::jsapi::{
    HandleObject as RawHandleObject, JSClass, JSFunctionSpec, JSObject, JSPropertySpec,
    ObjectOpResult, PropertyDescriptor, Value,
};
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2;

use super::error::JSError;

/// A JavaScript object, rooted in a scope's [`HandlePool`](crate::gc::pool::HandlePool).
///
/// `Object<'s>` is `Copy` and wraps a `Handle<'s, *mut JSObject>` — the
/// lifetime `'s` ties it to the scope that rooted it. Construction via
/// [`Object::new`] automatically roots in the pool, so no `rooted!` macro
/// is needed:
///
/// ```ignore
/// let obj = Object::new(&scope, None)?;
/// obj.set_property(&scope, c"x", val)?;
/// ```
///
/// All builtin types ([`Array`](super::array::Array),
/// [`Promise`](super::promise::Promise), etc.) implement `Deref` to `Object`,
/// so all property access methods are available on every builtin.
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct Object<'s>(pub(crate) Handle<'s, *mut JSObject>);

impl<'s> Object<'s> {
    // ---------------------------------------------------------------------------
    // Object creation
    // ---------------------------------------------------------------------------

    /// Create a new object with the given class, or a plain object if `clasp` is
    /// `None`. The object is rooted in the scope's pool.
    pub fn new(scope: &'s Scope<'_>, clasp: Option<&'static JSClass>) -> Result<Self, JSError> {
        let raw = scope.cx_mut();
        let obj = unsafe {
            match clasp {
                Some(c) => wrappers2::JS_NewObject(raw, c),
                None => wrappers2::JS_NewPlainObject(raw),
            }
        };
        NonNull::new(obj)
            .map(|nn| Object(scope.root_object(nn)))
            .ok_or(JSError)
    }

    /// Create a new plain object (`{}`), rooted in the scope's pool.
    pub fn new_plain(scope: &'s Scope<'_>) -> Result<Self, JSError> {
        let obj = unsafe { wrappers2::JS_NewPlainObject(scope.cx_mut()) };
        NonNull::new(obj)
            .map(|nn| Object(scope.root_object(nn)))
            .ok_or(JSError)
    }

    /// Create a new object with a specific prototype.
    pub fn new_with_proto(
        scope: &'s Scope<'_>,
        clasp: &'static JSClass,
        proto: Object<'s>,
    ) -> Result<Self, JSError> {
        let obj =
            unsafe { wrappers2::JS_NewObjectWithGivenProto(scope.cx_mut(), clasp, proto.handle()) };
        NonNull::new(obj)
            .map(|nn| Object(scope.root_object(nn)))
            .ok_or(JSError)
    }

    /// Create a new object for use inside a constructor (`new.target`).
    pub fn new_for_constructor(
        scope: &'s Scope<'_>,
        clasp: &'static JSClass,
        args: &mozjs::jsapi::CallArgs,
    ) -> Result<Self, JSError> {
        let obj = unsafe { wrappers2::JS_NewObjectForConstructor(scope.cx_mut(), clasp, args) };
        NonNull::new(obj)
            .map(|nn| Object(scope.root_object(nn)))
            .ok_or(JSError)
    }

    // ---------------------------------------------------------------------------
    // Pointer access
    // ---------------------------------------------------------------------------

    /// Get the rooted handle to the underlying `JSObject`.
    pub fn handle(&'_ self) -> HandleObject<'_> {
        self.0
    }

    /// Get the lifetime-erased handle to the underlying `JSObject`.
    pub(crate) fn raw_handle(&self) -> RawHandleObject {
        self.handle().into()
    }

    /// Get a raw `NonNull` pointer to the underlying `JSObject`.
    pub fn as_non_null(self) -> Option<NonNull<JSObject>> {
        NonNull::new(self.0.get())
    }

    /// Get the raw `*mut JSObject` pointer.
    pub fn as_raw(self) -> *mut JSObject {
        self.0.get()
    }

    /// Wrap a raw `*mut JSObject` pointer, rooting it in the scope's pool.
    ///
    /// Returns `None` if `ptr` is null.
    pub fn from_raw(scope: &'s Scope<'_>, ptr: *mut JSObject) -> Option<Self> {
        NonNull::new(ptr).map(|nn| Object(scope.root_object(nn)))
    }

    pub fn from_value(scope: &'s Scope<'_>, val: Value) -> Result<Self, ConversionError> {
        if val.is_object() {
            Ok(Self::from_raw(scope, val.to_object()).unwrap())
        } else {
            Err(ConversionError("Value isn't an object"))
        }
    }

    /// Wrap an existing rooted handle in an `Object`.
    ///
    /// This is useful for interop with code that already has rooted handles.
    pub fn from_handle(handle: Handle<'s, *mut JSObject>) -> Self {
        Object(handle)
    }

    // -----------------------------------------------------------------------
    // Property access
    // -----------------------------------------------------------------------

    /// Get a property by name.
    #[inline]
    pub fn get_property(&self, scope: &Scope<'_>, name: &CStr) -> Result<Value, JSError> {
        get_property(scope, self.0, name)
    }

    /// Get a property by property key (jsid).
    #[inline]
    pub fn get_property_by_id(&self, scope: &Scope<'_>, id: HandleId) -> Result<Value, JSError> {
        get_property_by_id(scope, self.0, id)
    }

    /// Get an array element by index.
    #[inline]
    pub fn get_element(&self, scope: &Scope<'_>, index: u32) -> Result<Value, JSError> {
        get_element(scope, self.0, index)
    }

    /// Set a property by name.
    #[inline]
    pub fn set_property(
        &self,
        scope: &Scope<'_>,
        name: &CStr,
        value: HandleValue,
    ) -> Result<(), JSError> {
        set_property(scope, self.0, name, value)
    }

    /// Set a property by property key (jsid).
    #[inline]
    pub fn set_property_by_id(
        &self,
        scope: &Scope<'_>,
        id: HandleId,
        value: HandleValue,
    ) -> Result<(), JSError> {
        set_property_by_id(scope, self.0, id, value)
    }

    /// Set an array element by index.
    #[inline]
    pub fn set_element(
        &self,
        scope: &Scope<'_>,
        index: u32,
        value: HandleValue,
    ) -> Result<(), JSError> {
        set_element(scope, self.0, index, value)
    }

    /// Check whether this object has a property with the given name.
    #[inline]
    pub fn has_property(&self, scope: &Scope<'_>, name: &CStr) -> Result<bool, JSError> {
        has_property(scope, self.0, name)
    }

    /// Check whether this object has a property with the given id.
    #[inline]
    pub fn has_property_by_id(&self, scope: &Scope<'_>, id: HandleId) -> Result<bool, JSError> {
        has_property_by_id(scope, self.0, id)
    }

    /// Check whether this object has an element at the given index.
    #[inline]
    pub fn has_element(&self, scope: &Scope<'_>, index: u32) -> Result<bool, JSError> {
        has_element(scope, self.0, index)
    }

    /// Check whether this object has an own property with the given name.
    #[inline]
    pub fn has_own_property(&self, scope: &Scope<'_>, name: &CStr) -> Result<bool, JSError> {
        has_own_property(scope, self.0, name)
    }

    /// Check whether this object has an own property with the given id.
    #[inline]
    pub fn has_own_property_by_id(&self, scope: &Scope<'_>, id: HandleId) -> Result<bool, JSError> {
        has_own_property_by_id(scope, self.0, id)
    }

    /// Delete a property by name.
    #[inline]
    pub fn delete_property(&self, scope: &Scope<'_>, name: &CStr) -> Result<(), JSError> {
        delete_property(scope, self.0, name)
    }

    /// Delete a property by name, returning the operation result.
    #[inline]
    pub fn delete_property_with_result(
        &self,
        scope: &Scope<'_>,
        name: &CStr,
        result: &mut ObjectOpResult,
    ) -> Result<(), JSError> {
        delete_property_with_result(scope, self.0, name, result)
    }

    /// Delete an element by index.
    #[inline]
    pub fn delete_element(&self, scope: &Scope<'_>, index: u32) -> Result<(), JSError> {
        delete_element(scope, self.0, index)
    }

    // -----------------------------------------------------------------------
    // Property definition
    // -----------------------------------------------------------------------

    /// Define a property with a JS value and attribute flags.
    #[inline]
    pub fn define_property<V: ToJSValConvertible + ?Sized>(
        &self,
        scope: &Scope<'_>,
        name: &CStr,
        value: &V,
        attrs: c_uint,
    ) -> Result<(), JSError> {
        define_property(scope, self.0, name, value, attrs)
    }

    /// Define a property using a property descriptor.
    #[inline]
    pub fn define_property_by_id(
        &self,
        scope: &Scope<'_>,
        id: HandleId,
        desc: Handle<'_, PropertyDescriptor>,
    ) -> Result<(), JSError> {
        define_property_by_id(scope, self.0, id, desc)
    }

    // -----------------------------------------------------------------------
    // Prototype chain
    // -----------------------------------------------------------------------

    /// Get the prototype of this object.
    #[inline]
    pub fn get_prototype(&self, scope: &'s Scope<'_>) -> Result<Object<'s>, JSError> {
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut result: *mut JSObject = std::ptr::null_mut());
        let ok = unsafe { wrappers2::JS_GetPrototype(scope.cx_mut(), self.0, result.handle_mut()) };
        JSError::check(ok)?;
        NonNull::new(result.get())
            .map(|nn| Object(scope.root_object(nn)))
            .ok_or(JSError)
    }

    /// Set the prototype of this object.
    #[inline]
    pub fn set_prototype(&self, scope: &Scope<'_>, proto: HandleObject) -> Result<(), JSError> {
        set_prototype(scope, self.0, proto)
    }

    /// Check whether this object is extensible.
    #[inline]
    pub fn is_extensible(&self, scope: &Scope<'_>) -> Result<bool, JSError> {
        is_extensible(scope, self.0)
    }

    /// Prevent extensions on this object.
    #[inline]
    pub fn prevent_extensions(
        &self,
        scope: &Scope<'_>,
        result: &mut ObjectOpResult,
    ) -> Result<(), JSError> {
        prevent_extensions(scope, self.0, result)
    }

    /// Freeze this object (make all properties non-configurable and
    /// non-writable).
    #[inline]
    pub fn freeze(&self, scope: &Scope<'_>) -> Result<(), JSError> {
        freeze(scope, self.0)
    }

    /// Deep-freeze this object and all objects reachable from it.
    #[inline]
    pub fn deep_freeze(&self, scope: &Scope<'_>) -> Result<(), JSError> {
        deep_freeze(scope, self.0)
    }
}

// ---------------------------------------------------------------------------
// Object creation (free functions)
// ---------------------------------------------------------------------------

/// Define an object property that is itself a new object.
pub fn define_object<'s>(
    scope: &'s Scope<'_>,
    obj: HandleObject,
    name: &CStr,
    clasp: &'static JSClass,
    attrs: c_uint,
) -> Result<Handle<'s, *mut JSObject>, JSError> {
    let child =
        unsafe { wrappers2::JS_DefineObject(scope.cx_mut(), obj, name.as_ptr(), clasp, attrs) };
    NonNull::new(child)
        .map(|p| scope.root_object(p))
        .ok_or(JSError)
}

/// Clone an object (shallow copy).
///
/// The `proto` handle specifies the prototype for the clone.
pub fn clone_object<'s>(
    scope: &'s Scope<'_>,
    obj: HandleObject,
    proto: HandleObject,
) -> Result<Handle<'s, *mut JSObject>, JSError> {
    let cloned = unsafe { wrappers2::JS_CloneObject(scope.cx_mut(), obj, proto) };
    NonNull::new(cloned)
        .map(|p| scope.root_object(p))
        .ok_or(JSError)
}

/// Get a constructor from a prototype object.
pub fn get_constructor<'s>(
    scope: &'s Scope<'_>,
    proto: HandleObject,
) -> Result<Handle<'s, *mut JSObject>, JSError> {
    let ctor = unsafe { wrappers2::JS_GetConstructor(scope.cx_mut(), proto) };
    NonNull::new(ctor)
        .map(|p| scope.root_object(p))
        .ok_or(JSError)
}

/// Get a property descriptor by id.
///
/// `is_none` is set to `true` if the property was not found.
pub fn get_own_property_descriptor(
    scope: &Scope<'_>,
    obj: HandleObject,
    id: HandleId,
    desc: MutableHandle<PropertyDescriptor>,
    is_none: &mut bool,
) -> Result<(), JSError> {
    let ok = unsafe {
        wrappers2::JS_GetOwnPropertyDescriptorById(scope.cx_mut(), obj, id, desc, is_none)
    };
    JSError::check(ok)
}

/// Check whether the object already has an own property with the given name.
pub fn already_has_own_property(
    scope: &Scope<'_>,
    obj: HandleObject,
    name: &CStr,
) -> Result<bool, JSError> {
    let mut found = false;
    let ok = unsafe {
        wrappers2::JS_AlreadyHasOwnProperty(scope.cx_mut(), obj, name.as_ptr(), &mut found)
    };
    JSError::check(ok)?;
    Ok(found)
}

/// Copy all own properties and private fields from `src` to `dst`.
pub fn copy_own_properties_and_private_fields(
    scope: &Scope<'_>,
    target: HandleObject,
    src: HandleObject,
) -> Result<(), JSError> {
    let ok =
        unsafe { wrappers2::JS_CopyOwnPropertiesAndPrivateFields(scope.cx_mut(), target, src) };
    JSError::check(ok)
}

/// Convert a value to an object.
pub fn value_to_object<'s>(
    scope: &'s Scope<'_>,
    val: HandleValue,
) -> Result<Handle<'s, *mut JSObject>, JSError> {
    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut objp: *mut JSObject = std::ptr::null_mut());
    let ok = unsafe { wrappers2::JS_ValueToObject(scope.cx_mut(), val, objp.handle_mut()) };
    JSError::check(ok)?;
    NonNull::new(objp.get())
        .map(|p| scope.root_object(p))
        .ok_or(JSError)
}

// ---------------------------------------------------------------------------
// Property definition
// ---------------------------------------------------------------------------

/// Define a property with a JS value and attribute flags.
///
/// Common attribute flags: `JSPROP_ENUMERATE`, `JSPROP_READONLY`,
/// `JSPROP_PERMANENT`.
pub fn define_property<V: ToJSValConvertible + ?Sized>(
    scope: &Scope<'_>,
    obj: HandleObject,
    name: &CStr,
    value: &V,
    attrs: c_uint,
) -> Result<(), JSError> {
    rooted!(in(unsafe { scope.cx_mut().raw_cx() }) let mut val = value::undefined());
    unsafe {
        value.to_jsval(scope.cx_mut().raw_cx(), val.handle_mut());
    }
    let ok = unsafe {
        wrappers2::JS_DefineProperty(scope.cx_mut(), obj, name.as_ptr(), val.handle(), attrs)
    };
    JSError::check(ok)
}

/// Define a property using a property descriptor.
pub fn define_property_by_id(
    scope: &Scope<'_>,
    obj: HandleObject,
    id: HandleId,
    desc: Handle<PropertyDescriptor>,
) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_DefinePropertyById1(scope.cx_mut(), obj, id, desc) };
    JSError::check(ok)
}

/// Define a property with a descriptor and get the operation result.
pub fn define_property_by_id_with_result(
    scope: &Scope<'_>,
    obj: HandleObject,
    id: HandleId,
    desc: Handle<PropertyDescriptor>,
    result: &mut ObjectOpResult,
) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_DefinePropertyById(scope.cx_mut(), obj, id, desc, result) };
    JSError::check(ok)
}

/// Define multiple properties from a static property spec array.
///
/// # Safety
///
/// `ps` must point to a null-terminated array of [`JSPropertySpec`].
pub unsafe fn define_properties(
    scope: &Scope<'_>,
    obj: HandleObject,
    ps: *const JSPropertySpec,
) -> Result<(), JSError> {
    let ok = wrappers2::JS_DefineProperties(scope.cx_mut(), obj, ps);
    JSError::check(ok)
}

/// Define multiple functions from a static function spec array.
///
/// # Safety
///
/// `fs` must point to a null-terminated array of [`JSFunctionSpec`].
pub unsafe fn define_functions(
    scope: &Scope<'_>,
    obj: HandleObject,
    fs: *const JSFunctionSpec,
) -> Result<(), JSError> {
    let ok = wrappers2::JS_DefineFunctions(scope.cx_mut(), obj, fs);
    JSError::check(ok)
}

// ---------------------------------------------------------------------------
// Property access
// ---------------------------------------------------------------------------

/// Get a property by name.
pub fn get_property(scope: &Scope<'_>, obj: HandleObject, name: &CStr) -> Result<Value, JSError> {
    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
    let ok =
        unsafe { wrappers2::JS_GetProperty(scope.cx_mut(), obj, name.as_ptr(), rval.handle_mut()) };
    JSError::check(ok)?;
    Ok(rval.get())
}

/// Get a property by property key (jsid).
pub fn get_property_by_id(
    scope: &Scope<'_>,
    obj: HandleObject,
    id: HandleId,
) -> Result<Value, JSError> {
    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
    let ok = unsafe { wrappers2::JS_GetPropertyById(scope.cx_mut(), obj, id, rval.handle_mut()) };
    JSError::check(ok)?;
    Ok(rval.get())
}

/// Get an array element by index.
pub fn get_element(scope: &Scope<'_>, obj: HandleObject, index: u32) -> Result<Value, JSError> {
    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
    let ok = unsafe { wrappers2::JS_GetElement(scope.cx_mut(), obj, index, rval.handle_mut()) };
    JSError::check(ok)?;
    Ok(rval.get())
}

/// Set a property by name.
pub fn set_property(
    scope: &Scope<'_>,
    obj: HandleObject,
    name: &CStr,
    value: HandleValue,
) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_SetProperty(scope.cx_mut(), obj, name.as_ptr(), value) };
    JSError::check(ok)
}

/// Set a property by property key (jsid).
pub fn set_property_by_id(
    scope: &Scope<'_>,
    obj: HandleObject,
    id: HandleId,
    value: HandleValue,
) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_SetPropertyById(scope.cx_mut(), obj, id, value) };
    JSError::check(ok)
}

/// Set an array element by index.
pub fn set_element(
    scope: &Scope<'_>,
    obj: HandleObject,
    index: u32,
    value: HandleValue,
) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_SetElement(scope.cx_mut(), obj, index, value) };
    JSError::check(ok)
}

/// Check whether an object has a property with the given name.
pub fn has_property(scope: &Scope<'_>, obj: HandleObject, name: &CStr) -> Result<bool, JSError> {
    let mut found = false;
    let ok = unsafe { wrappers2::JS_HasProperty(scope.cx_mut(), obj, name.as_ptr(), &mut found) };
    JSError::check(ok)?;
    Ok(found)
}

/// Check whether an object has a property with the given id.
pub fn has_property_by_id(
    scope: &Scope<'_>,
    obj: HandleObject,
    id: HandleId,
) -> Result<bool, JSError> {
    let mut found = false;
    let ok = unsafe { wrappers2::JS_HasPropertyById(scope.cx_mut(), obj, id, &mut found) };
    JSError::check(ok)?;
    Ok(found)
}

/// Check whether an object has an element at the given index.
pub fn has_element(scope: &Scope<'_>, obj: HandleObject, index: u32) -> Result<bool, JSError> {
    let mut found = false;
    let ok = unsafe { wrappers2::JS_HasElement(scope.cx_mut(), obj, index, &mut found) };
    JSError::check(ok)?;
    Ok(found)
}

/// Check whether an object has an own property with the given name.
pub fn has_own_property(
    scope: &Scope<'_>,
    obj: HandleObject,
    name: &CStr,
) -> Result<bool, JSError> {
    let mut found = false;
    let ok =
        unsafe { wrappers2::JS_HasOwnProperty(scope.cx_mut(), obj, name.as_ptr(), &mut found) };
    JSError::check(ok)?;
    Ok(found)
}

/// Check whether an object has an own property with the given id.
pub fn has_own_property_by_id(
    scope: &Scope<'_>,
    obj: HandleObject,
    id: HandleId,
) -> Result<bool, JSError> {
    let mut found = false;
    let ok = unsafe { wrappers2::JS_HasOwnPropertyById(scope.cx_mut(), obj, id, &mut found) };
    JSError::check(ok)?;
    Ok(found)
}

/// Delete a property by name.
pub fn delete_property(scope: &Scope<'_>, obj: HandleObject, name: &CStr) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_DeleteProperty1(scope.cx_mut(), obj, name.as_ptr()) };
    JSError::check(ok)
}

/// Delete a property by name, returning the operation result.
pub fn delete_property_with_result(
    scope: &Scope<'_>,
    obj: HandleObject,
    name: &CStr,
    result: &mut ObjectOpResult,
) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_DeleteProperty(scope.cx_mut(), obj, name.as_ptr(), result) };
    JSError::check(ok)
}

/// Delete an element by index.
pub fn delete_element(scope: &Scope<'_>, obj: HandleObject, index: u32) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_DeleteElement1(scope.cx_mut(), obj, index) };
    JSError::check(ok)
}

// ---------------------------------------------------------------------------
// Prototype chain
// ---------------------------------------------------------------------------

/// Get the prototype of an object.
pub fn get_prototype<'s>(
    scope: &'s Scope<'_>,
    obj: HandleObject,
) -> Result<Handle<'s, *mut JSObject>, JSError> {
    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut result: *mut JSObject = std::ptr::null_mut());
    let ok = unsafe { wrappers2::JS_GetPrototype(scope.cx_mut(), obj, result.handle_mut()) };
    JSError::check(ok)?;
    NonNull::new(result.get())
        .map(|p| scope.root_object(p))
        .ok_or(JSError)
}

/// Set the prototype of an object.
pub fn set_prototype(
    scope: &Scope<'_>,
    obj: HandleObject,
    proto: HandleObject,
) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_SetPrototype(scope.cx_mut(), obj, proto) };
    JSError::check(ok)
}

/// Check whether an object is extensible.
pub fn is_extensible(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
    let mut extensible = false;
    let ok = unsafe { wrappers2::JS_IsExtensible(scope.cx_mut(), obj, &mut extensible) };
    JSError::check(ok)?;
    Ok(extensible)
}

/// Prevent extensions on an object.
pub fn prevent_extensions(
    scope: &Scope<'_>,
    obj: HandleObject,
    result: &mut ObjectOpResult,
) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_PreventExtensions(scope.cx_mut(), obj, result) };
    JSError::check(ok)
}

/// Freeze an object (make all properties non-configurable and non-writable).
pub fn freeze(scope: &Scope<'_>, obj: HandleObject) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_FreezeObject(scope.cx_mut(), obj) };
    JSError::check(ok)
}

/// Deep-freeze an object and all objects reachable from it.
pub fn deep_freeze(scope: &Scope<'_>, obj: HandleObject) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_DeepFreezeObject(scope.cx_mut(), obj) };
    JSError::check(ok)
}

/// Set an immutable prototype on an object.
///
/// Returns whether the operation succeeded (may fail if the object does not
/// support immutable prototypes).
pub fn set_immutable_prototype(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
    let mut succeeded = false;
    let ok = unsafe { wrappers2::JS_SetImmutablePrototype(scope.cx_mut(), obj, &mut succeeded) };
    JSError::check(ok)?;
    Ok(succeeded)
}

/// Assign all enumerable own properties from `src` to `target`.
pub fn assign(scope: &Scope<'_>, target: HandleObject, src: HandleObject) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_AssignObject(scope.cx_mut(), target, src) };
    JSError::check(ok)
}

/// Get the property keys of an object.
///
/// `flags` controls which properties to include (e.g.,
/// `JSITER_OWNONLY`, `JSITER_HIDDEN`).
pub fn get_property_keys(
    scope: &Scope<'_>,
    obj: HandleObject,
    flags: c_uint,
    props: mozjs::jsapi::MutableHandleIdVector,
) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::GetPropertyKeys(scope.cx_mut(), obj, flags, props) };
    JSError::check(ok)
}

// ---------------------------------------------------------------------------
// Wrap / transplant
// ---------------------------------------------------------------------------

/// Wrap an object for use in the current compartment.
pub fn wrap_object(scope: &Scope<'_>, obj: MutableHandleObject) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_WrapObject(scope.cx_mut(), obj) };
    JSError::check(ok)
}

/// Wrap a value for use in the current compartment.
pub fn wrap_value(scope: &Scope<'_>, vp: MutableHandleValue) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_WrapValue(scope.cx_mut(), vp) };
    JSError::check(ok)
}

/// Transplant an object to a new target.
pub fn transplant<'s>(
    scope: &'s Scope<'_>,
    origobj: HandleObject,
    target: HandleObject,
) -> Result<Handle<'s, *mut JSObject>, JSError> {
    let result = unsafe { wrappers2::JS_TransplantObject(scope.cx_mut(), origobj, target) };
    NonNull::new(result)
        .map(|p| scope.root_object(p))
        .ok_or(JSError)
}

// ---------------------------------------------------------------------------
// Reserved slots
// ---------------------------------------------------------------------------

/// Get the `JSClass` pointer for a JS object.
///
/// # Safety
///
/// - `obj` must be a valid, non-null JS object pointer.
pub unsafe fn get_object_class(obj: *mut JSObject) -> *const JSClass {
    mozjs::rust::get_object_class(obj)
}

/// Return the number of reserved slots for a JS object's class.
///
/// # Safety
///
/// - `obj` must be a valid, non-null JS object pointer.
pub unsafe fn reserved_slot_count(obj: *mut JSObject) -> u32 {
    let clasp = get_object_class(obj);
    let flags = (*clasp).flags;
    (flags >> crate::class_spec::JSCLASS_RESERVED_SLOTS_SHIFT)
        & crate::class_spec::JSCLASS_RESERVED_SLOTS_MASK
}

/// Get the value stored in a reserved slot of a JS object.
///
/// # Safety
///
/// - `obj` must be a valid, non-null JS object pointer.
/// - `slot` must be less than the number of reserved slots for the object's class.
pub unsafe fn get_reserved_slot(obj: *mut JSObject, slot: u32) -> Value {
    let mut val = UndefinedValue();
    mozjs::glue::JS_GetReservedSlot(obj, slot, &mut val);
    val
}

/// Set a value in a reserved slot of a JS object.
///
/// # Safety
///
/// - `obj` must be a valid, non-null JS object pointer.
/// - `slot` must be less than the number of reserved slots for the object's class.
pub unsafe fn set_reserved_slot(obj: *mut JSObject, slot: u32, val: &Value) {
    mozjs::jsapi::JS_SetReservedSlot(obj, slot, val);
}

impl<'s> std::ops::Deref for Object<'s> {
    type Target = Handle<'s, *mut JSObject>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
