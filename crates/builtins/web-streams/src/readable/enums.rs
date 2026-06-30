// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! WebIDL enumerations from <https://streams.spec.whatwg.org/>

js::webidl_enum! {
    /// WebIDL enum `ReadableStreamReaderMode`
    pub enum ReaderMode {
        Byob => "byob",
    }
}

js::webidl_enum! {
    /// WebIDL enum `ReadableStreamType`
    pub enum ReadableStreamType {
        Bytes => "bytes",
    }
}
