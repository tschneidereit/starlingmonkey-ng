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
//! interact with the event loop via [`with_active_event_loop`].

use std::time::{Duration, Instant};

use js::conversion::ToJSVal;
use js::error::throw_error;
use js::gc::handle::Heap;
use js::heap::RootedTraceableBox;
use js::native::Value;

use js::gc::scope::Scope;

use super::{with_active_event_loop, Task, TaskId};

/// A timer's handler, per WebIDL `TimerHandler = (Function or DOMString)`: a JS function to call, or
/// a string to evaluate as a classic script when the timer fires.
enum TimerHandler {
    Function {
        callback: RootedTraceableBox<Heap<js::object::Object>>,
        /// The `setTimeout`/`setInterval` arguments after the timeout, passed
        /// to the callback on every invocation (HTML timer initialization
        /// steps: "invoke handler given arguments").
        args: Vec<RootedTraceableBox<Heap<Value>>>,
    },
    Code(String),
}

thread_local! {
    /// The HTML "timer nesting level" of the timer task currently running on this
    /// thread (0 outside timer tasks). The timer initialization steps read it to
    /// clamp deeply nested zero-delay timers, and each fired timer's handler runs
    /// with it set to that timer's own level.
    static CURRENT_TIMER_NESTING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// HTML timer initialization steps, step 5: "If nesting level is greater than 5,
/// and timeout is less than 4, then set timeout to 4." Without the clamp, a
/// zero-delay timer chain (`setInterval(f, 0)`, recursive `setTimeout(f, 0)`)
/// is perpetually ready: the loop never reaches its await branch, so
/// async-promise futures (fetch I/O) are never polled and never settle.
fn clamp_nested_timeout(nesting_level: u32, delay: Duration) -> Duration {
    const MIN_NESTED: Duration = Duration::from_millis(4);
    if nesting_level > 5 && delay < MIN_NESTED {
        MIN_NESTED
    } else {
        delay
    }
}

/// A timer task that runs its [`TimerHandler`] when it fires.
///
/// For `setInterval`, `interval` is `Some(duration)` and the task
/// re-queues itself with the same [`TaskId`] after each `run()`.
pub struct TimerTask {
    handler: TimerHandler,
    /// If `Some`, this is a repeating timer (`setInterval`) and will
    /// re-queue itself with this delay after each execution.
    interval: Option<Duration>,
    /// The task's HTML "timer nesting level": one more than the level of the
    /// timer task that scheduled it (0 from non-timer code).
    nesting_level: u32,
    /// The JS-visible timer id this task is registered under in the event
    /// loop's map of setTimeout and setInterval IDs.
    timer_id: u64,
}

impl TimerTask {
    /// Create a timer task with a function handler and its call arguments.
    ///
    /// # Safety
    ///
    /// `callback` must be a valid JS function object.
    unsafe fn function(
        callback: Object,
        args: Vec<RootedTraceableBox<Heap<Value>>>,
        interval: Option<Duration>,
        nesting_level: u32,
        timer_id: u64,
    ) -> Self {
        Self {
            handler: TimerHandler::Function {
                callback: RootedTraceableBox::new(Heap::from(callback)),
                args,
            },
            interval,
            nesting_level,
            timer_id,
        }
    }

    /// Create a timer task whose handler is a code string evaluated when it fires.
    fn code(code: String, interval: Option<Duration>, nesting_level: u32, timer_id: u64) -> Self {
        Self {
            handler: TimerHandler::Code(code),
            interval,
            nesting_level,
            timer_id,
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
        let Self {
            handler,
            interval,
            nesting_level,
            timer_id,
        } = *self;

        // Run the handler with the global as `this`: call the function, or evaluate the string
        // as a classic script in the global scope.
        // Timers the handler schedules nest one level below this task (see
        // `CURRENT_TIMER_NESTING`); restore the previous level afterwards.
        let previous_nesting = CURRENT_TIMER_NESTING.with(|cell| cell.replace(nesting_level));
        let failed = match &handler {
            TimerHandler::Function { callback, args } => {
                let cb = callback.get(scope);
                let fval = scope.root_value(cb.as_value());
                let arg_handles: Vec<_> = args.iter().map(|arg| arg.get(scope)).collect();
                js::Function::call_value(scope, scope.global().handle(), fval, &arg_handles)
                    .is_err()
            }
            TimerHandler::Code(code) => {
                js::compile::evaluate_with_filename(scope, code, "<timer>", 1).is_err()
            }
        };
        CURRENT_TIMER_NESTING.with(|cell| cell.set(previous_nesting));

        // For setInterval: re-queue ourselves (reusing the same handler) with the same delay and ID
        // — even when the handler threw. The spec's task substeps invoke the handler with "report"
        // (the exception is reported, not fatal) and still perform the repeat step, so a transient
        // error must not kill the interval. If the ID was cancelled during the run (via
        // clearInterval), requeue_timer skips it. The repeat re-enters the timer initialization
        // steps, so the nested-timer clamp applies with this task's nesting level, and the
        // re-queued task is one level deeper.
        if let Some(interval) = interval {
            let delay = clamp_nested_timeout(nesting_level, interval);
            let new_task = TimerTask {
                handler,
                interval: Some(interval),
                nesting_level: nesting_level.saturating_add(1),
                timer_id,
            };
            with_active_event_loop(|el| {
                el.requeue_js_timer(timer_id, id, Box::new(new_task), Instant::now() + delay);
            });
        } else {
            // One-shot: release the timer id, mirroring HTML's "remove
            // global's map of setTimeout and setInterval IDs[id]" substep
            // after the handler ran.
            with_active_event_loop(|el| el.js_timer_fired(timer_id));
        }

        if failed {
            // The caller (EventLoop::step) reports and clears the exception.
            return Err(());
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Timer global functions (setTimeout, setInterval, etc.)
// ---------------------------------------------------------------------------

// TODO: move these to a separate crate under `builtins`, and use `#[jsglobals]`.

use js::native::RawJSContext;
use js::{value, Object};

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

    js::Function::define(
        scope,
        global.handle(),
        set_timeout,
        Some(js_set_timeout),
        1,
        0,
    )
    .unwrap();
    js::Function::define(
        scope,
        global.handle(),
        set_interval,
        Some(js_set_interval),
        1,
        0,
    )
    .unwrap();
    js::Function::define(
        scope,
        global.handle(),
        clear_timeout,
        Some(js_clear_timeout),
        1,
        0,
    )
    .unwrap();
    js::Function::define(
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
    let scope = RootScope::from_current_realm(raw_cx);
    let args = js::native::CallArgs::from_vp(vp, argc);

    // Argument 0: handler (required) — WebIDL `TimerHandler = (Function or DOMString)`. A callable is
    // invoked when the timer fires; anything else is coerced to a string and evaluated as code (the
    // `setTimeout("code", ms)` form).
    if argc == 0 {
        throw_error(&scope, "setTimeout/setInterval requires a handler argument");
        return false;
    }
    let arg0 = args.get(0);
    let callable = arg0
        .is_object()
        .then(|| Object::from_value(&scope, *arg0).ok())
        .flatten()
        .filter(|object| object.is_callable());

    let code = match &callable {
        Some(_) => None,
        None => {
            use js::conversion::FromJSVal;
            match String::from_jsval(&scope, js::native::Handle::from_raw(arg0), ()) {
                Ok(code) => Some(code),
                Err(error) => {
                    error.throw(&scope);
                    return false;
                }
            }
        }
    };

    // Argument 1: delay in milliseconds (optional, default 0).
    // <https://html.spec.whatwg.org/multipage/timers-and-user-prompts.html#timer-initialisation-steps>
    let delay_ms = if argc > 1 {
        use js::conversion::{ConversionBehavior, FromJSVal};
        let delay_handle = js::native::Handle::from_raw(args.get(1));
        match i32::from_jsval(&scope, delay_handle, ConversionBehavior::Default) {
            Ok(delay) => delay.max(0) as u64,
            Err(error) => {
                error.throw(&scope);
                return false;
            }
        }
    } else {
        0
    };
    let delay = Duration::from_millis(delay_ms);

    // HTML timer initialization steps: "Let nesting level be the task's timer nesting level"
    // (the currently running timer task's, or 0), clamp the timeout once nested deeper than
    // five levels, and give the new task a level one greater.
    let nesting_level = CURRENT_TIMER_NESTING.with(|cell| cell.get());
    let delay = clamp_nested_timeout(nesting_level, delay);
    let task_nesting = nesting_level.saturating_add(1);

    let deadline = Instant::now() + delay;

    let interval = repeating.then_some(delay);

    // HTML timer initialization steps: "Let arguments be... the rest of the arguments"
    // For the handler form, everything after the timeout is forwarded to the callback on every
    // invocation. For the string form, additional arguments are ignored.
    let extra_args: Vec<RootedTraceableBox<Heap<Value>>> = match &callable {
        Some(_) => (2..argc)
            .map(|i| RootedTraceableBox::new(Heap::from(*args.get(i))))
            .collect(),
        None => Vec::new(),
    };

    // Queue on the current event loop.
    let timer_id = with_active_event_loop(|el| {
        el.queue_js_timer(&scope, deadline, |timer_id| match callable {
            Some(callback) => Box::new(TimerTask::function(
                callback,
                extra_args,
                interval,
                task_nesting,
                timer_id,
            )),
            None => {
                // Not callable: run the handler's string as code.
                // TODO: can we do the eval once instead of on every fire?
                let code = code.expect("non-callable handler was converted to a string above");
                Box::new(TimerTask::code(code, interval, task_nesting, timer_id))
            }
        })
    });

    match timer_id {
        Some(timer_id) => {
            args.rval().set(timer_id.to_jsval(&scope).unwrap().get());
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
    raw_cx: *mut RawJSContext,
    argc: u32,
    vp: *mut js::native::Value,
) -> bool {
    let scope = RootScope::from_current_realm(raw_cx);
    let args = js::native::CallArgs::from_vp(vp, argc);
    let id = if argc > 0 {
        use js::conversion::{ConversionBehavior, FromJSVal};
        let id_handle = js::native::Handle::from_raw(args.get(0));
        match u64::from_jsval(&scope, id_handle, ConversionBehavior::Default) {
            Ok(id) => id,
            Err(error) => {
                error.throw(&scope);
                return false;
            }
        }
    } else {
        0
    };

    with_active_event_loop(|el| el.clear_js_timer(id));

    args.rval().set(value::undefined());
    true
}
