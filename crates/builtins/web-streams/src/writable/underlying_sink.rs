// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/>

use core_runtime::webidl_dictionary;
use js::{prelude::HandleValue, Object};

/// <https://streams.spec.whatwg.org/#dictdef-underlyingsink>
#[webidl_dictionary]
pub struct UnderlyingSink<'a> {
    pub start: Option<StartCallback<'a>>,
    pub write: Option<WriteCallback<'a>>,
    pub close: Option<CloseCallback<'a>>,
    pub abort: Option<AbortCallback<'a>>,
    pub r#type: Option<HandleValue<'a>>,
}

/// WebIDL callback `UnderlyingSinkStartCallback`: (controller: WritableStreamDefaultController<'_>) -> HandleValue<'_>
pub type StartCallback<'s> = Object<'s>;

/// WebIDL callback `UnderlyingSinkWriteCallback`: (chunk: HandleValue<'_>, controller: WritableStreamDefaultController<'_>) -> Promise<'_>
pub type WriteCallback<'s> = Object<'s>;

/// WebIDL callback `UnderlyingSinkCloseCallback`: () -> Promise<'_>
pub type CloseCallback<'s> = Object<'s>;

/// WebIDL callback `UnderlyingSinkAbortCallback`: (reason: Option<HandleValue<'_>>) -> Promise<'_>
pub type AbortCallback<'s> = Object<'s>;
