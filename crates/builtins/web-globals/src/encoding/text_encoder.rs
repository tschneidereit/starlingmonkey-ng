// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://encoding.spec.whatwg.org/#textencoder>

use core_runtime::{webidl_dictionary, webidl_interface, webidl_methods};
use js::conversion::{ConversionError, ToJSVal};
use js::error::ExnThrown;
use js::gc::scope::Scope;
use js::prelude::HandleValue;
use js::Uint8Array;

/// <https://encoding.spec.whatwg.org/#textencoder>
///
/// The TextEncoder interface always encodes to UTF-8.
#[webidl_interface]
pub struct TextEncoder {}

#[webidl_methods]
impl TextEncoder<'_> {
    /// <https://encoding.spec.whatwg.org/#dom-textencoder>
    #[constructor]
    fn new() -> Self {
        // Step 1: (No-op -- TextEncoder has no internal state.)
        TextEncoderImpl {}
    }

    /// <https://encoding.spec.whatwg.org/#dom-textencoder-encoding>
    #[getter]
    fn encoding(&self) -> String {
        // Step 1: Return "utf-8".
        "utf-8".into()
    }

    /// <https://encoding.spec.whatwg.org/#dom-textencoder-encode>
    #[method]
    fn encode<'r>(
        &self,
        scope: &'r Scope<'_>,
        input: Option<String>,
    ) -> Result<Uint8Array<'r>, ExnThrown> {
        // Rust strings are already valid UTF-8; the SpiderMonkey-side conversion
        // has replaced any lone surrogates with U+FFFD, so the bytes here are
        // exactly what the spec calls for.
        let input = input.as_deref().unwrap_or("");
        Uint8Array::with_data(scope, input.as_bytes())
    }

    /// <https://encoding.spec.whatwg.org/#dom-textencoder-encodeinto>
    #[method]
    fn encode_into(
        &self,
        source: String,
        destination: Uint8Array,
    ) -> Result<TextEncoderEncodeIntoResult, ExnThrown> {
        // SAFETY: `destination` is rooted; the slice is only borrowed for the
        // `copy_from_slice` below, which is pure CPU work and cannot trigger GC
        // or detach the underlying buffer.
        let dest_buf = unsafe { destination.as_mut_slice() };
        let dest_len = dest_buf.len();

        // The source has already been converted to UTF-8, so encoding boils
        // down to finding the longest prefix that fits in the destination on
        // a char boundary, then a single slice copy. `read` counts UTF-16 code
        // units (1 for BMP scalars, 2 for supplementary).
        let mut written = 0usize;
        let mut read = 0u64;
        for (idx, ch) in source.char_indices() {
            let next = idx + ch.len_utf8();
            if next > dest_len {
                break;
            }
            written = next;
            read += ch.len_utf16() as u64;
        }
        dest_buf[..written].copy_from_slice(&source.as_bytes()[..written]);

        Ok(TextEncoderEncodeIntoResult {
            read,
            written: written as u64,
        })
    }
}

/// <https://encoding.spec.whatwg.org/#dictdef-textencoderencodeintoresult>
#[webidl_dictionary]
pub struct TextEncoderEncodeIntoResult {
    pub read: u64,
    pub written: u64,
}

impl<'s> ToJSVal<'s> for TextEncoderEncodeIntoResult {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        let obj = js::Object::new_plain(scope)?;
        obj.set_property(scope, c"read", self.read)?;
        obj.set_property(scope, c"written", self.written)?;
        obj.to_jsval(scope)
    }
}
