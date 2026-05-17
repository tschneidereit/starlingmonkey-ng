// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Integration tests for the event loop.
//!
//! Tests are grouped in a single test function because `JSEngine` can
//! only be initialized once per process, and must run with --test-threads=1.

#[cfg(not(target_arch = "wasm32"))]
use core_runtime::event_loop::run_to_completion;
use core_runtime::event_loop::timer::install_timer_globals;
use core_runtime::event_loop::{run_microtasks, with_event_loop, EventLoop, StepOutcome, Task};
use core_runtime::runtime::Runtime;
use js::gc::scope::Scope;
use js::native::JSTracer;

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Helper: create a tokio current-thread runtime with timer support and
/// block on `run_to_completion` using `tokio::time::sleep`.
#[cfg(not(target_arch = "wasm32"))]
fn block_on_event_loop(scope: &Scope<'_>, el: &mut EventLoop) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        // SAFETY: the scope (and its Runtime) must outlive this future.
        // Since the caller owns both and `block_on` runs synchronously on
        // the same thread, this is guaranteed.
        let raw_cx = unsafe { scope.raw_cx_no_gc() };
        unsafe { run_to_completion(raw_cx, el, tokio::time::sleep).await }
    });
}

// ---------------------------------------------------------------------------
// Custom task for testing
// ---------------------------------------------------------------------------

/// A simple task that increments a shared counter when run.
struct CounterTask {
    counter: Rc<Cell<u32>>,
    #[allow(dead_code)]
    label: &'static str,
}

impl Task for CounterTask {
    fn kind(&self) -> &'static str {
        "counter"
    }

    fn run(
        self: Box<Self>,
        _scope: &Scope<'_>,
        _id: core_runtime::event_loop::TaskId,
    ) -> Result<(), ()> {
        self.counter.set(self.counter.get() + 1);
        Ok(())
    }

    fn trace(&self, _trc: *mut JSTracer) {}
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_event_loop() {
    let rt = Runtime::init(&core_runtime::config::RuntimeConfig::default());
    let scope = rt.default_global();

    // Install timer globals for timer tests.
    unsafe {
        install_timer_globals(&scope, scope.global());
    }

    // ---- Test 1: Basic EventLoop operations ----
    {
        let mut el = EventLoop::new();

        // Empty loop.
        assert!(el.is_empty());
        assert!(!el.has_pending());
        assert!(!el.has_ready());
        assert_eq!(el.len(), 0);

        // Queue a task.
        let counter = Rc::new(Cell::new(0u32));
        let id = el.queue(Box::new(CounterTask {
            counter: counter.clone(),
            label: "task1",
        }));

        assert!(el.has_pending());
        assert!(!el.has_ready());
        assert_eq!(el.len(), 1);

        // Not ready yet — pop_ready should return None.
        assert!(el.pop_ready().is_none());

        // Signal it ready.
        el.signal_ready(id);
        assert!(el.has_ready());

        // Pop and run it.
        let (popped_id, task) = el.pop_ready().unwrap();
        assert_eq!(popped_id, id);
        assert_eq!(counter.get(), 0);
        task.run(&scope, popped_id).unwrap();
        assert_eq!(counter.get(), 1);

        // Now the loop is empty.
        assert!(el.is_empty());
    }

    // ---- Test 2: queue_ready (immediately ready) ----
    {
        let mut el = EventLoop::new();
        let counter = Rc::new(Cell::new(0u32));

        el.queue_ready(Box::new(CounterTask {
            counter: counter.clone(),
            label: "ready1",
        }));

        assert!(el.has_ready());
        let (id, task) = el.pop_ready().unwrap();
        task.run(&scope, id).unwrap();
        assert_eq!(counter.get(), 1);
        assert!(el.is_empty());
    }

    // ---- Test 3: Cancel ----
    {
        let mut el = EventLoop::new();
        let counter = Rc::new(Cell::new(0u32));

        let id = el.queue_ready(Box::new(CounterTask {
            counter: counter.clone(),
            label: "cancelled",
        }));

        assert!(el.cancel(id));
        assert!(el.is_empty());
        assert_eq!(counter.get(), 0);

        // Cancel of non-existent ID returns false.
        assert!(!el.cancel(id));
    }

    // ---- Test 4: Timer tasks ----
    {
        let mut el = EventLoop::new();
        let counter = Rc::new(Cell::new(0u32));

        // Queue a timer 5ms in the future.
        let _id = el.queue_timer(
            Box::new(CounterTask {
                counter: counter.clone(),
                label: "timer1",
            }),
            Instant::now() + Duration::from_millis(5),
        );

        assert!(el.has_pending());
        assert!(!el.has_ready());

        // advance_timers should NOT mark it ready yet.
        assert_eq!(el.advance_timers(), 0);
        assert!(!el.has_ready());

        // time_to_next_timer should be Some and > 0.
        let wait = el.time_to_next_timer();
        assert!(wait.is_some());

        // Sleep past the deadline.
        std::thread::sleep(Duration::from_millis(10));

        // Now advance_timers should mark it ready.
        assert_eq!(el.advance_timers(), 1);
        assert!(el.has_ready());

        let (id, task) = el.pop_ready().unwrap();
        task.run(&scope, id).unwrap();
        assert_eq!(counter.get(), 1);
    }

    // ---- Test 5: Multiple tasks, ordering ----
    {
        let mut el = EventLoop::new();
        let counter = Rc::new(Cell::new(0u32));

        let id1 = el.queue_ready(Box::new(CounterTask {
            counter: counter.clone(),
            label: "first",
        }));
        let id2 = el.queue_ready(Box::new(CounterTask {
            counter: counter.clone(),
            label: "second",
        }));
        let id3 = el.queue_ready(Box::new(CounterTask {
            counter: counter.clone(),
            label: "third",
        }));

        // Pop and run all three.
        let mut ids = Vec::new();
        while let Some((id, task)) = el.pop_ready() {
            ids.push(id);
            task.run(&scope, id).unwrap();
        }

        assert_eq!(counter.get(), 3);
        assert!(el.is_empty());
        // All three IDs should be unique.
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id2));
        assert!(ids.contains(&id3));
    }

    // ---- Test 6: run_to_completion with immediate tasks ----
    #[cfg(not(target_arch = "wasm32"))]
    {
        let counter = Rc::new(Cell::new(0u32));
        let mut el = EventLoop::new();

        el.queue_ready(Box::new(CounterTask {
            counter: counter.clone(),
            label: "native1",
        }));
        el.queue_ready(Box::new(CounterTask {
            counter: counter.clone(),
            label: "native2",
        }));

        block_on_event_loop(&scope, &mut el);
        assert_eq!(counter.get(), 2);
        assert!(el.is_empty());
    }

    // ---- Test 7: run_to_completion with timer ----
    #[cfg(not(target_arch = "wasm32"))]
    {
        let counter = Rc::new(Cell::new(0u32));
        let mut el = EventLoop::new();

        el.queue_timer(
            Box::new(CounterTask {
                counter: counter.clone(),
                label: "delayed",
            }),
            Instant::now() + Duration::from_millis(5),
        );

        block_on_event_loop(&scope, &mut el);
        assert_eq!(counter.get(), 1);
        assert!(el.is_empty());
    }

    // ---- Test 8: setTimeout from JavaScript ----
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut el = EventLoop::new();

        // Evaluate JS with the event loop thread-local set so setTimeout works.
        unsafe {
            with_event_loop(&mut el, |_| {
                let ok = js::compile::evaluate_with_filename(
                    &scope,
                    "globalThis._timerFired = false; setTimeout(function() { globalThis._timerFired = true; }, 1);",
                    "<test>",
                    1,
                );
                assert!(ok.is_ok(), "setTimeout JS evaluation failed");
            });
        }

        // The timer task should be queued.
        assert!(el.has_pending());

        // Run the event loop to fire the timer.
        block_on_event_loop(&scope, &mut el);

        // Verify the timer callback ran.
        let result = js::compile::evaluate_with_filename(
            &scope,
            "globalThis._timerFired",
            "<test-check>",
            1,
        );
        assert!(result.is_ok());
        let val = result.unwrap();
        assert!(
            val.is_boolean() && val.to_boolean(),
            "setTimeout callback should have fired"
        );
    }

    // ---- Test 9: clearTimeout cancels a timer ----
    {
        let mut el = EventLoop::new();

        unsafe {
            with_event_loop(&mut el, |_| {
                let ok = js::compile::evaluate_with_filename(
                    &scope,
                    "globalThis._cleared = true; var tid = setTimeout(function() { globalThis._cleared = false; }, 1); clearTimeout(tid);",
                    "<test-clear>",
                    1,
                );
                assert!(ok.is_ok());
            });
        }

        // After clearTimeout, the event loop should be empty.
        assert!(el.is_empty(), "clearTimeout should have removed the timer");

        // Verify the callback did NOT run.
        let result = js::compile::evaluate_with_filename(
            &scope,
            "globalThis._cleared",
            "<test-check-clear>",
            1,
        );
        assert!(result.is_ok());
        let val = result.unwrap();
        assert!(
            val.is_boolean() && val.to_boolean(),
            "clearTimeout should prevent callback"
        );
    }

    // ---- Test 10: setInterval fires multiple times ----
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut el = EventLoop::new();

        unsafe {
            with_event_loop(&mut el, |_| {
                let ok = js::compile::evaluate_with_filename(
                    &scope,
                    r#"
                    globalThis._intervalCount = 0;
                    globalThis._intervalId = setInterval(function() {
                        globalThis._intervalCount++;
                        if (globalThis._intervalCount >= 3) {
                            clearInterval(globalThis._intervalId);
                        }
                    }, 1);
                    "#,
                    "<test-interval>",
                    1,
                );
                assert!(ok.is_ok());
            });
        }

        assert!(el.has_pending());

        block_on_event_loop(&scope, &mut el);

        // Verify the interval fired exactly 3 times and then stopped.
        let result = js::compile::evaluate_with_filename(
            &scope,
            "globalThis._intervalCount",
            "<test-check-interval>",
            1,
        );
        assert!(result.is_ok());
        let val = result.unwrap();
        let count = if val.is_int32() {
            val.to_int32()
        } else {
            val.to_double() as i32
        };
        assert_eq!(count, 3, "setInterval should have fired exactly 3 times");
    }

    // ---- Test 11: Event loop with microtasks (promise reactions) ----
    {
        // Evaluate JS that creates a resolved promise — the .then callback
        // should run during run_microtasks.
        let ok = js::compile::evaluate_with_filename(
            &scope,
            "globalThis._promiseResult = 0; Promise.resolve(42).then(v => { globalThis._promiseResult = v; });",
            "<test-promise>",
            1,
        );
        assert!(ok.is_ok());

        // Run microtasks to process the promise.
        run_microtasks(&scope);

        let result = js::compile::evaluate_with_filename(
            &scope,
            "globalThis._promiseResult",
            "<test-check-promise>",
            1,
        );
        assert!(result.is_ok());
        let val = result.unwrap();
        let n = if val.is_int32() {
            val.to_int32()
        } else {
            val.to_double() as i32
        };
        assert_eq!(n, 42, "Promise.resolve .then should have run");
    }

    // ---- Test 12: step() returns Done on empty event loop ----
    {
        let mut el = EventLoop::new();
        let outcome = el.step(&scope);
        assert_eq!(outcome, StepOutcome::Done);
    }

    // ---- Test 13: step() returns Progressed when tasks run ----
    {
        let mut el = EventLoop::new();
        let counter = Rc::new(Cell::new(0u32));

        el.queue_ready(Box::new(CounterTask {
            counter: counter.clone(),
            label: "step-prog",
        }));

        let outcome = el.step(&scope);
        assert_eq!(outcome, StepOutcome::Progressed);
        assert_eq!(counter.get(), 1);
    }

    // ---- Test 14: step() returns Idle when tasks exist but none ready ----
    {
        let mut el = EventLoop::new();
        let counter = Rc::new(Cell::new(0u32));

        // Queue a task but don't signal it ready.
        let _id = el.queue(Box::new(CounterTask {
            counter: counter.clone(),
            label: "step-idle",
        }));

        let outcome = el.step(&scope);
        assert_eq!(outcome, StepOutcome::Idle);
        assert_eq!(counter.get(), 0);
    }

    // ---- Test 15: step() returns Idle with interest but no tasks ----
    {
        let mut el = EventLoop::new();

        el.acquire_interest();
        assert!(el.has_interest());
        assert!(el.is_alive());

        let outcome = el.step(&scope);
        assert_eq!(outcome, StepOutcome::Idle);

        el.release_interest();
        assert!(!el.has_interest());

        let outcome = el.step(&scope);
        assert_eq!(outcome, StepOutcome::Done);
    }

    // ---- Test 16: is_alive combines interest and pending ----
    {
        let mut el = EventLoop::new();

        assert!(!el.is_alive());

        // Interest alone keeps it alive.
        el.acquire_interest();
        assert!(el.is_alive());
        el.release_interest();
        assert!(!el.is_alive());

        // Pending tasks alone keep it alive.
        let counter = Rc::new(Cell::new(0u32));
        el.queue(Box::new(CounterTask {
            counter: counter.clone(),
            label: "alive-test",
        }));
        assert!(el.is_alive());
    }

    // ---- Test 17: step() runs timer tasks after advance ----
    {
        let mut el = EventLoop::new();
        let counter = Rc::new(Cell::new(0u32));

        // Queue a timer with deadline in the past.
        el.queue_timer(
            Box::new(CounterTask {
                counter: counter.clone(),
                label: "step-timer",
            }),
            Instant::now(),
        );

        // step() advances timers internally.
        let outcome = el.step(&scope);
        assert_eq!(outcome, StepOutcome::Progressed);
        assert_eq!(counter.get(), 1);
        assert!(el.is_empty());
    }

    // ---- Test 18: run_to_completion respects interest tracking ----
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut el = EventLoop::new();

        // Acquire interest, queue a task that releases it.
        el.acquire_interest();

        struct ReleaseInterestTask;
        impl Task for ReleaseInterestTask {
            fn kind(&self) -> &'static str {
                "release-interest"
            }
            fn run(
                self: Box<Self>,
                _scope: &Scope<'_>,
                _id: core_runtime::event_loop::TaskId,
            ) -> Result<(), ()> {
                core_runtime::event_loop::with_active_event_loop(|el| {
                    el.release_interest();
                });
                Ok(())
            }
            fn trace(&self, _trc: *mut JSTracer) {}
        }

        el.queue_ready(Box::new(ReleaseInterestTask));

        // With interest, the native driver should run the task and then
        // exit once interest drops to zero and no tasks remain.
        block_on_event_loop(&scope, &mut el);
        assert!(!el.has_interest());
        assert!(el.is_empty());
    }
}
