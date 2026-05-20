// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

mod text_decoder;
mod text_encoder;

use js::gc::scope::Scope;
use js::Object;

pub fn add_to_global(scope: &Scope<'_>, global: Object<'_>) {
    text_encoder::TextEncoder::add_to_global(scope, global);
    text_decoder::TextDecoder::add_to_global(scope, global);
}
