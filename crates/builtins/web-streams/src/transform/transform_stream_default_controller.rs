// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/>

use super::algorithms;
use super::transform_stream::TransformStreamImpl;
use core_runtime::{webidl_interface, webidl_methods};
use js::error::ExnThrown;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::native::Value;
use js::prelude::HandleValue;

/// <https://streams.spec.whatwg.org/#ts-default-controller-class>
#[webidl_interface(no_ctor)]
pub struct TransformStreamDefaultController {
    /// <https://streams.spec.whatwg.org/#transformstreamdefaultcontroller-cancelalgorithm>
    /// A promise-returning algorithm, taking one argument (the reason for cancellation), which
    /// communicates a requested cancellation to the transformer
    pub(crate) cancel_algorithm: Heap<Value>,
    /// <https://streams.spec.whatwg.org/#transformstreamdefaultcontroller-finishpromise>
    /// A promise which resolves on completion of either the [[cancelAlgorithm]] or the
    /// [[flushAlgorithm]]. If this field is unpopulated (that is, undefined), then neither of those
    /// algorithms have been invoked yet
    pub(crate) finish_promise: Option<Heap<js::promise::Promise>>,
    /// <https://streams.spec.whatwg.org/#transformstreamdefaultcontroller-flushalgorithm>
    /// A promise-returning algorithm which communicates a requested close to the transformer
    pub(crate) flush_algorithm: Heap<Value>,
    /// The `this` value the transform/flush/cancel algorithms are invoked with (the
    /// transformer object, or `undefined` for native algorithms). Not a spec slot.
    pub(crate) algorithm_receiver: Heap<Value>,
    /// <https://streams.spec.whatwg.org/#transformstreamdefaultcontroller-stream>
    /// The TransformStream instance controlled
    ///
    /// `Option`: set by `SetUpTransformStreamDefaultController` after creating.
    pub(crate) stream: Option<Heap<TransformStreamImpl>>,
    /// <https://streams.spec.whatwg.org/#transformstreamdefaultcontroller-transformalgorithm>
    /// A promise-returning algorithm, taking one argument (the chunk to transform), which requests
    /// the transformer perform its transformation
    pub(crate) transform_algorithm: Heap<Value>,
    /// The transform-rejection callback (`TransformStreamDefaultControllerPerformTransform`
    /// step 2; payload = this controller), created on the first transform and
    /// reused for every subsequent chunk. `None` until the first transform.
    pub(crate) transform_rejected_fn: Option<Heap<js::function::Function>>,
    /// The chunk a backpressured sink write parked until the backpressure-change
    /// promise fulfills, and the fulfillment callback that consumes it
    /// (`TransformStreamDefaultSinkWriteAlgorithm` step 3; payload = this
    /// controller, `None` until first use).
    pub(crate) pending_write_chunk: Heap<Value>,
    pub(crate) write_after_backpressure_fn: Option<Heap<js::function::Function>>,
}

#[webidl_methods]
impl TransformStreamDefaultController {
    /// <https://streams.spec.whatwg.org/#dom-transformstreamdefaultcontroller-constructor>
    #[constructor]
    fn new() -> Self {
        TransformStreamDefaultControllerImpl::default()
    }

    /// <https://streams.spec.whatwg.org/#ts-default-controller-desired-size>
    #[getter]
    fn desired_size(&self, scope: &Scope<'_>) -> Option<f64> {
        // Step 1: Let _readableController_ be `this`.`[[stream]]`.`[[readable]]`.`[[controller]]`.
        // Step 2: Return ! `DefaultControllerGetDesiredSize`(_readableController_).
        algorithms::transform_controller_readable_desired_size(scope, self)
    }

    /// <https://streams.spec.whatwg.org/#ts-default-controller-enqueue>
    #[method]
    fn enqueue(&self, scope: &Scope<'_>, chunk: Option<HandleValue<'_>>) -> Result<(), ExnThrown> {
        // Step 1: Perform ? `TransformStreamDefaultControllerEnqueue`(`this`, _chunk_).
        let chunk = chunk.unwrap_or_else(|| HandleValue::undefined());
        algorithms::transform_stream_default_controller_enqueue(scope, self, chunk)
    }

    /// <https://streams.spec.whatwg.org/#ts-default-controller-error>
    #[method]
    fn error(&self, scope: &Scope<'_>, reason: Option<HandleValue<'_>>) -> Result<(), ExnThrown> {
        // Step 1: Perform ? `TransformStreamDefaultControllerError`(`this`, _e_).
        let reason = reason.unwrap_or_else(|| HandleValue::undefined());
        algorithms::transform_stream_default_controller_error(scope, self, reason);
        Ok(())
    }

    /// <https://streams.spec.whatwg.org/#ts-default-controller-terminate>
    #[method]
    fn terminate(&self, scope: &Scope<'_>) -> Result<(), ExnThrown> {
        // Step 1: Perform ? `TransformStreamDefaultControllerTerminate`(`this`).
        algorithms::transform_stream_default_controller_terminate(scope, self);
        Ok(())
    }
}
