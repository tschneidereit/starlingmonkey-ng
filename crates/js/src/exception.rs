// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Pending exception management.
//!
//! This module provides direct access to the pending exception on a
//! `JSContext`. For the higher-level error type that wraps these operations,
//! see [`super::error::ExnThrown`].

use std::hint::cold_path;

use crate::gc::scope::Scope;
use mozjs::jsapi::ExceptionStackBehavior;
use mozjs::jsval::UndefinedValue;
use mozjs::rust::wrappers2;
use mozjs::rust::HandleValue;

use super::error::ExnThrown;

#[inline]
pub fn check_fn_return(scope: &Scope<'_>, ok: bool, name: &str) -> bool {
    if ok {
        debug_assert!(
            !is_pending(scope),
            "Native function '{name}' returned true but an exception is pending",
        );
    }
    // Note that we can't do the inverse of the above assert: `false` without a pending
    // exception is legitimate, since SpiderMonkey's API can return `false` without a
    // pending exception for uncatchable exceptions.
    ok
}

/// Check whether an exception is pending on the context.
pub fn is_pending(scope: &Scope<'_>) -> bool {
    unsafe { wrappers2::JS_IsExceptionPending(scope.cx()) }
}

/// Check whether the context is throwing an out-of-memory error.
pub fn is_throwing_oom(scope: &Scope<'_>) -> bool {
    unsafe { wrappers2::JS_IsThrowingOutOfMemory(scope.cx()) }
}

/// Get the pending exception value.
///
/// Returns `Err` if no exception is pending or retrieval fails.
pub fn get_pending<'r>(scope: &'r Scope<'_>) -> Result<HandleValue<'r>, &'static str> {
    let mut vp = scope.root_value_mut(UndefinedValue());
    let ok = unsafe { wrappers2::JS_GetPendingException(scope.cx_mut(), vp.reborrow()) };
    if !ok {
        cold_path();
        if !is_pending(scope) {
            return Err("No exception pending");
        }
        return Err("Failed to get pending exception");
    }
    Ok(vp.handle())
}

/// Get and clear the pending exception value.
///
/// Returns `Err` if no exception is pending or retrieval fails.
pub fn take_pending<'r>(scope: &'r Scope<'_>) -> Result<HandleValue<'r>, &'static str> {
    let result = get_pending(scope)?;
    clear(scope);
    Ok(result)
}

/// Get and clear the pending exception value, or return `undefined` if there is none.
///
/// Can be used in contexts where JS execution failed, but an uncatchable exception
/// (e.g., OOM) may be pending.
pub fn take_pending_or_undefined<'r>(scope: &'r Scope<'_>) -> HandleValue<'r> {
    take_pending(scope).unwrap_or_else(|_| HandleValue::undefined())
}

/// Set a pending exception on the context.
pub fn set_pending(
    scope: &Scope<'_>,
    v: HandleValue,
    behavior: ExceptionStackBehavior,
) -> ExnThrown {
    unsafe { wrappers2::JS_SetPendingException(scope.cx_mut(), v, behavior) };
    ExnThrown
}

/// Clear any pending exception on the context.
pub fn clear(scope: &Scope<'_>) {
    unsafe { wrappers2::JS_ClearPendingException(scope.cx()) }
}

/// Report the pending exception to stderr with a context label, then clear it.
///
/// For algorithms where a callback's throw is "reported" per spec but must not
/// abort the surrounding algorithm — event listener invocation, abort
/// algorithms, promise-settle bookkeeping. Swallowing such exceptions silently
/// makes real bugs in callback code undetectable, so they are printed in the
/// same shape as the event loop's uncaught-exception report. No-op when
/// nothing is pending.
pub fn report_and_clear(scope: &Scope<'_>, context: &str) {
    if !is_pending(scope) {
        return;
    }
    // `capture` clears the pending exception as part of extracting it.
    let captured = ExnThrown::capture(scope);
    eprintln!("[{context}] Uncaught exception: {captured}");
}

/// Get the `JSErrorReport` from an Error object.
///
/// Returns a reference to the error report if the object is an Error, or
/// `None` otherwise. The returned reference borrows the exception object's
/// internal data and is valid as long as the exception object remains rooted
/// (guaranteed by the [`HandleObject`](mozjs::gc::HandleObject) argument, whose
/// lifetime bounds the result).
pub fn error_from_exception<'a>(
    scope: &Scope<'_>,
    obj: mozjs::gc::HandleObject<'a>,
) -> Option<&'a mozjs::jsapi::JSErrorReport> {
    let ptr = unsafe { wrappers2::JS_ErrorFromException(scope.cx(), obj) };
    if ptr.is_null() {
        None
    } else {
        // SAFETY: SpiderMonkey guarantees the report pointer is valid for the
        // lifetime of the Error object, which is kept alive by the Handle.
        Some(unsafe { &*ptr })
    }
}

/// Report an uncatchable exception (e.g., OOM or stack overflow).
pub fn report_uncatchable(scope: &Scope<'_>) {
    unsafe { wrappers2::ReportUncatchableException(scope.cx()) }
}

/// Report an out-of-memory condition.
pub fn report_out_of_memory(scope: &Scope<'_>) {
    unsafe { wrappers2::JS_ReportOutOfMemory(scope.cx()) }
}
