// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Scoped exception handler (`TryCatch`).
//!
//! `TryCatch` provides structured exception handling similar to rusty_v8's
//! `v8::TryCatch`. It captures whether an exception was thrown during a block
//! of code and provides methods to inspect, rethrow, or clear the exception.
//!
//! # Example
//!
//! ```ignore
//! # use core_runtime::js::gc::scope::Scope;
//! # fn example(scope: &Scope<'_>) {
//! use core_runtime::js::try_catch::TryCatch;
//! use core_runtime::js::compile;
//!
//! let mut tc = TryCatch::new(scope);
//! let result = compile::evaluate(tc.scope(), "undeclared_variable");
//! if tc.has_caught() {
//!     let error = tc.capture();
//!     eprintln!("Error: {error}");
//! }
//! # }
//! ```

use crate::gc::scope::{InnerScope, Scope};
use mozjs::gc::HandleValue;
use mozjs::jsapi::ExceptionStackBehavior;
use mozjs::jsval::UndefinedValue;
use mozjs::rust::wrappers2;

use super::error::{CapturedError, ExnThrown};

/// A scoped exception handler.
///
/// When created, `TryCatch` records whether an exception is already pending.
/// Operations performed on the inner scope can be checked for exceptions via
/// [`has_caught`](TryCatch::has_caught). The exception can be inspected,
/// cleared, or rethrown.
///
/// `TryCatch` creates an inner scope, so values rooted through it are released
/// when the `TryCatch` is dropped.
pub struct TryCatch<'a> {
    inner: InnerScope<'a>,
    /// Whether an exception was already pending when TryCatch was created.
    had_exception: bool,
}

impl<'a> TryCatch<'a> {
    /// Create a new `TryCatch` scope.
    ///
    /// Records whether an exception is already pending. Operations on the
    /// returned scope's inner will be monitored for new exceptions.
    pub fn new(scope: &'a Scope<'_>) -> Self {
        // SAFETY: Scope guarantees a valid context pointer.
        let had_exception = unsafe { wrappers2::JS_IsExceptionPending(scope.cx()) };
        TryCatch {
            inner: scope.inner_scope(),
            had_exception,
        }
    }

    /// Get a reference to the inner scope for performing operations.
    pub fn scope(&self) -> &Scope<'_> {
        &self.inner
    }

    /// Returns `true` if an exception was caught (i.e., an exception is pending
    /// that was not pending when this `TryCatch` was created).
    pub fn has_caught(&self) -> bool {
        // SAFETY: Scope guarantees a valid context pointer.
        let pending = unsafe { wrappers2::JS_IsExceptionPending(self.inner.cx()) };
        pending && !self.had_exception
    }

    /// Get the pending exception value, if any.
    ///
    /// Returns `None` if no exception is pending. Does NOT clear the exception.
    pub fn exception(&self) -> Option<HandleValue<'_>> {
        // SAFETY: Scope guarantees a valid context with an entered realm.
        unsafe {
            if !wrappers2::JS_IsExceptionPending(self.inner.cx()) {
                return None;
            }
            let mut exc = self.inner.root_value_mut(UndefinedValue());
            if wrappers2::JS_GetPendingException(self.inner.cx_mut(), exc.reborrow()) {
                Some(exc.handle())
            } else {
                None
            }
        }
    }

    /// Capture the pending exception as a [`CapturedError`], clearing it.
    ///
    /// This extracts the error message, filename, line number, column, and
    /// stack trace from the pending exception. If no exception is pending,
    /// returns a default (empty) `CapturedError`.
    pub fn capture(&self) -> CapturedError {
        ExnThrown::capture(&self.inner)
    }

    /// Clear the pending exception without inspecting it.
    pub fn reset(&self) {
        // SAFETY: Scope guarantees a valid context pointer.
        unsafe { wrappers2::JS_ClearPendingException(self.inner.cx()) };
    }

    /// Re-set the given exception value as pending without capturing the stack.
    ///
    /// This is useful when you've inspected an exception and want to let it
    /// propagate. Pass the handle obtained from [`exception()`](Self::exception).
    pub fn rethrow(&self, exc: HandleValue<'_>) {
        // SAFETY: Scope guarantees a valid context with an entered realm.
        unsafe {
            wrappers2::JS_SetPendingException(
                self.inner.cx_mut(),
                exc,
                ExceptionStackBehavior::DoNotCapture,
            );
        };
    }
}
