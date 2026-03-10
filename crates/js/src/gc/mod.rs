// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Garbage collection control, configuration, and rooted handle types.
//!
//! This module provides safe wrappers for controlling SpiderMonkey's garbage
//! collector, including triggering collections, configuring GC parameters,
//! and managing incremental GC.
//!
//! The [`handle`] submodule defines [`Stack`](handle::Stack) and
//! [`Heap`](handle::Heap) — the typed wrappers for scope-rooted and
//! heap-traced JS object references.

pub mod handle;
pub mod pool;
pub mod scope;

use mozjs::context::JSContext;
use mozjs::jsapi::{JSGCParamKey, SliceBudget, Zone};
use mozjs::rust::wrappers2;
use scope::Scope;

// Re-export types that appear in public function signatures so callers
// can name them without depending on `mozjs` directly.
pub use mozjs::jsapi::{GCOptions, GCReason};

#[cfg(feature = "debugmozjs")]
pub use mozjs::jsapi::SetGCZeal;

pub fn init(cx: &mut JSContext) {
    pool::init_pool(cx);
}

pub fn shutdown() {
    pool::shutdown();
}

/// Trigger a full, non-incremental garbage collection.
pub fn gc(scope: &Scope<'_>, reason: GCReason) {
    unsafe { wrappers2::JS_GC(scope.cx_mut(), reason) }
}

/// Hint that now would be a good time for a GC, if one is needed.
pub fn maybe_gc(scope: &Scope<'_>) {
    unsafe { wrappers2::JS_MaybeGC(scope.cx_mut()) }
}

/// Prepare all zones for a full GC.
pub fn prepare_for_full_gc(scope: &Scope<'_>) {
    unsafe { wrappers2::PrepareForFullGC(scope.cx()) }
}

/// Prepare for an incremental GC (only zones that are already scheduled).
pub fn prepare_for_incremental_gc(scope: &Scope<'_>) {
    unsafe { wrappers2::PrepareForIncrementalGC(scope.cx()) }
}

/// Prepare a specific zone for GC.
///
/// # Safety
///
/// `zone` must be a valid pointer to a `Zone`.
pub unsafe fn prepare_zone_for_gc(scope: &Scope<'_>, zone: *mut Zone) {
    wrappers2::PrepareZoneForGC(scope.cx(), zone)
}

/// Skip a specific zone during the next GC.
///
/// # Safety
///
/// `zone` must be a valid pointer to a `Zone`.
pub unsafe fn skip_zone_for_gc(scope: &Scope<'_>, zone: *mut Zone) {
    wrappers2::SkipZoneForGC(scope.cx(), zone)
}

/// Check whether a GC is scheduled.
pub fn is_gc_scheduled(scope: &Scope<'_>) -> bool {
    unsafe { wrappers2::IsGCScheduled(scope.cx()) }
}

/// Perform a full non-incremental GC with the given options.
pub fn non_incremental_gc(scope: &Scope<'_>, options: GCOptions, reason: GCReason) {
    unsafe { wrappers2::NonIncrementalGC(scope.cx_mut(), options, reason) }
}

/// Start an incremental GC.
///
/// # Safety
///
/// `budget` must be a valid pointer to a `SliceBudget`.
pub unsafe fn start_incremental_gc(
    scope: &Scope<'_>,
    options: GCOptions,
    reason: GCReason,
    budget: *const SliceBudget,
) {
    wrappers2::StartIncrementalGC(scope.cx_mut(), options, reason, budget)
}

/// Perform one slice of an incremental GC.
///
/// # Safety
///
/// `budget` must be a valid pointer to a `SliceBudget`.
pub unsafe fn incremental_gc_slice(
    scope: &Scope<'_>,
    reason: GCReason,
    budget: *const SliceBudget,
) {
    wrappers2::IncrementalGCSlice(scope.cx_mut(), reason, budget)
}

/// Check whether incremental GC has foreground work pending.
pub fn incremental_gc_has_foreground_work(scope: &Scope<'_>) -> bool {
    unsafe { wrappers2::IncrementalGCHasForegroundWork(scope.cx()) }
}

/// Finish an in-progress incremental GC.
pub fn finish_incremental_gc(scope: &Scope<'_>, reason: GCReason) {
    unsafe { wrappers2::FinishIncrementalGC(scope.cx_mut(), reason) }
}

/// Abort an in-progress incremental GC.
pub fn abort_incremental_gc(scope: &Scope<'_>) {
    unsafe { wrappers2::AbortIncrementalGC(scope.cx_mut()) }
}

/// Check whether incremental GC is enabled.
pub fn is_incremental_gc_enabled(scope: &Scope<'_>) -> bool {
    unsafe { wrappers2::IsIncrementalGCEnabled(scope.cx()) }
}

/// Check whether an incremental GC is currently in progress.
pub fn is_incremental_gc_in_progress(scope: &Scope<'_>) -> bool {
    unsafe { wrappers2::IsIncrementalGCInProgress(scope.cx()) }
}

/// Set a GC parameter.
pub fn set_parameter(scope: &Scope<'_>, key: JSGCParamKey, value: u32) {
    unsafe { wrappers2::JS_SetGCParameter(scope.cx_mut(), key, value) }
}

/// Reset a GC parameter to its default value.
pub fn reset_parameter(scope: &Scope<'_>, key: JSGCParamKey) {
    unsafe { wrappers2::JS_ResetGCParameter(scope.cx_mut(), key) }
}

/// Get the current value of a GC parameter.
pub fn get_parameter(scope: &Scope<'_>, key: JSGCParamKey) -> u32 {
    unsafe { wrappers2::JS_GetGCParameter(scope.cx(), key) }
}

/// Set GC parameters based on available memory (in MB).
pub fn set_parameters_based_on_available_memory(scope: &Scope<'_>, avail_mem_mb: u32) {
    unsafe { wrappers2::JS_SetGCParametersBasedOnAvailableMemory(scope.cx_mut(), avail_mem_mb) }
}

/// Get the total GC heap usage in bytes.
pub fn get_heap_usage(scope: &Scope<'_>) -> u64 {
    unsafe { wrappers2::GetGCHeapUsage(scope.cx()) }
}

/// Notify the GC that the application is in a low-memory state.
pub fn set_low_memory_state(scope: &Scope<'_>, new_state: bool) {
    unsafe { wrappers2::SetLowMemoryState(scope.cx(), new_state) }
}

// ---------------------------------------------------------------------------
// Extra GC roots tracing
// ---------------------------------------------------------------------------

/// Register a callback to trace additional GC roots.
///
/// The callback will be invoked during garbage collection to trace any
/// `Heap<T>` values that are not otherwise reachable from the GC graph.
///
/// # Safety
///
/// `trace_op` must be a valid function pointer. `data` is passed through
/// to the callback as-is and must stay valid for the lifetime of the callback.
pub unsafe fn add_extra_gc_roots_tracer(
    cx: &mut JSContext,
    trace_op: mozjs::jsapi::JSTraceDataOp,
    data: *mut std::os::raw::c_void,
) {
    wrappers2::JS_AddExtraGCRootsTracer(cx, trace_op, data);
}

/// Remove a previously registered extra GC roots tracer callback.
///
/// The `trace_op` and `data` must match a previous call to
/// [`add_extra_gc_roots_tracer`].
///
/// # Safety
///
/// `trace_op` and `data` must match a prior registration.
pub unsafe fn remove_extra_gc_roots_tracer(
    cx: &JSContext,
    trace_op: mozjs::jsapi::JSTraceDataOp,
    data: *mut std::os::raw::c_void,
) {
    wrappers2::JS_RemoveExtraGCRootsTracer(cx, trace_op, data);
}
