// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Promise creation, resolution, rejection, and reaction management.
//!
//! The [`Promise`] newtype wraps a scope-rooted `Handle<'s, *mut JSObject>`
//! known to be a Promise object. It provides methods for state inspection,
//! resolution/rejection, and adding reactions.

use std::ptr::NonNull;

use crate::gc::scope::Scope;
use mozjs::gc::Handle;
use mozjs::jsapi::{JSObject, PromiseState};
use mozjs::rust::wrappers2;
use mozjs::rust::{HandleObject, HandleValue};

use super::builtins::{Is, To};
use super::error::JSError;
use super::object::Object;

/// A JavaScript `Promise` object, rooted in a scope's pool.
///
/// `Promise<'s>` wraps a `Handle<'s, *mut JSObject>` known to be a Promise.
/// Construction automatically roots in the scope:
///
/// ```ignore
/// let promise = Promise::new(&scope, executor.handle())?;
/// let state = promise.state();
/// ```
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct Promise<'s>(pub(crate) Handle<'s, *mut JSObject>);

impl<'s> Promise<'s> {
    /// Create a new `Promise` object from an executor function.
    ///
    /// The executor is called immediately with `(resolve, reject)` functions.
    pub fn new(scope: &'s Scope<'_>, executor: HandleObject) -> Result<Self, JSError> {
        let obj = unsafe { wrappers2::NewPromiseObject(scope.cx_mut(), executor) };
        NonNull::new(obj)
            .map(|nn| Promise(scope.root_object(nn)))
            .ok_or(JSError)
    }

    /// Create a new unresolved `Promise` without an executor.
    ///
    /// The returned promise starts in the "pending" state and must be
    /// resolved or rejected later via [`resolve`](Self::resolve) /
    /// [`reject`](Self::reject).
    pub fn new_pending(scope: &'s Scope<'_>) -> Result<Self, JSError> {
        let obj = unsafe { wrappers2::NewPromiseObject(scope.cx_mut(), HandleObject::null()) };
        NonNull::new(obj)
            .map(|nn| Promise(scope.root_object(nn)))
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

    /// Wrap an existing rooted handle in a `Promise`.
    pub fn from_handle(handle: Handle<'s, *mut JSObject>) -> Self {
        Promise(handle)
    }

    /// Check whether an object is a `Promise`.
    pub fn is_promise(obj: HandleObject) -> bool {
        // SAFETY: IsPromiseObject only inspects the object's class pointer.
        unsafe { wrappers2::IsPromiseObject(obj) }
    }

    /// Get the current state of this promise.
    pub fn state(&self) -> PromiseState {
        // SAFETY: self.0 is a rooted handle to a valid Promise object.
        unsafe { wrappers2::GetPromiseState(self.0) }
    }

    /// Get the unique ID of this promise (for debugging/tracking).
    pub fn id(&self) -> u64 {
        // SAFETY: self.0 is a rooted handle to a valid Promise object.
        unsafe { wrappers2::GetPromiseID(self.0) }
    }

    /// Check whether this promise has been handled (i.e., has a rejection handler).
    pub fn is_handled(&self) -> bool {
        // SAFETY: self.0 is a rooted handle to a valid Promise object.
        unsafe { wrappers2::GetPromiseIsHandled(self.0) }
    }

    /// Mark a settled promise as handled, suppressing unhandled rejection warnings.
    pub fn set_settled_is_handled(&self, scope: &Scope<'_>) -> Result<(), JSError> {
        let ok = unsafe { wrappers2::SetSettledPromiseIsHandled(scope.cx_mut(), self.0) };
        JSError::check(ok)
    }

    /// Mark any promise (including pending) as handled.
    pub fn set_any_is_handled(&self, scope: &Scope<'_>) -> Result<(), JSError> {
        let ok = unsafe { wrappers2::SetAnyPromiseIsHandled(scope.cx_mut(), self.0) };
        JSError::check(ok)
    }

    /// Get the allocation site of this promise (a `SavedFrame`, if available).
    pub fn allocation_site(&self) -> Option<NonNull<JSObject>> {
        NonNull::new(unsafe { wrappers2::GetPromiseAllocationSite(self.0) })
    }

    /// Get the resolution site of this promise (a `SavedFrame`, if available).
    pub fn resolution_site(&self) -> Option<NonNull<JSObject>> {
        NonNull::new(unsafe { wrappers2::GetPromiseResolutionSite(self.0) })
    }

    /// Resolve this promise with the given value.
    pub fn resolve(&self, scope: &Scope<'_>, value: HandleValue) -> Result<(), JSError> {
        let ok = unsafe { wrappers2::ResolvePromise(scope.cx_mut(), self.0, value) };
        JSError::check(ok)
    }

    /// Reject this promise with the given value.
    pub fn reject(&self, scope: &Scope<'_>, value: HandleValue) -> Result<(), JSError> {
        let ok = unsafe { wrappers2::RejectPromise(scope.cx_mut(), self.0, value) };
        JSError::check(ok)
    }

    /// Add `then` reactions (fulfillment and rejection handlers) to this promise.
    pub fn add_reactions(
        &self,
        scope: &Scope<'_>,
        on_fulfilled: HandleObject,
        on_rejected: HandleObject,
    ) -> Result<(), JSError> {
        let ok = unsafe {
            wrappers2::AddPromiseReactions(scope.cx_mut(), self.0, on_fulfilled, on_rejected)
        };
        JSError::check(ok)
    }

    /// Add `then` reactions ignoring unhandled rejection tracking.
    pub fn add_reactions_ignoring_unhandled_rejection(
        &self,
        scope: &Scope<'_>,
        on_fulfilled: HandleObject,
        on_rejected: HandleObject,
    ) -> Result<(), JSError> {
        let ok = unsafe {
            wrappers2::AddPromiseReactionsIgnoringUnhandledRejection(
                scope.cx_mut(),
                self.0,
                on_fulfilled,
                on_rejected,
            )
        };
        JSError::check(ok)
    }

    /// Call `Promise.resolve(value)` using the original `Promise` constructor.
    pub fn call_original_resolve(
        scope: &'s Scope<'_>,
        resolution_value: HandleValue,
    ) -> Result<Self, JSError> {
        let obj =
            unsafe { wrappers2::CallOriginalPromiseResolve(scope.cx_mut(), resolution_value) };
        NonNull::new(obj)
            .map(|nn| Promise(scope.root_object(nn)))
            .ok_or(JSError)
    }

    /// Call `Promise.reject(value)` using the original `Promise` constructor.
    pub fn call_original_reject(
        scope: &'s Scope<'_>,
        rejection_value: HandleValue,
    ) -> Result<Self, JSError> {
        let obj = unsafe { wrappers2::CallOriginalPromiseReject(scope.cx_mut(), rejection_value) };
        NonNull::new(obj)
            .map(|nn| Promise(scope.root_object(nn)))
            .ok_or(JSError)
    }

    /// Call the original `Promise.prototype.then` with the given handlers.
    ///
    /// Returns a new promise for the result.
    pub fn call_original_then(
        &self,
        scope: &'s Scope<'_>,
        on_fulfilled: HandleObject,
        on_rejected: HandleObject,
    ) -> Result<Self, JSError> {
        let obj = unsafe {
            wrappers2::CallOriginalPromiseThen(scope.cx_mut(), self.0, on_fulfilled, on_rejected)
        };
        NonNull::new(obj)
            .map(|nn| Promise(scope.root_object(nn)))
            .ok_or(JSError)
    }

    /// Create a `Promise.all`-style promise from a vector of promises.
    ///
    /// # Safety
    ///
    /// `promises` must be a handle to a valid `ObjectVector`.
    pub unsafe fn wait_for_all(
        scope: &'s Scope<'_>,
        promises: mozjs::jsapi::HandleObjectVector,
    ) -> Result<Self, JSError> {
        let obj = wrappers2::GetWaitForAllPromise(scope.cx_mut(), promises);
        NonNull::new(obj)
            .map(|nn| Promise(scope.root_object(nn)))
            .ok_or(JSError)
    }

    /// Get the `Promise` constructor for the current realm.
    pub fn constructor(scope: &'s Scope<'_>) -> Result<Handle<'s, *mut JSObject>, JSError> {
        let obj = unsafe { wrappers2::GetPromiseConstructor(scope.cx_mut()) };
        NonNull::new(obj)
            .map(|p| scope.root_object(p))
            .ok_or(JSError)
    }

    /// Get the `Promise.prototype` for the current realm.
    pub fn prototype(scope: &'s Scope<'_>) -> Result<Handle<'s, *mut JSObject>, JSError> {
        let obj = unsafe { wrappers2::GetPromisePrototype(scope.cx_mut()) };
        NonNull::new(obj)
            .map(|p| scope.root_object(p))
            .ok_or(JSError)
    }
}

impl Is for Promise<'_> {
    fn is(_scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        Ok(Promise::is_promise(obj))
    }
}

impl<'s> To<Promise<'s>> for Object<'s> {
    fn to(&self, scope: &Scope<'_>) -> Result<Promise<'s>, JSError> {
        if Promise::is(scope, self.0)? {
            Ok(Promise(self.0))
        } else {
            Err(JSError)
        }
    }
}

impl<'s> std::ops::Deref for Promise<'s> {
    type Target = Object<'s>;

    fn deref(&self) -> &Object<'s> {
        // SAFETY: Promise and Object are both repr(transparent) over
        // Handle<'s, *mut JSObject>.
        unsafe { &*(self as *const Promise<'s> as *const Object<'s>) }
    }
}
