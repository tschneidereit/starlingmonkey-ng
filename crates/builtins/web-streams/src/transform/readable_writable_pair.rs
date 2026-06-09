// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/>

use crate::readable::ReadableStream;
use crate::writable::WritableStream;
use core_runtime::webidl_dictionary;

/// <https://streams.spec.whatwg.org/#dictdef-readablewritablepair>
#[webidl_dictionary]
pub struct ReadableWritablePair<'a> {
    pub readable: ReadableStream<'a>,
    pub writable: WritableStream<'a>,
}
