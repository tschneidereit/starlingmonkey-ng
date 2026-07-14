// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/#rs-model>

mod algorithms;

pub(crate) mod readable_writable_pair;
pub(crate) mod transform_stream;
pub(crate) mod transform_stream_default_controller;
mod transformer;

pub use transform_stream::{TransformStream, TransformStreamImpl};
