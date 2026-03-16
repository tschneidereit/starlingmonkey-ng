// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Promise resolution task for the event loop.
//!
//! A [`PromiseTask`] wraps a completed [`PromiseOutcome`] (resolve or reject)
//! and the SpiderMonkey Promise object it should settle. When run, it calls
//! `ResolvePromise` or `RejectPromise` on the stored promise.
//!
//! This is the event-loop counterpart to the [`JSPromise`] / [`__spawn_promise`]
//! API in [`class`](crate::class). The promise task model allows futures to
//! be driven by the platform driver and resolved individually as they complete.
//!
//! TODO: use js::Promise objects throughout here to avoid `rooted!` and raw pointers.

use std::ffi::CString;

use js::heap::{Heap, RootedTraceableBox, Trace};
use js::native::{JSObject, JSTracer};
use js::rooted;
use js::value;
use js::Promise;

use js::gc::scope::Scope;
use js::promise::PromiseOutcome;

use super::Task;

/// A task that resolves or rejects a JS Promise.
///
/// Created when an async operation completes. The `PromiseOutcome` captures
/// either a resolve callback (which sets the resolution value) or a reject
/// string. The promise object is stored in a `RootedTraceableBox<Heap<...>>`
/// for GC safety.
// TODO: remove use of RootedTraceableBox here and make PromiseTask a proper Traceable.
#[js::must_root]
pub struct PromiseTask {
    /// The JS Promise object to settle, stored in a GC-traced heap wrapper.
    promise: RootedTraceableBox<Heap<*mut JSObject>>,
    /// The outcome to apply: resolve with a value or reject with an error.
    outcome: PromiseOutcome,
}

impl PromiseTask {
    /// Create a new promise task.
    ///
    /// # Safety
    ///
    /// `promise_obj` must be a valid JS Promise object.
    // TODO: make this safe by taking a rooted handle instead of a raw pointer.
    pub unsafe fn new(promise_obj: Promise, outcome: PromiseOutcome) -> Self {
        let promise = RootedTraceableBox::new(Heap::default());
        promise.set(promise_obj.as_raw());
        Self { promise, outcome }
    }
}

impl Task for PromiseTask {
    fn kind(&self) -> &'static str {
        "promise"
    }

    #[js::allow_unrooted]
    fn run(self: Box<Self>, scope: &Scope<'_>, _id: super::TaskId) -> Result<(), ()> {
        let promise_handle = self.promise.handle();
        // TODO: can this null check really ever be hit? If not, we should `expect`.
        if promise_handle.get().is_null() {
            return Ok(());
        }

        unsafe {
            // SAFETY: self.promise stores a known Promise object.
            let promise = Promise::from_handle_unchecked(promise_handle);
            match self.outcome {
                PromiseOutcome::Resolve(set_value) => {
                    rooted!(in(scope.raw_cx_no_gc()) let mut val = value::undefined());
                    if set_value(scope.cx_mut().raw_cx(), val.handle_mut()) {
                        let _ = promise.resolve(scope, val.handle());
                    }
                }
                PromiseOutcome::Reject(msg) => {
                    let c_msg =
                        CString::new(msg).unwrap_or_else(|_| CString::new("async error").unwrap());
                    if let Ok(js_str) = js::JSString::from_cstr(scope, &c_msg) {
                        let err_val = scope.root_value(js_str.as_value());
                        let _ = promise.reject(scope, err_val);
                    }
                }
            }
        }

        Ok(())
    }

    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn trace(&self, trc: *mut JSTracer) {
        // SAFETY: The RootedTraceableBox is self-tracing, but when stored
        // inside the event loop's task vector we trace it explicitly via
        // the extra-roots-tracer mechanism.
        unsafe {
            self.promise.trace(trc);
        }
    }
}
