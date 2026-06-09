// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/>

use core_runtime::webidl_dictionary;
use js::prelude::HandleValue;

/// <https://streams.spec.whatwg.org/#dictdef-ReadResult>
#[webidl_dictionary]
pub struct ReadResult<'a> {
    pub value: Option<HandleValue<'a>>,
    pub done: Option<bool>,
}
