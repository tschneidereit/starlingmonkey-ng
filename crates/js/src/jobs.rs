// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Job queue (microtask) management.
//!
//! SpiderMonkey requires a job queue to execute promise continuations and other
//! microtasks. This module provides wrappers for configuring and draining the
//! queue.
//!
//! # Quick Start
//!
//! ```ignore
//! // Use SpiderMonkey's built-in job queue (simplest option):
//! jobs::use_internal_job_queues(cx)?;
//!
//! // After evaluating JS that creates promises, drain the queue:
//! jobs::run_jobs(cx);
//! ```

use crate::gc::scope::Scope;
use mozjs::rust::wrappers2;

use super::error::ExnThrown;

/// Enable SpiderMonkey's built-in internal job queue.
///
/// This is the simplest way to get Promise resolution working. Must be called
/// before any promises are created. Call [`run_jobs`] after evaluation to
/// drain the queue.
pub fn use_internal_job_queues(scope: &Scope<'_>) -> Result<(), ExnThrown> {
    let ok = unsafe { wrappers2::UseInternalJobQueues(scope.cx()) };
    ExnThrown::check(ok)
}

/// Drain the job queue, executing all pending microtasks.
///
/// This runs all queued promise reactions and other microtasks until the
/// queue is empty.
pub fn run_jobs(scope: &Scope<'_>) {
    unsafe { wrappers2::RunJobs(scope.cx_mut()) }
}

/// Stop draining the job queue.
///
/// After calling this, [`run_jobs`] becomes a no-op until the queue is
/// re-enabled.
pub fn stop_draining(scope: &Scope<'_>) {
    unsafe { wrappers2::StopDrainingJobQueue(scope.cx()) }
}

/// Clear the weak references kept alive for the current turn.
///
/// This implements the host hook for WeakRef liveness semantics. Should be
/// called between "turns" (e.g. between event loop iterations).
pub fn clear_kept_objects(scope: &Scope<'_>) {
    unsafe { wrappers2::ClearKeptObjects(scope.cx()) }
}
