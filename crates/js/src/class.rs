// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! JSClass definition and standard class initialization.
//!
//! This module wraps SpiderMonkey's class system, providing access to
//! `JS_InitClass`, standard class resolution, and global object creation.

use std::os::raw::c_char;
use std::ptr::NonNull;

use crate::gc::scope::Scope;
use mozjs::gc::{Handle, HandleObject};
use mozjs::jsapi::{
    JSClass, JSFunctionSpec, JSNative, JSObject, JSPrincipals, JSPropertySpec, JSProtoKey,
    OnNewGlobalHookOption, PropertyKey, RealmOptions,
};
use mozjs::rooted;
use mozjs::rust::wrappers2;

use super::error::JSError;

/// Initialize a class on a global object.
///
/// This defines a constructor and prototype, wiring them together.
///
/// # Safety
///
/// All pointer parameters must be valid. `ps`, `fs`, `static_ps`, `static_fs`
/// must be null-terminated arrays or null.
pub unsafe fn init_class<'s>(
    scope: &'s Scope<'_>,
    global: HandleObject,
    proto_class: *const JSClass,
    proto_proto: HandleObject,
    name: *const c_char,
    constructor: JSNative,
    nargs: u32,
    ps: *const JSPropertySpec,
    fs: *const JSFunctionSpec,
    static_ps: *const JSPropertySpec,
    static_fs: *const JSFunctionSpec,
) -> Result<Handle<'s, *mut JSObject>, JSError> {
    let obj = wrappers2::JS_InitClass(
        scope.cx_mut(),
        global,
        proto_class,
        proto_proto,
        name,
        constructor,
        nargs,
        ps,
        fs,
        static_ps,
        static_fs,
    );
    NonNull::new(obj)
        .map(|p| scope.root_object(p))
        .ok_or(JSError)
}

/// Create a new global object.
///
/// # Safety
///
/// `clasp` must be a valid `JSClass` with the `GLOBAL` flag.
/// `principals` may be null. `options` must be valid.
pub unsafe fn new_global_object<'s>(
    scope: &'s Scope<'_>,
    clasp: *const JSClass,
    principals: *mut JSPrincipals,
    hook_option: OnNewGlobalHookOption,
    options: *const RealmOptions,
) -> Result<Handle<'s, *mut JSObject>, JSError> {
    let obj =
        wrappers2::JS_NewGlobalObject(scope.cx_mut(), clasp, principals, hook_option, options);
    NonNull::new(obj)
        .map(|p| scope.root_object(p))
        .ok_or(JSError)
}

/// Initialize the standard classes on a global object.
pub fn init_standard_classes(scope: &Scope<'_>) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::InitRealmStandardClasses(scope.cx_mut()) };
    JSError::check(ok)
}

/// Resolve a standard class by name (lazily).
pub fn resolve_standard_class(
    scope: &Scope<'_>,
    obj: HandleObject,
    id: Handle<PropertyKey>,
) -> Result<bool, JSError> {
    let mut resolved = false;
    let ok = unsafe { wrappers2::JS_ResolveStandardClass(scope.cx_mut(), obj, id, &mut resolved) };
    JSError::check(ok)?;
    Ok(resolved)
}

/// Eagerly enumerate all standard classes on a global object.
pub fn enumerate_standard_classes(scope: &Scope<'_>, obj: HandleObject) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_EnumerateStandardClasses(scope.cx_mut(), obj) };
    JSError::check(ok)
}

/// Get the constructor for a standard class by `JSProtoKey`.
pub fn get_class_object<'s>(
    scope: &'s Scope<'_>,
    key: JSProtoKey,
) -> Result<Handle<'s, *mut JSObject>, JSError> {
    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut objp: *mut JSObject = std::ptr::null_mut());
    let ok = unsafe { wrappers2::JS_GetClassObject(scope.cx_mut(), key, objp.handle_mut()) };
    JSError::check(ok)?;
    NonNull::new(objp.get())
        .map(|p| scope.root_object(p))
        .ok_or(JSError)
}

/// Get the prototype for a standard class by `JSProtoKey`.
pub fn get_class_prototype<'s>(
    scope: &'s Scope<'_>,
    key: JSProtoKey,
) -> Result<Handle<'s, *mut JSObject>, JSError> {
    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut objp: *mut JSObject = std::ptr::null_mut());
    let ok = unsafe { wrappers2::JS_GetClassPrototype(scope.cx_mut(), key, objp.handle_mut()) };
    JSError::check(ok)?;
    NonNull::new(objp.get())
        .map(|p| scope.root_object(p))
        .ok_or(JSError)
}

/// Initialize `Reflect.parse` on a global object.
pub fn init_reflect_parse(scope: &Scope<'_>, global: HandleObject) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_InitReflectParse(scope.cx_mut(), global) };
    JSError::check(ok)
}

/// Link a constructor and its prototype.
pub fn link_constructor_and_prototype(
    scope: &Scope<'_>,
    ctor: HandleObject,
    proto: HandleObject,
) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_LinkConstructorAndPrototype(scope.cx_mut(), ctor, proto) };
    JSError::check(ok)
}

/// Fire the `onNewGlobalObject` hook for a newly created global.
pub fn fire_on_new_global_object(scope: &Scope<'_>, global: HandleObject) {
    unsafe { wrappers2::JS_FireOnNewGlobalObject(scope.cx_mut(), global) }
}

/// Check whether an object is an instance of the given `JSClass`.
///
/// This checks the object's direct class — not the prototype chain.
/// Returns `false` for null objects or objects of different classes.
///
/// Unlike `JS_InstanceOf`, this does **not** throw on failure: pass `null`
/// for the `args` parameter to suppress the TypeError.
///
/// # Safety
///
/// `obj` must be a valid rooted object handle. `clasp` must point to a
/// valid `JSClass` that will remain valid for the duration of the call.
pub fn instance_of(scope: &Scope<'_>, obj: HandleObject, clasp: &JSClass) -> bool {
    // Safety: JS_InstanceOf with a null CallArgs pointer performs a
    // non-throwing check: it returns true if `obj` has `clasp` as its
    // direct class, false otherwise.
    unsafe {
        mozjs::jsapi::JS_InstanceOf(
            scope.cx_mut().raw_cx(),
            obj.into(),
            clasp,
            std::ptr::null_mut(),
        )
    }
}
