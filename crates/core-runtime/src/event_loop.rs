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
//!    queuing, cancellation, readiness signaling, timer advancement, and GC
//!    tracing of all live tasks.
//!
//! 3. **Platform drivers** — swap in a blocking loop for native targets
//!    ([`native`]) or a callback-driven model for WASIp3 ([`wasi`]) without
//!    changing any task code.
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

pub mod native;
pub mod promise;
pub mod timer;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use js::native::JSTracer;

use js::gc::scope::Scope;
use js::jobs;

// ---------------------------------------------------------------------------
// TaskId
// ---------------------------------------------------------------------------

/// Opaque identifier for a queued task.
///
/// Task IDs are unique within a single [`EventLoop`] instance and are never
/// reused (the internal counter is a `u64` — wrapping is not a concern in
/// practice).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TaskId(u64);

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TaskId({})", self.0)
    }
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
}

impl EventLoop {
    /// Create a new, empty event loop.
    pub fn new() -> Self {
        Self {
            next_id: 0,
            tasks: Vec::new(),
            cancelled_while_running: HashSet::new(),
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
    }

    /// Mark a queued task as ready to run.
    ///
    /// Has no effect if the task ID is not found (the task may have
    /// already been cancelled or run).
    pub fn signal_ready(&mut self, id: TaskId) {
        if let Some(entry) = self.tasks.iter_mut().find(|e| e.id == id) {
            entry.ready = true;
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
    jobs::run_jobs(scope);
    // TODO: when should this run? After microtasks, or only after full turns?
    jobs::clear_kept_objects(scope);
}
