// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! ES module compilation, linking, and evaluation.
//!
//! This module wraps SpiderMonkey's ES module API, providing access to
//! compiling modules from source, linking them, evaluating them, and
//! inspecting their requested imports and namespace.

use std::ptr::NonNull;

use crate::gc::scope::Scope;
use crate::{Object, Promise};
use mozjs::gc::{Handle, HandleObject, HandleString};
use mozjs::jsapi::mozilla::Utf8Unit;
use mozjs::jsapi::{
    JSObject, JSScript, JSString, ModuleErrorBehaviour, ModuleType, ReadOnlyCompileOptions,
    SourceText,
};
use mozjs::rooted;
use mozjs::rust::wrappers2;

use super::error::ExnThrown;

/// Compile an ES module from UTF-16 source.
///
/// # Safety
///
/// `options` and `src_buf` must be valid pointers.
pub unsafe fn compile_module_utf16<'s>(
    scope: &'s Scope<'_>,
    options: *const ReadOnlyCompileOptions,
    src_buf: *mut SourceText<u16>,
) -> Result<Handle<'s, *mut JSObject>, ExnThrown> {
    let obj = wrappers2::CompileModule(scope.cx_mut(), options, src_buf);
    NonNull::new(obj)
        .map(|p| scope.root_object(p))
        .ok_or(ExnThrown)
}

/// Compile an ES module from UTF-8 source.
///
/// # Safety
///
/// `options` and `src_buf` must be valid pointers.
pub unsafe fn compile_module<'s>(
    scope: &'s Scope<'_>,
    options: *const ReadOnlyCompileOptions,
    src_buf: *mut SourceText<Utf8Unit>,
) -> Result<Object<'s>, ExnThrown> {
    let obj = wrappers2::CompileModule1(scope.cx_mut(), options, src_buf);
    Object::from_raw(scope, obj).ok_or(ExnThrown)
}

/// Compile a JSON module from UTF-8 source.
///
/// # Safety
///
/// `options` and `src_buf` must be valid pointers.
pub unsafe fn compile_json_module<'s>(
    scope: &'s Scope<'_>,
    options: *const ReadOnlyCompileOptions,
    src_buf: *mut SourceText<Utf8Unit>,
) -> Result<Object<'s>, ExnThrown> {
    let obj = wrappers2::CompileJsonModule1(scope.cx_mut(), options, src_buf);
    Object::from_raw(scope, obj).ok_or(ExnThrown)
}

/// Link a compiled module, resolving its imports.
///
/// This must be called after compilation and before evaluation.
pub fn link(scope: &Scope<'_>, module_record: Object) -> Result<(), ExnThrown> {
    let ok = unsafe { wrappers2::ModuleLink(scope.cx_mut(), module_record.handle()) };
    ExnThrown::check(ok)
}

/// Evaluate a linked module.
///
/// Returns the evaluation result (typically a promise for top-level await).
pub fn evaluate(
    scope: &Scope<'_>,
    module_record: Object,
) -> Result<mozjs::jsapi::Value, ExnThrown> {
    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = mozjs::jsval::UndefinedValue());
    let ok = unsafe {
        wrappers2::ModuleEvaluate(scope.cx_mut(), module_record.handle(), rval.handle_mut())
    };
    ExnThrown::check(ok)?;
    Ok(rval.get())
}

/// Throw if module evaluation failed.
pub fn throw_on_evaluation_failure(
    scope: &Scope<'_>,
    evaluation_promise: Promise,
    error_behaviour: ModuleErrorBehaviour,
) -> Result<(), ExnThrown> {
    let ok = unsafe {
        wrappers2::ThrowOnModuleEvaluationFailure(
            scope.cx_mut(),
            evaluation_promise.handle(),
            error_behaviour,
        )
    };
    ExnThrown::check(ok)
}

/// Get the number of requested module imports.
pub fn get_requested_modules_count(scope: &Scope<'_>, module_record: Object) -> u32 {
    unsafe { wrappers2::GetRequestedModulesCount(scope.cx(), module_record.handle()) }
}

/// Get the module specifier string for a requested import at `index`.
pub fn get_requested_module_specifier(
    scope: &Scope<'_>,
    module_record: Object,
    index: u32,
) -> Option<NonNull<JSString>> {
    NonNull::new(unsafe {
        wrappers2::GetRequestedModuleSpecifier(scope.cx_mut(), module_record.handle(), index)
    })
}

/// Get the module type for a requested import at `index`.
pub fn get_requested_module_type(
    scope: &Scope<'_>,
    module_record: Object,
    index: u32,
) -> ModuleType {
    unsafe { wrappers2::GetRequestedModuleType(scope.cx(), module_record.handle(), index) }
}

/// Get the `JSScript` associated with a module record.
pub fn get_module_script(module_record: Object) -> Option<NonNull<JSScript>> {
    NonNull::new(unsafe { wrappers2::GetModuleScript(module_record.handle()) })
}

/// Create a module request object with a specifier and type.
pub fn create_module_request<'s>(
    scope: &'s Scope<'_>,
    specifier: HandleString,
    module_type: ModuleType,
) -> Result<Handle<'s, *mut JSObject>, ExnThrown> {
    let obj = unsafe { wrappers2::CreateModuleRequest(scope.cx_mut(), specifier, module_type) };
    NonNull::new(obj)
        .map(|p| scope.root_object(p))
        .ok_or(ExnThrown)
}

/// Get the specifier string of a module request.
pub fn get_module_request_specifier(
    scope: &Scope<'_>,
    module_request: HandleObject,
) -> Option<NonNull<JSString>> {
    NonNull::new(unsafe { wrappers2::GetModuleRequestSpecifier(scope.cx(), module_request) })
}

/// Get the type of a module request.
pub fn get_module_request_type(scope: &Scope<'_>, module_request: HandleObject) -> ModuleType {
    unsafe { wrappers2::GetModuleRequestType(scope.cx(), module_request) }
}

/// Get the namespace object of a module.
pub fn get_namespace<'s>(
    scope: &'s Scope<'_>,
    module_record: HandleObject,
) -> Result<Handle<'s, *mut JSObject>, ExnThrown> {
    let obj = unsafe { wrappers2::GetModuleNamespace(scope.cx_mut(), module_record) };
    NonNull::new(obj)
        .map(|p| scope.root_object(p))
        .ok_or(ExnThrown)
}

/// Get the module for a given namespace object.
pub fn get_module_for_namespace(
    scope: &Scope<'_>,
    module_namespace: HandleObject,
) -> Option<NonNull<JSObject>> {
    NonNull::new(unsafe { wrappers2::GetModuleForNamespace(scope.cx(), module_namespace) })
}

/// Get the module environment (lexical scope) object.
pub fn get_environment(scope: &Scope<'_>, module_obj: HandleObject) -> Option<NonNull<JSObject>> {
    NonNull::new(unsafe { wrappers2::GetModuleEnvironment(scope.cx(), module_obj) })
}

/// Set the module resolve hook on the runtime.
///
/// The hook is called by SpiderMonkey when an `import` statement needs to
/// resolve a module specifier to a compiled module object.
///
/// # Safety
///
/// `rt` must be a valid `JSRuntime` pointer. `hook` must be a valid function
/// pointer (or `None` to clear the hook).
pub unsafe fn set_module_resolve_hook(
    rt: *mut mozjs::jsapi::JSRuntime,
    hook: mozjs::jsapi::ModuleResolveHook,
) {
    mozjs::jsapi::SetModuleResolveHook(rt, hook);
}
