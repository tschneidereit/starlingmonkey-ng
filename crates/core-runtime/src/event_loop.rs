// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Event loop for the StarlingMonkey runtime.
//!
//! The event loop manages asynchronous tasks (timers, I/O completions, promise
//! resolutions, etc.) and drives them to completion. It is designed around three
//! layered abstractions:
//!
//! 1. **[`Task`] trait** — extensible interface that builtins implement. The
//!    event loop knows nothing about specific task types; it just stores trait
//!    objects and runs them when ready.
//!
//! 2. **[`EventLoop`] struct** — platform-independent task registry. Manages
//!    queuing, cancellation, readiness signaling, timer advancement, GC
//!    tracing, and notification of idle drivers via [`event_listener::Event`].
//!
//! 3. **[`run_to_completion`]** — a single `async fn` driver that calls
//!    [`EventLoop::step`] in a loop and, when idle, races the next timer
//!    deadline against a readiness notification. Platform differences are
//!    confined to the sleep function and the executor:
//!    - [`wasi`]: `wasi:clocks/monotonic-clock.wait-for` for sleep,
//!      `wasip3::wit_bindgen::spawn` as executor.
//!    - [`native`]: the caller provides a proper async sleep (e.g.
//!      `tokio::time::sleep`) and executor (e.g.
//!      `tokio::runtime::Runtime::block_on`). This keeps the core runtime
//!      free of any specific async runtime dependency.
//!
//! # Task lifecycle
//!
//! ```text
//! queue(task) → Queued → signal_ready(id) → Ready → pop_ready() → run(scope)
//!                 │                                                   │
//!                 └── cancel(id) ────────────────────────── dropped ◄─┘
//! ```
//!
//! Tasks start in the **Queued** state. External events (timer expiry, I/O
//! completion, a future resolving) move them to **Ready** via
//! [`EventLoop::signal_ready`]. The platform driver calls
//! [`EventLoop::pop_ready`] to take the next ready task and then runs it.
//! Running consumes the task (`self: Box<Self>`); repeating behaviors like
//! `setInterval` re-queue themselves inside `run()`.
//!
//! # GC integration
//!
//! Tasks that hold references to GC-managed JS objects must trace them in
//! [`Task::trace`]. The [`EventLoop`] is registered as a SpiderMonkey
//! extra-roots tracer so that all live tasks are traced during both minor
//! and major GC.
//!
//! # Example: implementing a custom task
//!
//! ```rust,ignore
//! struct MyTask { /* ... */ }
//!
//! impl Task for MyTask {
//!     fn kind(&self) -> &'static str { "my-task" }
//!
//!     fn run(self: Box<Self>, scope: &Scope<'_>, _id: TaskId) -> Result<(), ()> {
//!         // Do JS work using `scope`
//!         Ok(())
//!     }
//!
//!     fn trace(&self, _trc: *mut JSTracer) {
//!         // Trace any Heap<*mut JSObject> fields here
//!     }
//! }
//!
//! // Queue it:
//! let id = event_loop.queue(Box::new(MyTask { /* ... */ }));
//!
//! // Later, when the task is ready:
//! event_loop.signal_ready(id);
//! ```

pub mod interest;
pub mod promise;
pub mod spawner;
pub mod timer;

use std::cell::RefCell;
use std::collections::HashSet;
use std::time::{Duration, Instant};

use event_listener::{Event, EventListener};
use js::error::ExnThrown;
use js::native::JSTracer;

use js::gc::scope::Scope;
use js::jobs;

pub use interest::InterestTracker;

// ---------------------------------------------------------------------------
// Current event loop pointer
// ---------------------------------------------------------------------------

thread_local! {
    /// Temporary reference to the active [`EventLoop`], set by the platform
    /// driver while it is running. `JSNative` callbacks (`setTimeout` etc.)
    /// have no other way to reach Rust state, so they read this pointer.
    ///
    /// Valid only while the driver holds a `&mut EventLoop` — set before
    /// calling any JS and cleared immediately after.
    pub(crate) static CURRENT_EVENT_LOOP: RefCell<Option<*mut EventLoop>> = const { RefCell::new(None) };
}

/// Set the current event loop for the duration of a closure.
///
/// Saves and restores the previous value, so calls may be nested.
///
/// # Safety
///
/// `event_loop` must remain valid (not dropped or moved) for the entire
/// duration of `f`. The caller must not create any other `&mut EventLoop`
/// to the same event loop during `f()` — in particular, `with_active_event_loop`
/// must not be called on the same loop re-entrantly in a way that could alias
/// the caller's borrow.
///
/// This function is inherently unsafe because `JSNative` callbacks are C
/// function pointers (`fn(*mut RawJSContext, u32, *mut Value) -> bool`) that
/// carry no Rust lifetime information. There is no way to tie the event loop
/// pointer to the `Scope` lifetime through the C ABI boundary.
pub unsafe fn with_event_loop<R>(
    event_loop: &mut EventLoop,
    f: impl FnOnce(&mut EventLoop) -> R,
) -> R {
    let ptr = event_loop as *mut EventLoop;
    CURRENT_EVENT_LOOP.with(|el| {
        let prev = *el.borrow();
        *el.borrow_mut() = Some(ptr);
        let result = f(event_loop);
        *el.borrow_mut() = prev;
        result
    })
}

/// Run a closure with a mutable reference to the active event loop.
///
/// Returns `None` if no event loop is active (i.e. we're not inside a
/// driver's run loop). The raw pointer never escapes this function.
pub fn with_active_event_loop<R>(f: impl FnOnce(&mut EventLoop) -> R) -> Option<R> {
    CURRENT_EVENT_LOOP.with(|el| {
        el.borrow().map(|ptr| {
            // SAFETY: The pointer is set by the platform driver immediately
            // before calling JS and cleared immediately after, so it is
            // valid for at least the lifetime of this closure.
            f(unsafe { &mut *ptr })
        })
    })
}

/// Set the thread-local current event loop pointer.
///
/// Must be called before running any JS code that might invoke
/// `setTimeout` / `setInterval`. Call [`clear_current_event_loop`] when done.
///
/// # Safety
///
/// `event_loop` must remain valid and at a stable address for as long as
/// the pointer is set.
pub unsafe fn set_current_event_loop(event_loop: &mut EventLoop) {
    let ptr = event_loop as *mut EventLoop;
    CURRENT_EVENT_LOOP.with(|el| *el.borrow_mut() = Some(ptr));
}

/// Clear the thread-local current event loop pointer.
pub fn clear_current_event_loop() {
    CURRENT_EVENT_LOOP.with(|el| *el.borrow_mut() = None);
}

// ---------------------------------------------------------------------------
// TaskId
// ---------------------------------------------------------------------------

/// Opaque identifier for a queued task.
///
/// Task IDs are unique within a single [`EventLoop`] instance and are never
/// reused.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TaskId(u64);

impl TaskId {
    /// Create a `TaskId` from a raw `u64` value.
    ///
    /// Test-only, since in production code it'd be a footgun.
    #[cfg(test)]
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Returns the raw `u64` value.
    ///
    /// Test-only, since in production code it'd be a footgun.
    #[cfg(test)]
    pub fn as_raw(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TaskId({})", self.0)
    }
}

// ---------------------------------------------------------------------------
// StepOutcome
// ---------------------------------------------------------------------------

/// Result of a single [`EventLoop::step`] iteration.
///
/// Platform drivers use this to decide what to do next: exit, wait for
/// external events, or immediately step again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StepOutcome {
    /// The event loop has completed — no pending tasks and no external
    /// interest. The driver should exit.
    Done,

    /// There is still pending work (queued tasks or external interest),
    /// but nothing was ready to run in this step. The driver should wait
    /// for an external event (timer expiry, I/O completion, etc.) before
    /// stepping again.
    Idle,

    /// At least one task ran successfully. The driver should step again
    /// immediately — there may be more ready work.
    Progressed,
}

// ---------------------------------------------------------------------------
// Task trait
// ---------------------------------------------------------------------------

/// A unit of asynchronous work managed by the event loop.
///
/// Implementations are provided by individual builtins (timers, fetch,
/// promise resolution, etc.). The event loop stores tasks as trait objects
/// and is completely agnostic to their concrete types.
pub trait Task {
    /// A human-readable label for this task type (e.g. `"timer"`,
    /// `"promise"`, `"fetch"`). Used for debugging and diagnostics only.
    fn kind(&self) -> &'static str;

    /// Execute the task's work.
    ///
    /// Receives a `&Scope<'_>` for interacting with JavaScript and the
    /// task's [`TaskId`] (useful for re-queuing, e.g. `setInterval`).
    /// The task is consumed (`self: Box<Self>`) — repeating tasks should
    /// re-queue themselves inside this method.
    ///
    /// Returns `Ok(())` on success or `Err(())` if a JS exception was
    /// thrown (the caller is responsible for reporting it).
    #[allow(clippy::result_unit_err)]
    fn run(self: Box<Self>, scope: &Scope<'_>, id: TaskId) -> Result<(), ()>;

    /// Trace any GC-managed pointers held by this task.
    ///
    /// Called by SpiderMonkey during garbage collection. Implementations
    /// must call `.trace(trc)` on every `Heap<*mut JSObject>` (or other
    /// `Trace`-implementing) field.
    ///
    /// The default implementation is a no-op, which is correct for tasks
    /// that hold no JS references.
    fn trace(&self, _trc: *mut JSTracer) {}
}

// ---------------------------------------------------------------------------
// Task entry (internal)
// ---------------------------------------------------------------------------

/// Internal wrapper pairing a [`TaskId`] with its [`Task`] and readiness
/// state.
struct TaskEntry {
    id: TaskId,
    task: Box<dyn Task>,
    ready: bool,
    /// For timer tasks: the `Instant` at which this task becomes ready.
    /// `None` for non-timer tasks.
    deadline: Option<Instant>,
}

// ---------------------------------------------------------------------------
// EventLoop
// ---------------------------------------------------------------------------

/// The event loop task registry.
///
/// Owns all queued tasks and tracks which are ready to run. Platform
/// drivers ([`native`], [`wasi`]) call into this struct to advance the
/// loop.
///
/// The `EventLoop` is stored on the [`Runtime`](crate::runtime::Runtime)
/// and its [`trace`](EventLoop::trace) method is called during GC to keep
/// JS references inside tasks alive.
pub struct EventLoop {
    /// Monotonically increasing counter for generating unique [`TaskId`]s.
    next_id: u64,
    /// All live tasks. Order is not significant — tasks are looked up by
    /// [`TaskId`].
    tasks: Vec<TaskEntry>,
    /// IDs that were cancelled while the corresponding task was being
    /// executed (i.e. popped from `tasks`). Used to suppress interval
    /// timer re-queuing when `clearInterval` is called from within the
    /// interval callback.
    cancelled_while_running: HashSet<TaskId>,
    /// External keep-alive interest. When positive, the event loop stays
    /// alive even with an empty task queue.
    interest: InterestTracker,
    /// Notification event: signaled whenever a task becomes ready or a
    /// timer is queued. The async driver awaits this to avoid busy-polling.
    notify: Event,
}

impl EventLoop {
    /// Create a new, empty event loop.
    pub fn new() -> Self {
        Self {
            next_id: 0,
            tasks: Vec::new(),
            cancelled_while_running: HashSet::new(),
            interest: InterestTracker::new(),
            notify: Event::new(),
        }
    }

    /// Queue a task for later execution.
    ///
    /// The task starts in the **Queued** (not ready) state. Call
    /// [`signal_ready`](Self::signal_ready) to mark it runnable.
    ///
    /// Returns the [`TaskId`] assigned to this task.
    pub fn queue(&mut self, task: Box<dyn Task>) -> TaskId {
        let id = self.next_task_id();
        self.tasks.push(TaskEntry {
            id,
            task,
            ready: false,
            deadline: None,
        });
        id
    }

    /// Queue a task that is **immediately ready** to run.
    ///
    /// This is a convenience for tasks that don't need to wait for an
    /// external event (e.g. resolved promises, `queueMicrotask` work).
    pub fn queue_ready(&mut self, task: Box<dyn Task>) -> TaskId {
        let id = self.next_task_id();
        self.tasks.push(TaskEntry {
            id,
            task,
            ready: true,
            deadline: None,
        });
        self.notify.notify(1);
        id
    }

    /// Queue a task with a timer deadline.
    ///
    /// The task will become ready when [`advance_timers`](Self::advance_timers)
    /// detects that the deadline has passed. For `setTimeout(fn, 0)` or
    /// similar, use a deadline of `Instant::now()`.
    pub fn queue_timer(&mut self, task: Box<dyn Task>, deadline: Instant) -> TaskId {
        let id = self.next_task_id();
        self.tasks.push(TaskEntry {
            id,
            task,
            ready: false,
            deadline: Some(deadline),
        });
        // Wake the driver so it can re-evaluate the earliest timer deadline.
        self.notify.notify(1);
        id
    }

    /// Cancel a queued task, removing it from the event loop.
    ///
    /// Returns `true` if the task was found and removed. If the task is
    /// not in the queue (e.g. it is currently being executed by the
    /// driver), the ID is recorded so that interval re-queuing is
    /// suppressed for that ID.
    pub fn cancel(&mut self, id: TaskId) -> bool {
        if let Some(pos) = self.tasks.iter().position(|e| e.id == id) {
            self.tasks.swap_remove(pos);
            true
        } else {
            // The task may be currently running (popped for execution).
            // Record it so requeue_timer can check.
            self.cancelled_while_running.insert(id);
            false
        }
    }

    /// Re-queue an interval timer task with a specific [`TaskId`].
    ///
    /// If the ID was cancelled during execution (via `clearInterval`
    /// called from within the callback), the task is silently dropped
    /// instead of re-queued.
    pub fn requeue_timer(&mut self, id: TaskId, task: Box<dyn Task>, deadline: Instant) {
        if self.cancelled_while_running.remove(&id) {
            // clearInterval was called during the callback — don't re-queue.
            return;
        }
        self.tasks.push(TaskEntry {
            id,
            task,
            ready: false,
            deadline: Some(deadline),
        });
        // Wake the driver so it can re-evaluate the earliest timer deadline.
        self.notify.notify(1);
    }

    /// Mark a queued task as ready to run.
    ///
    /// Has no effect if the task ID is not found (the task may have
    /// already been cancelled or run).
    pub fn signal_ready(&mut self, id: TaskId) {
        if let Some(entry) = self.tasks.iter_mut().find(|e| e.id == id) {
            entry.ready = true;
            self.notify.notify(1);
        }
    }

    /// Take the next ready task out of the queue.
    ///
    /// Returns `None` if no tasks are currently ready. The returned task
    /// is removed from the event loop — the caller must `run()` it.
    pub fn pop_ready(&mut self) -> Option<(TaskId, Box<dyn Task>)> {
        let pos = self.tasks.iter().position(|e| e.ready)?;
        let entry = self.tasks.swap_remove(pos);
        Some((entry.id, entry.task))
    }

    /// Check all timer-based tasks and mark those whose deadline has
    /// passed as ready.
    ///
    /// Returns the number of timers that became ready.
    // TODO: consider merging with `time_to_next_timer` and returning the tasks directly.
    pub fn advance_timers(&mut self) -> usize {
        let now = Instant::now();
        let mut count = 0;
        for entry in &mut self.tasks {
            if let Some(deadline) = entry.deadline {
                if !entry.ready && deadline <= now {
                    entry.ready = true;
                    count += 1;
                }
            }
        }
        count
    }

    /// Returns the duration until the next timer fires, or `None` if
    /// there are no pending timers.
    ///
    /// A return value of `Duration::ZERO` (or very small) means a timer
    /// is already expired and [`advance_timers`](Self::advance_timers)
    /// should be called.
    pub fn time_to_next_timer(&self) -> Option<Duration> {
        let now = Instant::now();
        self.tasks
            .iter()
            .filter_map(|e| {
                if !e.ready {
                    e.deadline.map(|d| d.saturating_duration_since(now))
                } else {
                    None
                }
            })
            .min()
    }

    /// Returns `true` if there are any tasks (ready or not) in the queue.
    pub fn has_pending(&self) -> bool {
        !self.tasks.is_empty()
    }

    /// Returns `true` if at least one task is in the ready state.
    pub fn has_ready(&self) -> bool {
        self.tasks.iter().any(|e| e.ready)
    }

    /// Returns the number of tasks currently queued (ready or not).
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Returns `true` if the event loop has no tasks.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    // -----------------------------------------------------------------------
    // Interest tracking
    // -----------------------------------------------------------------------

    /// Register external interest — the event loop should stay alive even
    /// with no pending tasks.
    pub fn acquire_interest(&mut self) {
        self.interest.acquire();
    }

    /// Release previously registered interest.
    ///
    /// # Panics
    ///
    /// Panics if no matching [`acquire_interest`](Self::acquire_interest)
    /// was called.
    pub fn release_interest(&mut self) {
        self.interest.release();
        // Wake the driver — if interest dropped to zero and no tasks
        // remain, the loop should discover `Done` and exit.
        self.notify.notify(1);
    }

    /// Returns `true` if at least one external interest is held.
    pub fn has_interest(&self) -> bool {
        self.interest.has_interest()
    }

    /// Returns `true` if the event loop should stay alive: either there
    /// are pending tasks or external interest is held.
    pub fn is_alive(&self) -> bool {
        self.has_pending() || self.has_interest()
    }

    /// Returns an [`EventListener`] future that resolves when the event
    /// loop is notified of new readiness (a task became ready, a timer
    /// was queued, etc.).
    ///
    /// The returned listener must be `.await`ed. If the event was already
    /// notified before the listener was created, the first listener to
    /// poll will complete immediately.
    pub fn notified(&self) -> EventListener<()> {
        self.notify.listen()
    }
}

/// Run the event loop to completion asynchronously.
///
/// This is the single, platform-agnostic event loop driver. On each
/// iteration it creates a fresh [`RootScope`] from `raw_cx` (dropping it
/// before any await point), calls [`EventLoop::step`], and acts on the
/// outcome:
///
/// - [`StepOutcome::Done`] → return.
/// - [`StepOutcome::Progressed`] → immediately loop again.
/// - [`StepOutcome::Idle`] → race the next timer deadline against a
///   notification from the event loop. The `sleep` parameter abstracts
///   the platform timer: on native it is a `thread::sleep`-based future;
///   on wasm32 it is `wasi:clocks/monotonic-clock.wait-for`.
///
/// The [`CURRENT_EVENT_LOOP`] thread-local is set during each `step()`
/// call and cleared before any await.
///
/// # Safety
///
/// `raw_cx` must be a valid JSContext pointer that remains valid for the
/// lifetime of this future (i.e. the `Runtime` must not be dropped).
pub async unsafe fn run_to_completion<S, F>(
    raw_cx: *mut js::native::RawJSContext,
    event_loop: &mut EventLoop,
    sleep: S,
) where
    S: Fn(Duration) -> F,
    F: std::future::Future<Output = ()>,
{
    loop {
        // Create a fresh rooting scope each iteration — the same
        // technique JSNative trampolines use. It is dropped before any
        // await point so GC roots don't span a suspension.
        let scope = js::gc::scope::RootScope::from_current_realm(raw_cx);

        let outcome = with_event_loop(event_loop, |el| el.step(&scope));

        drop(scope);

        match outcome {
            StepOutcome::Done => return,
            StepOutcome::Progressed => continue,
            StepOutcome::Idle => {
                // Wait for either the next timer to fire or an external
                // notification (a task was signaled ready, a new timer
                // was queued, etc.).
                let timer_wait = async {
                    if let Some(wait) = event_loop.time_to_next_timer() {
                        sleep(wait).await;
                    } else {
                        // No timers — pend forever; the notified branch
                        // will wake us.
                        std::future::pending::<()>().await;
                    }
                };
                let notified = event_loop.notified();

                // Race: first one to complete wins.
                futures_lite::future::or(timer_wait, notified).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Exception handling helper
// ---------------------------------------------------------------------------

/// Handle and clear a pending JS exception.
///
/// Prints the exception to stderr and clears the pending state,
/// allowing the event loop to continue. This is called by [`EventLoop::step`]
/// when a task or microtask throws.
fn handle_and_clear_exception(scope: &Scope<'_>) {
    let e = ExnThrown::capture(scope);
    eprintln!("[event_loop] Uncaught exception: {e}");
    js::exception::clear(scope);
}

impl EventLoop {
    /// Advance the event loop by one step.
    ///
    /// A single step:
    /// 1. Asserts that the microtask queue is empty.
    /// 2. Asserts that there are no pending JS exceptions.
    /// 3. Advances timers.
    /// 4. Runs all currently-ready tasks (draining microtasks and checking
    ///    for exceptions after each).
    ///
    /// Returns a [`StepOutcome`] telling the driver what to do next.
    pub fn step(&mut self, scope: &Scope<'_>) -> StepOutcome {
        // 1. Assert that there are no pending microtasks.
        debug_assert!(
            !js::jobs::has_pending_jobs(scope),
            "Pending microtask detected"
        );

        // 2. Assert that there are no pending exceptions.
        debug_assert!(
            !js::exception::is_pending(scope),
            "Pending JS exception detected"
        );

        // 3. Advance timers.
        self.advance_timers();

        // 4. Run all ready tasks.
        let mut ran_any = false;
        while let Some((id, task)) = self.pop_ready() {
            if task.run(scope, id).is_err() {
                eprintln!("[event_loop] Task error (id={:?})", id);
                handle_and_clear_exception(scope);
            }
            ran_any = true;

            // After each task, drain microtasks — the task may have
            // resolved promises or scheduled reactions.
            run_microtasks(scope);

            if js::exception::is_pending(scope) {
                handle_and_clear_exception(scope);
            }

            // Re-advance timers in case task execution took long enough
            // for more timers to expire.
            self.advance_timers();
        }

        if ran_any {
            return StepOutcome::Progressed;
        }

        if self.is_alive() {
            StepOutcome::Idle
        } else {
            StepOutcome::Done
        }
    }

    /// Trace all live tasks for GC.
    ///
    /// Called by SpiderMonkey's GC through the extra-roots-tracer
    /// mechanism. Each task's [`Task::trace`] method is invoked so that
    /// any `Heap<*mut JSObject>` fields are properly marked.
    ///
    /// # Safety
    ///
    /// `trc` must be a valid `JSTracer` pointer provided by SpiderMonkey.
    pub unsafe fn trace(&self, trc: *mut JSTracer) {
        for entry in &self.tasks {
            entry.task.trace(trc);
        }
    }

    /// Allocate the next unique [`TaskId`].
    fn next_task_id(&mut self) -> TaskId {
        let id = TaskId(self.next_id);
        self.next_id += 1;
        id
    }
}

impl Default for EventLoop {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Run SpiderMonkey's microtask queue and clear weak references.
///
/// This should be called after running any task that may have created
/// promise reactions or other microtasks. It drains the job queue and
/// then clears the weak-reference set for the current "turn".
pub fn run_microtasks(scope: &Scope<'_>) {
    debug_assert!(
        !js::exception::is_pending(scope),
        "Cannot run microtasks with pending exception"
    );
    jobs::run_jobs(scope);
    // Weak-ref set is cleared by run_jobs.
}
