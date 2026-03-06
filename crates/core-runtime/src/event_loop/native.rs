// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Native (blocking) event loop driver.
//!
//! This driver runs a synchronous loop that processes tasks until the event
//! loop is empty. It is intended for CLI / native targets where blocking the
//! thread is acceptable.
//!
//! # Loop structure
//!
//! Each iteration:
//! 1. Run microtasks (promise `.then()` handlers, etc.)
//! 2. Check for JS exceptions — bail if one is pending.
//! 3. Advance timers — mark expired timers as ready.
//! 4. Pop and run all ready tasks. After each task, re-run microtasks.
//! 5. If no tasks are ready and timers are pending, sleep until the next
//!    timer fires, then go to step 1.
//! 6. If the event loop is empty, exit.
//!
//! The [`CURRENT_EVENT_LOOP`] thread-local is set for the duration of the
//! loop so that `JSNative` callbacks (like `setTimeout`) can queue tasks.

use std::thread;

use js::gc::scope::Scope;

use super::{run_microtasks, EventLoop};

/// Run the event loop to completion.
///
/// Blocks the current thread, processing tasks and timers until there is
/// nothing left to do. Returns `Ok(())` if the loop drained cleanly, or
/// `Err(())` if a JS exception was thrown.
///
/// This function should be called after the initial script/module evaluation
/// to process any queued async work (timers, resolved promises, etc.).
#[allow(clippy::result_unit_err)]
pub fn run_to_completion(scope: &Scope<'_>, event_loop: &mut EventLoop) -> Result<(), ()> {
    // Set the thread-local so that JSNative callbacks (setTimeout etc.)
    // can queue tasks on this event loop.
    use super::timer::CURRENT_EVENT_LOOP;
    let ptr = event_loop as *mut EventLoop;
    CURRENT_EVENT_LOOP.with(|el| *el.borrow_mut() = Some(ptr));

    let result = run_loop(scope, event_loop);

    CURRENT_EVENT_LOOP.with(|el| *el.borrow_mut() = None);
    result
}

/// Inner loop — runs with `CURRENT_EVENT_LOOP` already set.
fn run_loop(scope: &Scope<'_>, event_loop: &mut EventLoop) -> Result<(), ()> {
    loop {
        // 1. Drain microtasks.
        run_microtasks(scope);

        // 2. Check for unhandled exceptions.
        // TODO: this should only report exceptions and then continue.
        if js::exception::is_pending(scope) {
            return Err(());
        }

        // 3. Advance timers.
        event_loop.advance_timers();

        // 4. Run all ready tasks.
        let mut ran_any = false;
        while let Some((id, task)) = event_loop.pop_ready() {
            task.run(scope, id)?;
            ran_any = true;

            // After each task, drain microtasks again — the task may
            // have resolved promises or scheduled reactions.
            run_microtasks(scope);

            if js::exception::is_pending(scope) {
                return Err(());
            }

            // Re-advance timers in case task execution took long enough
            // for more timers to expire.
            event_loop.advance_timers();
        }

        // 5. If no tasks ran and nothing is pending, we're done.
        if !event_loop.has_pending() {
            return Ok(());
        }

        // 6. If we have pending timers but nothing is ready, sleep.
        if !ran_any {
            if let Some(wait) = event_loop.time_to_next_timer() {
                if !wait.is_zero() {
                    thread::sleep(wait);
                }
                // After sleeping, go back to step 1 to re-check.
                continue;
            }

            // If there are pending tasks but no timers and nothing is
            // ready, we have tasks that are waiting for external events
            // that will never arrive in the native driver. This is a
            // hang — break out to avoid spinning forever.
            //
            // In a real native async scenario there would be an I/O
            // reactor driving readiness. For now, this covers the
            // timer-only case.
            return Ok(());
        }
    }
}
