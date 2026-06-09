// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/>

use core_runtime::webidl_dictionary;
use js::{prelude::HandleValue, Object};

/// <https://streams.spec.whatwg.org/#dictdef-transformer>
#[webidl_dictionary]
pub struct Transformer<'a> {
    pub start: Option<StartCallback<'a>>,
    pub transform: Option<TransformCallback<'a>>,
    pub flush: Option<FlushCallback<'a>>,
    pub cancel: Option<CancelCallback<'a>>,
    pub readable_type: Option<HandleValue<'a>>,
    pub writable_type: Option<HandleValue<'a>>,
}

/// WebIDL callback `TransformerStartCallback`: (controller: TransformStreamDefaultController<'_>) -> HandleValue<'_>
pub type StartCallback<'s> = Object<'s>;

/// WebIDL callback `TransformerFlushCallback`: (controller: TransformStreamDefaultController<'_>) -> Promise<'_>
pub type FlushCallback<'s> = Object<'s>;

/// WebIDL callback `TransformerTransformCallback`: (chunk: HandleValue<'_>, controller: TransformStreamDefaultController<'_>) -> Promise<'_>
pub type TransformCallback<'s> = Object<'s>;

/// WebIDL callback `TransformerCancelCallback`: (reason: HandleValue<'_>) -> Promise<'_>
pub type CancelCallback<'s> = Object<'s>;
