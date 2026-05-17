// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Spawner trait for driving async futures to completion.
//!
//! The [`Spawner`] trait decouples the event loop from any specific async
//! runtime (tokio, async-std, WASIp3 host, etc.). When a JS method returns
//! a [`JSPromise`](js::promise::JSPromise), the generated glue code calls
//! [`__spawn_promise`](js::promise::__spawn_promise) which queues the future.
//! The embedding's spawner implementation picks up these futures and drives
//! them — when a future completes, the spawner queues a ready
//! [`PromiseTask`](super::promise::PromiseTask) on the event loop so the
//! next `step()` settles the JS promise.
//!
//! ## Design
//!
//! `core-runtime` defines the trait; embeddings provide implementations:
//!
//! - **Native (CLI/starling)**: a tokio-backed spawner that runs futures on
//!   the tokio runtime and wakes the event loop via `signal_ready()`.
//! - **WASIp3**: a spawner that maps future I/O to waitable handles so the
//!   host runtime drives completion.

use std::future::Future;
use std::pin::Pin;

use js::promise::PromiseOutcome;

use super::TaskId;

/// A boxed future that produces a [`PromiseOutcome`] when complete.
pub type PromiseFuture = Pin<Box<dyn Future<Output = PromiseOutcome> + 'static>>;

/// Trait for spawning async futures and driving them to completion.
///
/// Implementations are responsible for:
/// 1. Polling the future to completion (using whatever async mechanism
///    is available on the platform).
/// 2. When the future resolves, calling the provided `on_complete` callback
///    to queue the result back onto the event loop.
///
/// The `task_id` parameter is the event loop's task ID for the associated
/// promise task — the spawner (or its completion callback) uses this to
/// signal readiness via `EventLoop::signal_ready()`.
pub trait Spawner {
    /// Spawn a future that will produce a [`PromiseOutcome`].
    ///
    /// - `task_id`: the event loop task ID associated with the promise.
    /// - `future`: the async computation to drive.
    ///
    /// When the future completes, the spawner must arrange for the
    /// outcome to be delivered to the event loop (typically by updating
    /// the task's outcome and calling `signal_ready(task_id)`).
    fn spawn(&self, task_id: TaskId, future: PromiseFuture);
}

/// A no-op spawner that panics if any future is spawned.
///
/// Useful as a placeholder when no async runtime is available (e.g. in
/// tests that don't use promises, or in sync-only embeddings).
pub struct NoopSpawner;

impl Spawner for NoopSpawner {
    fn spawn(&self, _task_id: TaskId, _future: PromiseFuture) {
        panic!("NoopSpawner: no async runtime available to drive futures");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A test spawner that records spawned task IDs without actually
    /// driving the futures.
    struct RecordingSpawner {
        spawned: Rc<RefCell<Vec<TaskId>>>,
    }

    impl Spawner for RecordingSpawner {
        fn spawn(&self, task_id: TaskId, _future: PromiseFuture) {
            self.spawned.borrow_mut().push(task_id);
        }
    }

    #[test]
    fn recording_spawner_captures_task_ids() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let spawner = RecordingSpawner {
            spawned: log.clone(),
        };

        let future = Box::pin(async { PromiseOutcome::Reject("test".into()) });
        spawner.spawn(TaskId::from_raw(42), future);

        let future2 = Box::pin(async { PromiseOutcome::Reject("test2".into()) });
        spawner.spawn(TaskId::from_raw(99), future2);

        assert_eq!(
            *log.borrow(),
            vec![TaskId::from_raw(42), TaskId::from_raw(99)]
        );
    }

    #[test]
    #[should_panic(expected = "NoopSpawner")]
    fn noop_spawner_panics() {
        let spawner = NoopSpawner;
        let future = Box::pin(async { PromiseOutcome::Reject("boom".into()) });
        spawner.spawn(TaskId::from_raw(1), future);
    }
}
