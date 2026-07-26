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
    // WebIDL `[EnforceRange] unsigned long long`. Held as `f64` so the sign and
    // non-finiteness survive the dictionary conversion (the macro converts
    // integers with wrapping semantics); `[EnforceRange]` is applied in
    // `set_up_readable_byte_stream_controller_from_underlying_source`.
    pub auto_allocate_chunk_size: Option<f64>,
}

/// WebIDL callback `UnderlyingSourceStartCallback`: (controller: ReadableStreamController) -> HandleValue<'_>
pub type StartCallback<'s> = Object<'s>;

/// WebIDL callback `UnderlyingSourcePullCallback`: (controller: ReadableStreamController) -> Promise<'_>
pub type PullCallback<'s> = Object<'s>;

/// WebIDL callback `UnderlyingSourceCancelCallback`: (reason: Option<HandleValue<'_>>) -> Promise<'_>
pub type CancelCallback<'s> = Object<'s>;
