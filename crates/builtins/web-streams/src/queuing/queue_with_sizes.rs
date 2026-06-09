// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Queue-with-sizes support, shared by the default readable/writable stream
//! controllers.
//!
//! <https://streams.spec.whatwg.org/#queue-with-sizes>
//!
//! The spec models these containers as having `[[queue]]` (a list of
//! "value-with-size" records) and `[[queueTotalSize]]` (a number) internal
//! slots, and defines the queue operations (`EnqueueValueWithSize`,
//! `DequeueValue`, `PeekQueueValue`, `ResetQueue`) generically over any such
//! container. The [`QueueWithSizes`] trait captures that shared shape so the
//! operations in [`crate::algorithms`] can run against either controller.

use std::collections::VecDeque;

use core_runtime::Traceable;
use js::gc::handle::Heap;
use js::native::Value;

/// A single `value-with-size` entry in a queue-with-sizes.
///
/// <https://streams.spec.whatwg.org/#value-with-size>
#[js::must_root]
#[derive(Traceable, Default)]
pub struct ValueWithSize {
    /// The enqueued chunk.
    pub value: Heap<Value>,
    /// The chunk's size, as computed by the stream's size algorithm. Already
    /// `ToNumber`-coerced (the size algorithm returns an `unrestricted double`),
    /// so it is a plain `f64` rather than a JS value.
    #[no_trace]
    pub size: f64,
    /// Whether this entry is the writable stream's `close sentinel` (enqueued by
    /// `WritableStreamDefaultControllerClose` with size 0). The readable
    /// controllers never set this.
    ///
    /// <https://streams.spec.whatwg.org/#writablestreamdefaultcontroller-close-sentinel>
    #[no_trace]
    pub is_close_sentinel: bool,
}

/// A container with `[[queue]]` and `[[queueTotalSize]]` internal slots, as
/// required by the § 8.1 queue-with-sizes operations.
pub trait QueueWithSizes {
    fn queue(&self) -> &VecDeque<ValueWithSize>;
    fn queue_mut(&mut self) -> &mut VecDeque<ValueWithSize>;
    fn queue_total_size(&self) -> f64;
    fn set_queue_total_size(&mut self, size: f64);
}
