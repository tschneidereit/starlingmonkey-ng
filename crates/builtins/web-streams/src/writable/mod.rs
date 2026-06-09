// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/#ws-model>

pub(crate) mod algorithms;

pub mod default_controller;
pub mod default_writer;
pub mod underlying_sink;
pub mod writable_stream;

pub use default_controller::WritableStreamDefaultController;
pub use default_writer::WritableStreamDefaultWriter;
pub use writable_stream::WritableStream;
