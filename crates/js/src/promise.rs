// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Promise creation, resolution, rejection, and reaction management.
//!
//! The [`Promise`] marker type implements [`JSType`](crate::gc::handle::JSType),
//! enabling [`Promise<'s>`](crate::Promise) as the scope-rooted
//! handle type. It provides methods for state inspection, resolution/rejection,
//! and adding reactions.

use std::ptr::NonNull;

use crate::builtins::JSType;
use crate::gc::handle::Stack;
use crate::gc::scope::Scope;
use mozjs::jsapi::{JSObject, PromiseState};
use mozjs::rust::wrappers2;
use mozjs::rust::{HandleObject, HandleValue};

use super::error::ExnThrown;
use crate::Object;

/// Marker type for JavaScript `Promise` objects.
///
/// [`Promise<'s>`](crate::Promise) is the scope-rooted handle type:
///
/// ```ignore
/// let promise = Promise::new(&scope, executor.handle())?;
/// let state = promise.state();
/// ```
pub struct Promise;

impl JSType for Promise {
    const JS_NAME: &'static str = "Promise";

    fn js_class() -> *const mozjs::jsapi::JSClass {
        crate::class::proto_key_to_class(mozjs::jsapi::JSProtoKey::JSProto_Promise)
    }
}

impl<'s> Stack<'s, Promise> {
    /// Create a new `Promise` object from an executor function.
    ///
    /// The executor is called immediately with `(resolve, reject)` functions.
    pub fn new(scope: &'s Scope<'_>, executor: HandleObject) -> Result<Self, ExnThrown> {
        let obj = unsafe { wrappers2::NewPromiseObject(scope.cx_mut(), executor) };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(ExnThrown)
    }

    /// Create a new unresolved `Promise` without an executor.
    ///
    /// The returned promise starts in the "pending" state and must be
    /// resolved or rejected later via [`resolve`](Self::resolve) /
    /// [`reject`](Self::reject).
    pub fn new_pending(scope: &'s Scope<'_>) -> Result<Self, ExnThrown> {
        let obj = unsafe { wrappers2::NewPromiseObject(scope.cx_mut(), HandleObject::null()) };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(ExnThrown)
    }

    /// Create a new `Promise` that is immediately resolved with the given `value`.
    pub fn new_resolved_with_value(
        scope: &'s Scope<'_>,
        value: HandleValue,
    ) -> Result<Self, ExnThrown> {
        let promise = Self::new_pending(scope)?;
        promise.resolve(scope, value)?;
        Ok(promise)
    }

    /// Create a new `Promise` that is immediately rejected with the given `error`.
    pub fn new_rejected_with_error(
        scope: &'s Scope<'_>,
        error: HandleValue,
    ) -> Result<Self, ExnThrown> {
        let promise = Self::new_pending(scope)?;
        promise.reject(scope, error)?;
        Ok(promise)
    }

    /// Create a new `Promise` that is immediately rejected with the pending exception.
    pub fn new_rejected_with_pending_error(scope: &'s Scope<'_>) -> Result<Self, &'static str> {
        let pending_exception = crate::exception::get_pending(scope)?;
        crate::exception::clear(scope);
        let promise = Self::new_pending(scope).map_err(|_| "Failed to create promise")?;
        promise
            .reject(scope, pending_exception)
            .map_err(|_| "Failed to reject promise with pending exception")?;
        Ok(promise)
    }

    /// Check whether an object is a `Promise`.
    pub fn is_promise(obj: HandleObject) -> bool {
        // SAFETY: IsPromiseObject only inspects the object's class pointer.
        unsafe { wrappers2::IsPromiseObject(obj) }
    }

    /// Get the current state of this promise.
    pub fn state(&self) -> PromiseState {
        // SAFETY: self is a rooted handle to a valid Promise object.
        unsafe { wrappers2::GetPromiseState(self.handle()) }
    }

    /// Check whether this promise is already rejected.
    pub fn is_rejected(&self) -> bool {
        self.state() == PromiseState::Rejected
    }

    /// Get the result value of a settled promise.
    ///
    /// For a fulfilled promise this is the fulfillment value; for a rejected
    /// promise this is the rejection reason. On a pending promise this returns
    /// `undefined`.
    pub fn result<'a>(&self, scope: &'a Scope<'_>) -> HandleValue<'a> {
        let mut val = scope.root_value_mut(mozjs::jsval::UndefinedValue());
        // SAFETY: self is a rooted handle to a valid Promise object.
        unsafe { mozjs::glue::JS_GetPromiseResult(self.handle().into(), val.reborrow().into()) };
        val.handle()
    }

    /// Get the unique ID of this promise (for debugging/tracking).
    pub fn id(&self) -> u64 {
        // SAFETY: self is a rooted handle to a valid Promise object.
        unsafe { wrappers2::GetPromiseID(self.handle()) }
    }

    /// Check whether this promise has been handled (i.e., has a rejection handler).
    pub fn is_handled(&self) -> bool {
        // SAFETY: self is a rooted handle to a valid Promise object.
        unsafe { wrappers2::GetPromiseIsHandled(self.handle()) }
    }

    /// Mark a settled promise as handled, suppressing unhandled rejection warnings.
    pub fn set_settled_is_handled(&self, scope: &Scope<'_>) -> Result<(), ExnThrown> {
        let ok = unsafe { wrappers2::SetSettledPromiseIsHandled(scope.cx_mut(), self.handle()) };
        ExnThrown::check(ok)
    }

    /// Mark any promise (including pending) as handled.
    pub fn set_any_is_handled(&self, scope: &Scope<'_>) -> Result<(), ExnThrown> {
        let ok = unsafe { wrappers2::SetAnyPromiseIsHandled(scope.cx_mut(), self.handle()) };
        ExnThrown::check(ok)
    }

    /// Get the allocation site of this promise (a `SavedFrame`, if available).
    pub fn allocation_site(&self) -> Option<NonNull<JSObject>> {
        NonNull::new(unsafe { wrappers2::GetPromiseAllocationSite(self.handle()) })
    }

    /// Get the resolution site of this promise (a `SavedFrame`, if available).
    pub fn resolution_site(&self) -> Option<NonNull<JSObject>> {
        NonNull::new(unsafe { wrappers2::GetPromiseResolutionSite(self.handle()) })
    }

    /// Resolve this promise with the given value.
    pub fn resolve(&self, scope: &Scope<'_>, value: HandleValue) -> Result<(), ExnThrown> {
        let ok = unsafe { wrappers2::ResolvePromise(scope.cx_mut(), self.handle(), value) };
        ExnThrown::check(ok)
    }

    /// Reject this promise with the given value.
    pub fn reject(&self, scope: &Scope<'_>, value: HandleValue) -> Result<(), ExnThrown> {
        let ok = unsafe { wrappers2::RejectPromise(scope.cx_mut(), self.handle(), value) };
        ExnThrown::check(ok)
    }

    /// Add `then` reactions (fulfillment and rejection handlers) to this promise.
    pub fn add_reactions(
        &self,
        scope: &Scope<'_>,
        on_fulfilled: HandleObject,
        on_rejected: HandleObject,
    ) -> Result<(), ExnThrown> {
        let ok = unsafe {
            wrappers2::AddPromiseReactions(scope.cx_mut(), self.handle(), on_fulfilled, on_rejected)
        };
        ExnThrown::check(ok)
    }

    /// Add `then` reactions ignoring unhandled rejection tracking.
    pub fn add_reactions_ignoring_unhandled_rejection(
        &self,
        scope: &Scope<'_>,
        on_fulfilled: HandleObject,
        on_rejected: HandleObject,
    ) -> Result<(), ExnThrown> {
        let ok = unsafe {
            wrappers2::AddPromiseReactionsIgnoringUnhandledRejection(
                scope.cx_mut(),
                self.handle(),
                on_fulfilled,
                on_rejected,
            )
        };
        ExnThrown::check(ok)
    }

    /// Call `Promise.resolve(value)` using the original `Promise` constructor.
    pub fn call_original_resolve(
        scope: &'s Scope<'_>,
        resolution_value: HandleValue,
    ) -> Result<Self, ExnThrown> {
        let obj =
            unsafe { wrappers2::CallOriginalPromiseResolve(scope.cx_mut(), resolution_value) };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(ExnThrown)
    }

    /// Call `Promise.reject(value)` using the original `Promise` constructor.
    pub fn call_original_reject(
        scope: &'s Scope<'_>,
        rejection_value: HandleValue,
    ) -> Result<Self, ExnThrown> {
        let obj = unsafe { wrappers2::CallOriginalPromiseReject(scope.cx_mut(), rejection_value) };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(ExnThrown)
    }

    /// Call the original `Promise.prototype.then` with the given handlers.
    ///
    /// Returns a new promise for the result.
    pub fn call_original_then(
        &self,
        scope: &'s Scope<'_>,
        on_fulfilled: HandleObject,
        on_rejected: HandleObject,
    ) -> Result<Self, ExnThrown> {
        let obj = unsafe {
            wrappers2::CallOriginalPromiseThen(
                scope.cx_mut(),
                self.handle(),
                on_fulfilled,
                on_rejected,
            )
        };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(ExnThrown)
    }

    /// Create a `Promise.all`-style promise from a vector of promises.
    ///
    /// # Safety
    ///
    /// `promises` must be a handle to a valid `ObjectVector`.
    pub unsafe fn wait_for_all(
        scope: &'s Scope<'_>,
        promises: mozjs::jsapi::HandleObjectVector,
    ) -> Result<Self, ExnThrown> {
        let obj = wrappers2::GetWaitForAllPromise(scope.cx_mut(), promises);
        NonNull::new(obj)
            .map(|nn| Self::from_handle_unchecked(scope.root_object(nn)))
            .ok_or(ExnThrown)
    }

    /// Get the `Promise` constructor for the current realm.
    pub fn constructor(
        scope: &'s Scope<'_>,
    ) -> Result<mozjs::gc::Handle<'s, *mut JSObject>, ExnThrown> {
        let obj = unsafe { wrappers2::GetPromiseConstructor(scope.cx_mut()) };
        NonNull::new(obj)
            .map(|p| scope.root_object(p))
            .ok_or(ExnThrown)
    }

    /// Get the `Promise.prototype` for the current realm.
    pub fn prototype(
        scope: &'s Scope<'_>,
    ) -> Result<mozjs::gc::Handle<'s, *mut JSObject>, ExnThrown> {
        let obj = unsafe { wrappers2::GetPromisePrototype(scope.cx_mut()) };
        NonNull::new(obj)
            .map(|p| scope.root_object(p))
            .ok_or(ExnThrown)
    }
}

impl<'s> std::ops::Deref for Stack<'s, Promise> {
    type Target = Object<'s>;

    fn deref(&self) -> &Object<'s> {
        // SAFETY: Stack<Promise> and Stack<Object> are both repr(transparent)
        // over Handle<'s, *mut JSObject>.
        unsafe { &*(self as *const Stack<'s, Promise> as *const Object<'s>) }
    }
}

// ---------------------------------------------------------------------------
// Async promise support — JSPromise, PromiseOutcome, __spawn_promise
// ---------------------------------------------------------------------------

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;

use crate::heap::{Heap as MozHeap, RootedTraceableBox};
use crate::native::{MutableHandleValue, RawJSContext};
use crate::value;
use mozjs::conversions::ToJSValConvertible;

/// Callback that sets a resolved value on a `MutableHandleValue`.
type ResolveCallback = Box<dyn FnOnce(*mut RawJSContext, MutableHandleValue) -> bool>;

/// A pending promise paired with its future.
pub(crate) type PendingPromise = (
    RootedTraceableBox<MozHeap<*mut JSObject>>,
    Pin<Box<dyn Future<Output = PromiseOutcome> + 'static>>,
);

/// The outcome of an async method — either resolve with a convertible value
/// or reject with an error message.
pub enum PromiseOutcome {
    /// Resolve the promise. The boxed closure sets the return value on the
    /// provided `MutableHandleValue` and returns `true` on success.
    Resolve(ResolveCallback),
    /// Reject the promise with the given error message.
    Reject(String),
}

/// A future that resolves or rejects a JS Promise.
///
/// Use `JSPromise::new` in a `#[method]` to return a promise from an async
/// operation. The macro detects the `JSPromise` return type and generates
/// code to create a bare SpiderMonkey Promise, spawn the future, and
/// resolve/reject the promise when the future completes.
///
/// The design is async-runtime agnostic: call `drain_promises` with your
/// executor in your event loop to resolve/reject completed promises.
///
/// # Example
///
/// ```rust,ignore
/// #[method]
/// fn slow_greet(&self, name: String) -> JSPromise {
///     let greeting = self.prefix.clone();
///     JSPromise::new(async move {
///         // simulate async work
///         Ok(format!("{}, {}!", greeting, name))
///     })
/// }
/// ```
pub struct JSPromise {
    pub(crate) future: Pin<Box<dyn Future<Output = PromiseOutcome> + 'static>>,
}

impl JSPromise {
    /// Create a `JSPromise` from a future that returns `Result<T, E>`.
    ///
    /// - `Ok(value)` resolves the promise; `value` must implement `ToJSValConvertible`.
    /// - `Err(e)` rejects the promise with `e.to_string()` as the error message.
    pub fn new<T, E, F>(future: F) -> Self
    where
        T: ToJSValConvertible + 'static,
        E: std::fmt::Display + 'static,
        F: Future<Output = Result<T, E>> + 'static,
    {
        JSPromise {
            future: Box::pin(async move {
                match future.await {
                    Ok(value) => PromiseOutcome::Resolve(Box::new(
                        move |cx: *mut RawJSContext, rval: MutableHandleValue| unsafe {
                            value.to_jsval(cx, rval);
                            true
                        },
                    )),
                    Err(e) => PromiseOutcome::Reject(e.to_string()),
                }
            }),
        }
    }

    /// Create a `JSPromise` from a future that resolves to `()` (void).
    pub fn new_void<E, F>(future: F) -> Self
    where
        E: std::fmt::Display + 'static,
        F: Future<Output = Result<(), E>> + 'static,
    {
        JSPromise {
            future: Box::pin(async move {
                match future.await {
                    Ok(()) => PromiseOutcome::Resolve(Box::new(
                        move |_cx: *mut RawJSContext, mut rval: MutableHandleValue| {
                            rval.set(value::undefined());
                            true
                        },
                    )),
                    Err(e) => PromiseOutcome::Reject(e.to_string()),
                }
            }),
        }
    }
}

thread_local! {
    // Crown: `PendingPromise` is self-rooting via `RootedTraceableBox`, so we
    // don't need to root the Vec itself.
    #[crate::allow_unrooted_interior]
    static PENDING_FUTURES: RefCell<Vec<PendingPromise>> = RefCell::new(Vec::new());
}

/// Queue a future that will resolve or reject a JS Promise.
///
/// This is called by generated JSNative wrappers. It stores the promise
/// object in a `RootedTraceableBox<MozHeap<*mut JSObject>>` for GC safety
/// and queues the future for later execution via `drain_promises`.
///
/// # Safety
///
/// - `promise_obj` must be a valid JS Promise object.
#[doc(hidden)]
// Crown: The provided `promise_obj` is rooted immediately.
#[crate::allow_unrooted_interior]
pub unsafe fn __spawn_promise(promise_obj: *mut JSObject, js_promise: JSPromise) {
    let boxed_heap = RootedTraceableBox::new(MozHeap::default());
    boxed_heap.set(promise_obj);

    PENDING_FUTURES.with(|f| {
        f.borrow_mut().push((boxed_heap, js_promise.future));
    });
}

/// Take all pending promise futures, returning them for execution.
///
/// This drains the internal queue. Callers (typically the event loop) are
/// responsible for running each future and resolving/rejecting its paired
/// JS promise.
pub fn take_pending_futures() -> Vec<PendingPromise> {
    PENDING_FUTURES.with(|f| std::mem::take(&mut *f.borrow_mut()))
}
