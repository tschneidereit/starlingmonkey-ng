// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/>

use core_runtime::webidl_dictionary;
use js::conversion::EnforceRange;
use web_globals::signals::AbortSignal;

use super::enums::ReaderMode;

/// <https://streams.spec.whatwg.org/#dictdef-BYOBReaderReadOptions>
#[webidl_dictionary]
pub struct BYOBReaderReadOptions {
    #[webidl(default = EnforceRange(1))]
    pub min: EnforceRange<u64>,
}

/// <https://streams.spec.whatwg.org/#dictdef-readablestreamgetreaderoptions>
#[webidl_dictionary]
pub struct ReadableStreamGetReaderOptions {
    pub mode: Option<ReaderMode>,
}

/// <https://streams.spec.whatwg.org/#dictdef-readablestreamiteratoroptions>
#[webidl_dictionary]
pub struct ReadableStreamIteratorOptions {
    #[webidl(default = false)]
    pub prevent_cancel: bool,
}

/// <https://streams.spec.whatwg.org/#dictdef-streampipeoptions>
#[webidl_dictionary]
pub struct StreamPipeOptions<'a> {
    #[webidl(default = false)]
    pub prevent_close: bool,
    #[webidl(default = false)]
    pub prevent_abort: bool,
    #[webidl(default = false)]
    pub prevent_cancel: bool,
    pub signal: Option<AbortSignal<'a>>,
}
