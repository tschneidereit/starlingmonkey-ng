// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Cross-compartment wrapper operations.
//!
//! SpiderMonkey uses compartments to isolate different security domains.
//! When an object from one compartment needs to be accessed from another,
//! a cross-compartment wrapper (CCW) is used. This module provides safe
//! wrappers for CCW management and compartment queries.

use std::ptr::NonNull;

use crate::gc::scope::Scope;
use mozjs::gc::HandleObject;
use mozjs::jsapi::{
    CompartmentFilter, JSClass, JSObject, NukeReferencesFromTarget, NukeReferencesToWindow, Realm,
};
use mozjs::rust::wrappers2;
use mozjs::rust::MutableHandleObject;

use super::error::JSError;

/// Wrap an object for use in the current compartment.
///
/// If the object is already in the current compartment, this is a no-op.
/// Otherwise, a cross-compartment wrapper is created or reused.
pub fn wrap_object(scope: &Scope<'_>, objp: MutableHandleObject) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_WrapObject(scope.cx_mut(), objp) };
    JSError::check(ok)
}

/// Refresh existing cross-compartment wrappers to an object.
pub fn refresh_cross_compartment_wrappers(
    scope: &Scope<'_>,
    obj: HandleObject,
) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_RefreshCrossCompartmentWrappers(scope.cx_mut(), obj) };
    JSError::check(ok)
}

/// Nuke cross-compartment wrappers matching the given filters.
///
/// # Safety
///
/// `source_filter` and `target` must be valid pointers.
pub unsafe fn nuke_cross_compartment_wrappers(
    scope: &Scope<'_>,
    source_filter: *const CompartmentFilter,
    target: *mut Realm,
    nuke_references_to_window: NukeReferencesToWindow,
    nuke_references_from_target: NukeReferencesFromTarget,
) -> Result<(), JSError> {
    let ok = wrappers2::NukeCrossCompartmentWrappers(
        scope.cx(),
        source_filter,
        target,
        nuke_references_to_window,
        nuke_references_from_target,
    );
    JSError::check(ok)
}

/// Recompute wrappers matching the given compartment filters.
///
/// # Safety
///
/// `source_filter` and `target_filter` must be valid pointers.
pub unsafe fn recompute_wrappers(
    scope: &Scope<'_>,
    source_filter: *const CompartmentFilter,
    target_filter: *const CompartmentFilter,
) -> Result<(), JSError> {
    let ok = wrappers2::RecomputeWrappers(scope.cx_mut(), source_filter, target_filter);
    JSError::check(ok)
}

/// Check whether an object is in the context's current compartment.
pub fn is_object_in_context_compartment(obj: NonNull<JSObject>, scope: &Scope<'_>) -> bool {
    unsafe { wrappers2::IsObjectInContextCompartment(obj.as_ptr(), scope.cx()) }
}

/// Get the number of system compartments.
pub fn system_compartment_count(scope: &Scope<'_>) -> usize {
    unsafe { wrappers2::SystemCompartmentCount(scope.cx()) }
}

/// Get the number of user compartments.
pub fn user_compartment_count(scope: &Scope<'_>) -> usize {
    unsafe { wrappers2::UserCompartmentCount(scope.cx()) }
}

/// Set the window proxy class for the context.
///
/// # Safety
///
/// `clasp` must be a valid `JSClass` pointer that outlives the context.
pub unsafe fn set_window_proxy_class(scope: &Scope<'_>, clasp: *const JSClass) {
    wrappers2::SetWindowProxyClass(scope.cx(), clasp)
}

/// Set the window proxy for a global object.
pub fn set_window_proxy(scope: &Scope<'_>, global: HandleObject, window_proxy: HandleObject) {
    unsafe { wrappers2::SetWindowProxy(scope.cx(), global, window_proxy) }
}
