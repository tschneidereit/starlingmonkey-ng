// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/>

use core_runtime::webidl_dictionary;
use web_globals::signals::AbortSignal;

use super::enums::ReaderMode;

/// <https://streams.spec.whatwg.org/#dictdef-BYOBReaderReadOptions>
#[webidl_dictionary]
pub struct BYOBReaderReadOptions {
    /// WebIDL declares `min` as `[EnforceRange] unsigned long long` (default 1).
    /// It is captured here as an `f64` rather than an integer so the sign and
    /// non-finiteness survive: the `[EnforceRange]` conversion (which rejects a
    /// negative or non-finite value with a `TypeError`) is applied in
    /// `BYOBReader.read`, where the failure can surface as a
    /// rejected promise per WebIDL §3.7.7 ("Operations").
    #[webidl(default = 1.0)]
    pub min: f64,
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
