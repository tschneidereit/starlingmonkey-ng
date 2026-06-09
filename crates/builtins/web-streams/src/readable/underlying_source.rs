// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/>

use super::enums::ReadableStreamType;
use core_runtime::webidl_dictionary;
use js::Object;

/// <https://streams.spec.whatwg.org/#dictdef-underlyingsource>
#[webidl_dictionary]
pub struct UnderlyingSource<'a> {
    pub start: Option<StartCallback<'a>>,
    pub pull: Option<PullCallback<'a>>,
    pub cancel: Option<CancelCallback<'a>>,
    pub r#type: Option<ReadableStreamType>,
    pub auto_allocate_chunk_size: Option<u64>,
}

/// WebIDL callback `UnderlyingSourceStartCallback`: (controller: ReadableStreamController<'_>) -> HandleValue<'_>
pub type StartCallback<'s> = Object<'s>;

/// WebIDL callback `UnderlyingSourcePullCallback`: (controller: ReadableStreamController<'_>) -> Promise<'_>
pub type PullCallback<'s> = Object<'s>;

/// WebIDL callback `UnderlyingSourceCancelCallback`: (reason: Option<HandleValue<'_>>) -> Promise<'_>
pub type CancelCallback<'s> = Object<'s>;
