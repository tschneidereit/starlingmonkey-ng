// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Timer tasks for `setTimeout` and `setInterval`.
//!
//! A [`TimerTask`] holds a reference to a JS callback function and fires
//! it when the event loop runs the task. For `setInterval`, the task
//! re-queues itself with the same delay after each execution.
//!
//! # Global registration
//!
//! Call [`install_timer_globals`] to add `setTimeout`, `setInterval`,
//! `clearTimeout`, and `clearInterval` to a global object. These functions
//! interact with the event loop via the [`CURRENT_EVENT_LOOP`] thread-local
//! set by the platform driver.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use js::error::throw_error;
use js::heap::{Heap, RootedTraceableBox, Trace};
use js::native::{JSObject, JSTracer};

use js::gc::scope::Scope;

use super::{EventLoop, Task, TaskId};

/// A timer task that calls a JS function when it fires.
///
/// For `setInterval`, `interval` is `Some(duration)` and the task
/// re-queues itself with the same [`TaskId`] after each `run()`.
// TODO: remove use of RootedTraceableBox here and make TimerTask a proper Traceable.
pub struct TimerTask {
    /// The JS callback function to invoke.
    callback: RootedTraceableBox<Heap<*mut JSObject>>,
    /// If `Some`, this is a repeating timer (`setInterval`) and will
    /// re-queue itself with this delay after each execution.
    interval: Option<Duration>,
}

impl TimerTask {
    /// Create a one-shot timer task (`setTimeout`).
    ///
    /// # Safety
    ///
    /// `callback` must be a valid JS function object.
    pub unsafe fn one_shot(callback: *mut JSObject) -> Self {
        let heap = RootedTraceableBox::new(Heap::default());
        heap.set(callback);
        Self {
            callback: heap,
            interval: None,
        }
    }

    /// Create a repeating timer task (`setInterval`).
    ///
    /// # Safety
    ///
    /// `callback` must be a valid JS function object.
    pub unsafe fn repeating(callback: *mut JSObject, interval: Duration) -> Self {
        let heap = RootedTraceableBox::new(Heap::default());
        heap.set(callback);
        Self {
            callback: heap,
            interval: Some(interval),
        }
    }
}

impl Task for TimerTask {
    fn kind(&self) -> &'static str {
        if self.interval.is_some() {
            "interval"
        } else {
            "timeout"
        }
    }

    fn run(self: Box<Self>, scope: &Scope<'_>, id: TaskId) -> Result<(), ()> {
        let cb_handle = self.callback.handle();
        // TODO: this shouldn't be possible, so maybe mark it as unreachable? Or just use `js::Object`?
        if cb_handle.get().is_null() {
            return Ok(());
        }

        // Call the callback with no arguments and the global as `this`.
        let result = {
            let fval = scope.root_value(unsafe { js::value::from_object(cb_handle.get()) });
            let global = scope.global();
            let args = js::native::HandleValueArray::empty();
            js::function::call_value(scope, global.handle(), fval, &args)
        };

        if result.is_err() {
            return Err(());
        }

        // For setInterval: re-queue ourselves with the same delay and ID.
        if let Some(interval) = self.interval {
            let new_task = unsafe { TimerTask::repeating(cb_handle.get(), interval) };
            CURRENT_EVENT_LOOP.with(|el| {
                if let Some(el_ptr) = &mut *el.borrow_mut() {
                    // SAFETY: We still have the scope active, so the
                    // callback object is still valid. The new
                    // RootedTraceableBox roots it immediately.
                    let el_ref = unsafe { &mut **el_ptr };
                    // Use the same TaskId for re-queuing. If the ID was
                    // cancelled during the callback (via clearInterval),
                    // requeue_timer will skip the re-queue.
                    el_ref.requeue_timer(id, Box::new(new_task), Instant::now() + interval);
                }
            });
        }

        Ok(())
    }

    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn trace(&self, trc: *mut JSTracer) {
        unsafe {
            self.callback.trace(trc);
        }
    }
}

// ---------------------------------------------------------------------------
// Thread-local event loop pointer for timer globals
// ---------------------------------------------------------------------------

thread_local! {
    /// Temporary reference to the current `EventLoop`, set by the platform
    /// driver while it is actively running the loop. This allows
    /// `setTimeout` and friends (which are `JSNative` callbacks without
    /// access to Rust state) to queue tasks.
    ///
    /// The `*mut EventLoop` is valid only while the driver holds a
    /// `&mut EventLoop` — the driver sets this before calling any JS and
    /// clears it after.
    pub(crate) static CURRENT_EVENT_LOOP: RefCell<Option<*mut EventLoop>> = const { RefCell::new(None) };
}

/// Set the current event loop pointer for the duration of a closure.
///
/// # Safety
///
/// The caller must ensure that `event_loop` remains valid (not dropped
/// or moved) for the entire duration of `f`.
// TODO: consider tying the current event loop to the Scope to make this safe.
pub unsafe fn with_current_event_loop<R>(event_loop: &mut EventLoop, f: impl FnOnce() -> R) -> R {
    let ptr = event_loop as *mut EventLoop;
    CURRENT_EVENT_LOOP.with(|el| {
        let prev = *el.borrow();
        *el.borrow_mut() = Some(ptr);
        let result = f();
        *el.borrow_mut() = prev;
        result
    })
}

/// Access the current event loop from a `JSNative` callback.
///
/// Returns `None` if no event loop is currently active (i.e. we're not
/// inside a platform driver's run loop).
pub fn current_event_loop() -> Option<*mut EventLoop> {
    CURRENT_EVENT_LOOP.with(|el| *el.borrow())
}

// ---------------------------------------------------------------------------
// Timer global functions (setTimeout, setInterval, etc.)
// ---------------------------------------------------------------------------

// TODO: move these to a separate crate under `builtins`, and use `#[jsglobals]`.

use js::native::RawJSContext;
use js::value;

use js::gc::scope::RootScope;

/// Install `setTimeout`, `setInterval`, `clearTimeout`, and `clearInterval`
/// on a global object.
///
/// # Safety
///
/// - `scope` must have an active realm.
/// - `global` must be the realm's global object.
pub unsafe fn install_timer_globals(scope: &Scope<'_>, global: js::Object<'_>) {
    let set_timeout = c"setTimeout";
    let set_interval = c"setInterval";
    let clear_timeout = c"clearTimeout";
    let clear_interval = c"clearInterval";

    js::function::define_function(
        scope,
        global.handle(),
        set_timeout,
        Some(js_set_timeout),
        1,
        0,
    )
    .unwrap();
    js::function::define_function(
        scope,
        global.handle(),
        set_interval,
        Some(js_set_interval),
        1,
        0,
    )
    .unwrap();
    js::function::define_function(
        scope,
        global.handle(),
        clear_timeout,
        Some(js_clear_timeout),
        1,
        0,
    )
    .unwrap();
    js::function::define_function(
        scope,
        global.handle(),
        clear_interval,
        Some(js_clear_interval),
        1,
        0,
    )
    .unwrap();
}

/// `setTimeout(callback, delay?)` — schedule a one-shot timer.
///
/// Returns a numeric timer ID that can be passed to `clearTimeout`.
unsafe extern "C" fn js_set_timeout(
    raw_cx: *mut RawJSContext,
    argc: u32,
    vp: *mut js::native::Value,
) -> bool {
    queue_timer_from_js(raw_cx, argc, vp, false)
}

/// `setInterval(callback, delay?)` — schedule a repeating timer.
///
/// Returns a numeric timer ID that can be passed to `clearInterval`.
unsafe extern "C" fn js_set_interval(
    raw_cx: *mut RawJSContext,
    argc: u32,
    vp: *mut js::native::Value,
) -> bool {
    queue_timer_from_js(raw_cx, argc, vp, true)
}

/// Common implementation for `setTimeout` and `setInterval`.
unsafe fn queue_timer_from_js(
    raw_cx: *mut RawJSContext,
    argc: u32,
    vp: *mut js::native::Value,
    repeating: bool,
) -> bool {
    let mut cx = js::native::JSContext::from_ptr(std::ptr::NonNull::new_unchecked(raw_cx));
    let scope = RootScope::from_current_realm(&mut cx);
    let args = js::native::CallArgs::from_vp(vp, argc);

    // Argument 0: callback (required)
    if argc == 0 || !args.get(0).is_object() {
        throw_error(
            &scope,
            "setTimeout/setInterval requires a function argument",
        );
        return false;
    }
    let callback = args.get(0).to_object();

    // Argument 1: delay in milliseconds (optional, default 0)
    let delay_ms = if argc > 1 && args.get(1).is_number() {
        let d = if args.get(1).is_double() {
            args.get(1).to_double()
        } else {
            args.get(1).to_int32() as f64
        };
        d.max(0.0) as u64
    } else {
        0
    };
    let delay = Duration::from_millis(delay_ms);

    let deadline = Instant::now() + delay;

    // Queue on the current event loop.
    let task_id = CURRENT_EVENT_LOOP.with(|el| {
        if let Some(el_ptr) = &mut *el.borrow_mut() {
            // SAFETY: The event loop pointer is valid for the duration
            // of the platform driver's run loop.
            let el_ref = unsafe { &mut **el_ptr };
            let task: Box<dyn Task> = if repeating {
                Box::new(TimerTask::repeating(callback, delay))
            } else {
                Box::new(TimerTask::one_shot(callback))
            };
            Some(el_ref.queue_timer(task, deadline))
        } else {
            None
        }
    });

    match task_id {
        Some(id) => {
            // Return the task ID as the timer handle (truncated to i32 for JS).
            args.rval().set(value::from_i32(id.0 as i32));
            true
        }
        None => {
            throw_error(&scope, "No active event loop");
            false
        }
    }
}

/// `clearTimeout(id)` / `clearInterval(id)` — cancel a timer.
unsafe extern "C" fn js_clear_timeout(
    raw_cx: *mut RawJSContext,
    argc: u32,
    vp: *mut js::native::Value,
) -> bool {
    clear_timer_from_js(raw_cx, argc, vp)
}

unsafe extern "C" fn js_clear_interval(
    raw_cx: *mut RawJSContext,
    argc: u32,
    vp: *mut js::native::Value,
) -> bool {
    clear_timer_from_js(raw_cx, argc, vp)
}

unsafe fn clear_timer_from_js(
    _raw_cx: *mut RawJSContext,
    argc: u32,
    vp: *mut js::native::Value,
) -> bool {
    let args = js::native::CallArgs::from_vp(vp, argc);

    if argc == 0 {
        args.rval().set(value::undefined());
        return true;
    }

    let id_val = args.get(0);
    let id = if id_val.is_int32() {
        id_val.to_int32() as u64
    } else if id_val.is_double() {
        id_val.to_double() as u64
    } else {
        // Per spec, invalid arguments are silently ignored.
        args.rval().set(value::undefined());
        return true;
    };

    CURRENT_EVENT_LOOP.with(|el| {
        if let Some(el_ptr) = &mut *el.borrow_mut() {
            // SAFETY: The event loop pointer is valid for the duration
            // of the platform driver's run loop.
            let el_ref = unsafe { &mut **el_ptr };
            el_ref.cancel(TaskId(id));
        }
    });

    args.rval().set(value::undefined());
    true
}
