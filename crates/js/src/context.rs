// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Context options, callbacks, and memory management.
//!
//! This module provides access to `JSContext`-level configuration, including
//! interrupt callbacks, private data, stack quotas, and runtime queries.

use std::ptr::NonNull;

use crate::gc::scope::Scope;
use mozjs::jsapi::{ContextOptions, JSInterruptCallback, JSRuntime, NativeStackSize};
use mozjs::rust::wrappers2;

use super::error::ExnThrown;

/// Get a reference to the context's [`ContextOptions`].
///
/// The returned reference borrows the context, so it is valid as long as the
/// borrow is held.
pub fn options<'a>(scope: &'a Scope<'_>) -> &'a ContextOptions {
    // SAFETY: ContextOptionsRef returns a non-null pointer to options that
    // live as long as the JSContext.
    unsafe { &*wrappers2::ContextOptionsRef(scope.cx()) }
}

/// Get a mutable reference to the context's [`ContextOptions`].
///
/// The returned reference borrows the context, so it is valid as long as the
/// borrow is held.
#[allow(clippy::mut_from_ref)]
pub fn options_mut<'a>(scope: &'a Scope<'_>) -> &'a mut ContextOptions {
    // SAFETY: ContextOptionsRef returns a non-null pointer to options that
    // live as long as the JSContext.
    unsafe { &mut *wrappers2::ContextOptionsRef(scope.cx()) }
}

/// Get the private data pointer associated with the context.
pub fn get_private(scope: &Scope<'_>) -> *mut std::os::raw::c_void {
    unsafe { wrappers2::JS_GetContextPrivate(scope.cx()) }
}

/// Set the private data pointer on the context.
///
/// # Safety
///
/// The caller must ensure `data` remains valid for the lifetime of the context
/// or until replaced.
pub unsafe fn set_private(scope: &Scope<'_>, data: *mut std::os::raw::c_void) {
    wrappers2::JS_SetContextPrivate(scope.cx(), data)
}

/// Get the parent `JSRuntime` for this context.
pub fn get_runtime(scope: &Scope<'_>) -> NonNull<JSRuntime> {
    let ptr = unsafe { wrappers2::JS_GetRuntime(scope.cx()) };
    // SAFETY: a valid JSContext always has a non-null runtime.
    NonNull::new(ptr).expect("JSContext runtime should never be null")
}

/// Set native stack size quotas for the context.
///
/// # Safety
///
/// Must be called before any script execution on this context.
pub unsafe fn set_native_stack_quota(
    scope: &Scope<'_>,
    system_code: NativeStackSize,
    trusted_script: NativeStackSize,
    untrusted_script: NativeStackSize,
) {
    wrappers2::JS_SetNativeStackQuota(scope.cx(), system_code, trusted_script, untrusted_script)
}

/// Add an interrupt callback.
///
/// The callback is invoked when the engine is interrupted (e.g., by
/// `JS_RequestInterruptCallback`). Returns `false` to cancel execution.
///
/// # Safety
///
/// `callback` must be a valid function pointer that remains valid for the
/// lifetime of the context.
pub unsafe fn add_interrupt_callback(scope: &Scope<'_>, callback: JSInterruptCallback) -> bool {
    wrappers2::JS_AddInterruptCallback(scope.cx(), callback)
}

/// Check for a pending interrupt and invoke any registered callbacks.
pub fn check_for_interrupt(scope: &Scope<'_>) -> Result<(), ExnThrown> {
    let ok = unsafe { wrappers2::JS_CheckForInterrupt(scope.cx_mut()) };
    ExnThrown::check(ok)
}

/// Request an interrupt callback (thread-safe).
pub fn request_interrupt_callback(scope: &Scope<'_>) {
    unsafe { wrappers2::JS_RequestInterruptCallback(scope.cx()) }
}

/// Allocate memory tracked by the GC.
///
/// # Safety
///
/// The returned pointer must be freed with [`free`].
pub unsafe fn malloc(scope: &Scope<'_>, nbytes: usize) -> *mut std::os::raw::c_void {
    wrappers2::JS_malloc(scope.cx(), nbytes)
}

/// Free memory previously allocated with [`malloc`].
///
/// # Safety
///
/// `p` must have been allocated with [`malloc`] on the same context.
pub unsafe fn free(scope: &Scope<'_>, p: *mut std::os::raw::c_void) {
    wrappers2::JS_free(scope.cx(), p)
}
