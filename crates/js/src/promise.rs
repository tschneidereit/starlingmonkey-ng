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
use crate::prelude::ToJSVal;
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
    type Rooted<'s> = Stack<'s, Self>;
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
        unsafe { Self::from_mozjs_rval(scope, obj) }
    }

    /// Create a new unresolved `Promise` without an executor.
    ///
    /// The returned promise starts in the "pending" state and must be
    /// resolved or rejected later via [`resolve`](Self::resolve) /
    /// [`reject`](Self::reject).
    pub fn new_pending(scope: &'s Scope<'_>) -> Result<Self, ExnThrown> {
        let obj = unsafe { wrappers2::NewPromiseObject(scope.cx_mut(), HandleObject::null()) };
        // SAFETY: NewPromiseObject returns a Promise object or null.
        unsafe { Self::from_mozjs_rval(scope, obj) }
    }

    /// Create a new `Promise` resolved with the given `value`.
    ///
    /// This always creates a new `Promise` and resolves it with `value`.
    /// Use [`call_original_resolve`](Self::call_original_resolve) to get the
    /// behavior of `Promise.resolve(value)`, where if `value` already is a
    /// `Promise`, it's returned unchanged.
    pub fn new_resolved_with_value(
        scope: &'s Scope<'_>,
        value: impl ToJSVal<'s>,
    ) -> Result<Self, ExnThrown> {
        let value = value.to_jsval_throwing(scope)?;
        let promise = Self::new_pending(scope)?;
        promise.resolve(scope, value)?;
        Ok(promise)
    }

    /// Create a new `Promise` that is immediately rejected with the given `error`.
    pub fn new_rejected_with_error(
        scope: &'s Scope<'_>,
        error: impl ToJSVal<'s>,
    ) -> Result<Self, ExnThrown> {
        let error = error.to_jsval_throwing(scope)?;
        let obj = unsafe { wrappers2::CallOriginalPromiseReject(scope.cx_mut(), error) };
        // SAFETY: CallOriginalPromiseReject returns a Promise object or null.
        unsafe { Self::from_mozjs_rval(scope, obj) }
    }

    /// Create a new `Promise` that is immediately rejected with the pending exception.
    pub fn new_rejected_with_pending_error(scope: &'s Scope<'_>) -> Result<Self, &'static str> {
        let error = crate::exception::take_pending(scope)?;
        Self::new_rejected_with_error(scope, error)
            .map_err(|_| "Failed to reject promise with pending exception")
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

    /// Check whether this promise is still pending (not settled).
    pub fn is_pending(&self) -> bool {
        self.state() == PromiseState::Pending
    }

    /// Get the result value of a settled promise.
    ///
    /// For a fulfilled promise this is the fulfillment value; for a rejected
    /// promise this is the rejection reason. On a pending promise this returns
    /// `None`.
    pub fn result<'a>(&self, scope: &'a Scope<'_>) -> Option<HandleValue<'a>> {
        if self.is_pending() {
            return None;
        }
        let mut val = scope.root_value_mut(mozjs::jsval::UndefinedValue());
        // SAFETY: self is a rooted handle to a valid, settled Promise object.
        unsafe { mozjs::glue::JS_GetPromiseResult(self.handle().into(), val.reborrow().into()) };
        Some(val.handle())
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
    pub fn resolve<'a>(
        &self,
        scope: &'a Scope<'_>,
        resolution: impl ToJSVal<'a>,
    ) -> Result<(), ExnThrown> {
        let value = resolution.to_jsval_throwing(scope)?;
        let ok = unsafe { wrappers2::ResolvePromise(scope.cx_mut(), self.handle(), value) };
        ExnThrown::check(ok)
    }

    /// Reject this promise with the given value.
    pub fn reject<'a>(
        &self,
        scope: &'a Scope<'_>,
        error: impl ToJSVal<'a>,
    ) -> Result<(), ExnThrown> {
        let value = error.to_jsval_throwing(scope)?;
        let ok = unsafe { wrappers2::RejectPromise(scope.cx_mut(), self.handle(), value) };
        ExnThrown::check(ok)
    }

    /// Add `then` reactions (fulfillment and rejection handlers) to this promise.
    pub fn add_reactions(
        &self,
        scope: &Scope<'_>,
        on_fulfilled: Option<Object<'_>>,
        on_rejected: Option<Object<'_>>,
    ) -> Result<(), ExnThrown> {
        let ok = unsafe {
            wrappers2::AddPromiseReactions(
                scope.cx_mut(),
                self.handle(),
                on_fulfilled.map_or(HandleObject::null(), |o| o.handle()),
                on_rejected.map_or(HandleObject::null(), |o| o.handle()),
            )
        };
        ExnThrown::check(ok)
    }

    /// Add `then` reactions ignoring unhandled rejection tracking.
    pub fn add_reactions_ignoring_unhandled_rejection(
        &self,
        scope: &Scope<'_>,
        on_fulfilled: Option<Object<'_>>,
        on_rejected: Option<Object<'_>>,
    ) -> Result<(), ExnThrown> {
        let ok = unsafe {
            wrappers2::AddPromiseReactionsIgnoringUnhandledRejection(
                scope.cx_mut(),
                self.handle(),
                on_fulfilled.map_or(HandleObject::null(), |o| o.handle()),
                on_rejected.map_or(HandleObject::null(), |o| o.handle()),
            )
        };
        ExnThrown::check(ok)
    }

    /// Call `Promise.resolve(value)` using the original `Promise` constructor.
    ///
    /// If `resolution_value` is already a `Promise`, this returns it unchanged.
    /// Use [`new_resolved_with_value`](Self::new_resolved_with_value) if you
    /// need to ensure a fresh promise.
    pub fn call_original_resolve(
        scope: &'s Scope<'_>,
        resolution_value: impl ToJSVal<'s>,
    ) -> Result<Self, ExnThrown> {
        let resolution_value = resolution_value.to_jsval_throwing(scope)?;
        let obj =
            unsafe { wrappers2::CallOriginalPromiseResolve(scope.cx_mut(), resolution_value) };
        unsafe { Self::from_mozjs_rval(scope, obj) }
    }

    /// Call `Promise.reject(value)` using the original `Promise` constructor.
    pub fn call_original_reject(
        scope: &'s Scope<'_>,
        rejection_value: impl ToJSVal<'s>,
    ) -> Result<Self, ExnThrown> {
        let rejection_value = rejection_value.to_jsval_throwing(scope)?;
        let obj = unsafe { wrappers2::CallOriginalPromiseReject(scope.cx_mut(), rejection_value) };
        unsafe { Self::from_mozjs_rval(scope, obj) }
    }

    /// Call the original `Promise.prototype.then` with the given handlers.
    ///
    /// Returns a new promise for the result.
    pub fn call_original_then(
        &self,
        scope: &'s Scope<'_>,
        on_fulfilled: Option<Object<'_>>,
        on_rejected: Option<Object<'_>>,
    ) -> Result<Self, ExnThrown> {
        let obj = unsafe {
            wrappers2::CallOriginalPromiseThen(
                scope.cx_mut(),
                self.handle(),
                on_fulfilled.map_or(HandleObject::null(), |o| o.handle()),
                on_rejected.map_or(HandleObject::null(), |o| o.handle()),
            )
        };
        unsafe { Self::from_mozjs_rval(scope, obj) }
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

    /// Create a `Promise.all`-style promise that settles once every promise in
    /// `promises` settles: it fulfills with their values, or rejects as soon as
    /// one of them rejects. This is the spec's "getting a promise to wait for
    /// all" operation.
    pub fn wait_for_all_from(
        scope: &'s Scope<'_>,
        promises: &[HandleObject],
    ) -> Result<Self, ExnThrown> {
        let cx = unsafe { scope.raw_cx_no_gc() };
        let vector = mozjs::rust::RootedObjectVectorWrapper::new(cx);
        for promise in promises {
            if !vector.append(promise.get()) {
                return Err(ExnThrown);
            }
        }
        // SAFETY: `vector` is a live, valid ObjectVector for the duration of the call;
        // `GetWaitForAllPromise` reads it synchronously.
        unsafe { Self::wait_for_all(scope, vector.handle()) }
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

crate::gc::handle::deref_to_object!(Promise);

// ---------------------------------------------------------------------------
// Async promise support — JSPromise, PromiseOutcome, __spawn_promise
// ---------------------------------------------------------------------------

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;

use crate::heap::{MozHeap, RootedTraceableBox};
use crate::native::{MutableHandleValue, RawJSContext};
use crate::value;
use mozjs::conversions::ToJSValConvertible;

/// Callback that sets a resolved value on a `MutableHandleValue`.
type ResolveCallback = Box<dyn FnOnce(*mut RawJSContext, MutableHandleValue) -> bool>;

/// A pending promise paired with its future, tagged with the id of the event loop that owns it (the
/// loop active when it was spawned), so concurrent per-request loops drive and settle only their own
/// futures. Id 0 means unowned (spawned with no active loop), can be driven by any loop.
pub(crate) type PendingPromise = (
    u64,
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

    /// Create a `JSPromise` from a future that yields a [`PromiseOutcome`]
    /// directly.
    ///
    /// Use this when settling needs scope access — e.g. `fetch` builds the JS
    /// `Response` object inside the resolve callback (which receives the
    /// `JSContext`), rather than producing a `ToJSValConvertible` value up front.
    pub fn from_outcome<F>(future: F) -> Self
    where
        F: Future<Output = PromiseOutcome> + 'static,
    {
        JSPromise {
            future: Box::pin(future),
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

thread_local! {
    /// The event-loop id that owns futures spawned now (set by the event loop in `with_event_loop`).
    /// 0 when no loop is active.
    static CURRENT_FUTURE_OWNER: Cell<u64> = const { Cell::new(0) };
}

/// Set the owning event-loop id for futures spawned from now on, returning the previous owner so the
/// caller can restore it when the loop's scope ends. The event loop calls this in `with_event_loop`.
pub fn set_current_future_owner(owner: u64) -> u64 {
    CURRENT_FUTURE_OWNER.with(|owner_cell| owner_cell.replace(owner))
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

    let owner = CURRENT_FUTURE_OWNER.with(|owner_cell| owner_cell.get());
    PENDING_FUTURES.with(|f| {
        f.borrow_mut().push((owner, boxed_heap, js_promise.future));
    });
}

/// Take all pending promise futures, returning them for execution.
///
/// This drains the internal queue into the active set managed by
/// [`drive_pending_futures`]; it is not normally called directly.
fn take_pending_futures() -> Vec<PendingPromise> {
    PENDING_FUTURES.with(|f| std::mem::take(&mut *f.borrow_mut()))
}

thread_local! {
    // The futures currently being polled by the event loop. Like `PENDING_FUTURES`,
    // each entry self-roots its promise via `RootedTraceableBox`.
    static ACTIVE_FUTURES: RefCell<Vec<PendingPromise>> = const { RefCell::new(Vec::new()) };
}

/// A GC-rooted object handle that may be held across `await` points inside a
/// `'static` future. It self-registers with the tracer (via `RootedTraceableBox`),
/// so the referenced object stays live and is relocated by the GC while async
/// work runs — the safe way to keep a JS object a spawned future will use on
/// completion (e.g. a `fetch` body stream the future fills once the host read
/// finishes).
pub struct RootedObject(RootedTraceableBox<MozHeap<*mut JSObject>>);

impl RootedObject {
    /// Root the object behind an already-rooted handle for this handle's
    /// lifetime.
    ///
    /// Taking a handle (rather than a raw pointer) guarantees the pointer
    /// registered with the tracer refers to a live object — tracing an
    /// arbitrary pointer would be undefined behavior.
    pub fn new(object: mozjs::gc::Handle<'_, *mut JSObject>) -> Self {
        let boxed = RootedTraceableBox::new(MozHeap::default());
        boxed.set(object.get());
        RootedObject(boxed)
    }

    /// The rooted object pointer.
    pub fn get(&self) -> *mut JSObject {
        self.0.get()
    }

    /// Root the referenced object in `scope`.
    pub fn object<'s>(&self, scope: &'s Scope<'_>) -> crate::Object<'s> {
        // SAFETY: the stored pointer came from a rooted handle (see `new`)
        // and is traced by the RootedTraceableBox, so it is live; it is
        // non-null by construction.
        unsafe { crate::Object::from_raw(scope, self.0.get()) }.expect("RootedObject is never null")
    }
}

/// Cancel and drop the pending future settling `promise`, if one is queued.
///
/// Dropping the future cancels its in-flight work (e.g. an outstanding HTTP
/// request) and lets the event loop exit once nothing else is pending. Used by
/// `fetch` abort: after rejecting the fetch promise there is no point keeping the
/// request alive. Returns whether a future was found and dropped.
///
/// Takes a rooted [`HandleObject`] rather than a bare `*mut JSObject`: the
/// identity match below compares the GC-current pointer, so the caller must hold
/// the promise live. Reading the pointer from the handle here, with no
/// intervening allocation, keeps the comparison sound under a compacting GC.
pub fn cancel_pending_future(promise: HandleObject) -> bool {
    let promise_obj = promise.get();
    let mut removed = false;
    let mut drop_matching = |queue: &RefCell<Vec<PendingPromise>>| {
        queue.borrow_mut().retain(|(_owner, boxed, _)| {
            let matches = boxed.get() == promise_obj;
            removed |= matches;
            !matches
        });
    };
    PENDING_FUTURES.with(&mut drop_matching);
    ACTIVE_FUTURES.with(&mut drop_matching);
    removed
}

/// Drop every pending async-promise future owned by `owner` (or unowned, id 0).
///
/// Called when an event loop terminates with futures still in flight (e.g. WPT mode stops the loop
/// once a test's completion callback fires, even though a cancelled/disturbed body read is still
/// pending). Dropping each future cancels its in-flight host I/O and unregisters its
/// `RootedTraceableBox` (and any `RootedObject` it captured) from the engine's extra-roots tracer
/// while the `JSContext` is still alive — otherwise engine teardown's `finishRoots` would trace a
/// now-freed box and crash.
pub fn cancel_pending_futures_for(owner: u64) {
    let drop_owned = |queue: &RefCell<Vec<PendingPromise>>| {
        queue
            .borrow_mut()
            .retain(|(future_owner, _, _)| *future_owner != owner && *future_owner != 0);
    };
    PENDING_FUTURES.with(drop_owned);
    ACTIVE_FUTURES.with(drop_owned);
}

/// Whether any async-promise future owned by `owner` (or unowned) is pending (spawned but not yet
/// settled).
///
/// An event loop uses this to stay alive while *its own* async I/O (e.g. a `fetch`) is in flight,
/// even with no tasks or timers — without being held alive by another request's loop's futures.
pub fn has_pending_futures(owner: u64) -> bool {
    let owned_by_caller = |future: &PendingPromise| future.0 == owner || future.0 == 0;
    ACTIVE_FUTURES.with(|a| a.borrow().iter().any(owned_by_caller))
        || PENDING_FUTURES.with(|f| f.borrow().iter().any(owned_by_caller))
}

/// A promise object whose future completed, paired with its outcome — returned by
/// [`poll_pending_futures`] for [`settle_completed_futures`] to settle.
pub type CompletedFuture = (RootedTraceableBox<MozHeap<*mut JSObject>>, PromiseOutcome);

/// Poll the async-promise futures owned by `owner` (or unowned, id 0) one turn: adopt any newly
/// spawned futures, poll the matching ones with `task_cx`, and return those that completed. Futures
/// owned by another loop are left untouched for that loop to drive.
///
/// Polling does not run JS, so it needs no active loop. The caller settles the returned completions
/// with [`settle_completed_futures`] **with the owning loop active**, so a reaction (a timer, or
/// releasing the loop's interest) reaches the right loop.
///
/// The event loop calls this from inside its asynchronous wait, so the futures are polled with a
/// real waker (their I/O readiness wakes the loop). `JSPromise`-returning builtins (e.g. `fetch`)
/// rely on this being driven.
pub fn poll_pending_futures(
    owner: u64,
    task_cx: &mut std::task::Context<'_>,
) -> Vec<CompletedFuture> {
    // Take the active set out of the thread-local (so a future's poll can't re-borrow it) and adopt
    // any newly spawned futures.
    let mut futures = ACTIVE_FUTURES.with(|a| std::mem::take(&mut *a.borrow_mut()));
    futures.append(&mut take_pending_futures());

    let mut still_pending: Vec<PendingPromise> = Vec::with_capacity(futures.len());
    let mut completed: Vec<CompletedFuture> = Vec::new();
    for (future_owner, boxed, mut future) in futures {
        // Leave another loop's future for that loop's wait to poll and settle.
        if future_owner != owner && future_owner != 0 {
            still_pending.push((future_owner, boxed, future));
            continue;
        }
        match future.as_mut().poll(task_cx) {
            std::task::Poll::Ready(outcome) => completed.push((boxed, outcome)),
            std::task::Poll::Pending => still_pending.push((future_owner, boxed, future)),
        }
    }

    // Restore the still-pending futures. Anything spawned during the polls above landed in
    // `PENDING_FUTURES` and is adopted on the next call.
    ACTIVE_FUTURES.with(|a| a.borrow_mut().append(&mut still_pending));
    completed
}

/// Settle promises whose futures completed (from [`poll_pending_futures`]): resolve/reject each and
/// drain the reactions they queue. Must be called with the owning event loop active so a reaction
/// that touches the loop (a timer, releasing interest) reaches the right loop.
///
/// # Safety
///
/// `js_cx` must be a valid `JSContext` whose current realm is entered.
pub unsafe fn settle_completed_futures(scope: &Scope<'_>, completed: Vec<CompletedFuture>) {
    if completed.is_empty() {
        return;
    }
    // Each `boxed` keeps its promise rooted until settled.
    for (boxed, outcome) in completed {
        settle_promise(scope, boxed.get(), outcome);
    }
    // Settling queues the promises' reactions as microtasks; drain them here so the event loop's
    // next `step` sees an empty job queue (mirroring how `step` drains after each task). An
    // exception left pending (a failed resolve/reject above, or job-level fallout) is reported —
    // not silently dropped — matching the event loop's uncaught-exception handling.
    crate::jobs::run_jobs(scope);
    crate::exception::report_and_clear(scope, "async promise settle");
}

/// Settle one completed promise future: resolve with the produced value, or
/// reject with a `TypeError` carrying the error message.
fn settle_promise(scope: &Scope<'_>, promise_obj: *mut JSObject, outcome: PromiseOutcome) {
    let Some(nn) = NonNull::new(promise_obj) else {
        return;
    };
    let promise = unsafe { Stack::<Promise>::from_handle_unchecked(scope.root_object(nn)) };
    match outcome {
        PromiseOutcome::Resolve(resolve) => {
            let mut rval = scope.root_value_mut(value::undefined());
            // SAFETY: All GC references are properly rooted.
            if resolve(unsafe { scope.cx_mut().raw_cx() }, rval.reborrow()) {
                let _ = promise.resolve(scope, rval.handle());
            } else {
                // The resolve callback left a pending exception; reject with it.
                if let Ok(error) = crate::exception::take_pending(scope) {
                    let _ = promise.reject(scope, error);
                }
            }
        }
        PromiseOutcome::Reject(message) => {
            let _ = crate::error::TypeError(message).throw(scope);
            if let Ok(error) = crate::exception::take_pending(scope) {
                let _ = promise.reject(scope, error);
            }
        }
    }
}
