// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://encoding.spec.whatwg.org/#textdecoder>

use core_runtime::{webidl_dictionary, webidl_interface, webidl_methods, webidl_union};
use js::error::{ExnThrown, RangeError};
use js::gc::scope::Scope;
use js::{ArrayBuffer, ArrayBufferView};
use crate::decoder_common::new_decoder;

use crate::decoder_common::TextDecoderOptions;

/// <https://encoding.spec.whatwg.org/#textdecoder>
#[webidl_interface]
pub struct TextDecoder {
    /// The encoding label, lowercased (e.g. "utf-8", "windows-1252").
    encoding_name: String,
    /// The encoding_rs encoding object (always `Some` after construction).
    #[no_trace]
    encoding: Option<&'static encoding_rs::Encoding>,
    /// <https://encoding.spec.whatwg.org/#textdecoder-error-mode>
    fatal: bool,
    /// <https://encoding.spec.whatwg.org/#textdecoder-ignore-bom>
    ignore_bom: bool,
    /// <https://encoding.spec.whatwg.org/#textdecoder-do-not-flush-flag>
    do_not_flush: bool,
    /// The streaming decoder state, stored as an Option so we can replace it
    /// when resetting.
    #[no_trace]
    decoder: Option<encoding_rs::Decoder>,
}

#[webidl_methods]
impl TextDecoder<'_> {
    /// <https://encoding.spec.whatwg.org/#dom-textdecoder>
    #[constructor]
    fn new(
        &self,
        label: Option<String>,
        options: Option<TextDecoderOptions>,
    ) -> Result<(), RangeError> {
        let label = label.as_deref().unwrap_or("utf-8");
        let options = options.unwrap_or_default();

        // Steps 1-2: Look up the encoding, rejecting failure and "replacement".
        let encoding = encoding_rs::Encoding::for_label_no_replacement(label.as_bytes())
            .ok_or_else(|| {
                RangeError(format!(
                    "The encoding label provided ('{label}') is invalid."
                ))
            })?;

        let mut data = self.data_mut();
        data.encoding_name = encoding.name().to_ascii_lowercase();
        data.encoding = Some(encoding);
        data.fatal = options.fatal;
        data.ignore_bom = options.ignore_bom;
        data.do_not_flush = false;
        data.decoder = Some(new_decoder(encoding, options.ignore_bom));

        Ok(())
    }

    /// <https://encoding.spec.whatwg.org/#dom-textdecoder-encoding>
    #[getter]
    fn encoding(&self) -> String {
        // Step 1: Return this's encoding's name, ASCII lowercased.
        self.data().encoding_name.clone()
    }

    /// <https://encoding.spec.whatwg.org/#dom-textdecoder-fatal>
    #[getter(name = "fatal")]
    fn get_fatal(&self) -> bool {
        // Step 1: Return true if this's error mode is "fatal"; otherwise false.
        self.data().fatal
    }

    /// <https://encoding.spec.whatwg.org/#dom-textdecoder-ignorebom>
    #[getter(name = "ignoreBOM")]
    fn ignore_bom(&self) -> bool {
        // Step 1: Return this's ignore BOM.
        self.data().ignore_bom
    }

    /// <https://encoding.spec.whatwg.org/#dom-textdecoder-decode>
    #[method]
    fn decode(
        &self,
        scope: &Scope<'_>,
        input: Option<ArrayBufferViewOrArrayBuffer<'_>>,
        options: Option<TextDecodeOptions>,
    ) -> Result<String, ExnThrown> {
        let stream = options.is_some_and(|o| o.stream);
        let last = !stream;

        let mut data = self.data_mut();
        let encoding = data
            .encoding
            .expect("encoding must be set after construction");
        let fatal = data.fatal;
        let ignore_bom = data.ignore_bom;

        // Step 1: If do-not-flush is false, replace the decoder so the new
        // decode starts fresh; otherwise carry over state from the prior call.
        if !data.do_not_flush {
            data.decoder = Some(new_decoder(encoding, ignore_bom));
        }
        data.do_not_flush = stream;
        let decoder = data.decoder.as_mut().expect("decoder must be initialized");

        // SAFETY: `input` is rooted via the Uint8Array handle and the slice
        // is only borrowed for the duration of the decode call below, which
        // is pure CPU work in `encoding_rs` and cannot trigger GC or detach
        // the underlying buffer.
        let bytes: &[u8] = match input {
            Some(ArrayBufferViewOrArrayBuffer::Buffer(input)) => unsafe { input.bytes() },
            Some(ArrayBufferViewOrArrayBuffer::View(input)) => unsafe { input.bytes() },
            None => &[],
        };

        if fatal {
            let max_len = decoder
                .max_utf8_buffer_length_without_replacement(bytes.len())
                .unwrap_or(bytes.len().saturating_mul(4));
            let mut output = String::with_capacity(max_len);

            let (result, _read) =
                decoder.decode_to_string_without_replacement(bytes, &mut output, last);

            match result {
                encoding_rs::DecoderResult::InputEmpty => Ok(output),
                encoding_rs::DecoderResult::Malformed(_, _) => Err(js::error::throw_type_error(
                    scope,
                    c"The encoded data was not valid.",
                )),
                encoding_rs::DecoderResult::OutputFull => {
                    unreachable!("output buffer was pre-allocated to max size");
                }
            }
        } else {
            // Malformed sequences become U+FFFD.
            let max_len = decoder
                .max_utf8_buffer_length(bytes.len())
                .unwrap_or(bytes.len().saturating_mul(4).saturating_add(3));
            let mut output = String::with_capacity(max_len);

            let (_result, _read, _had_errors) = decoder.decode_to_string(bytes, &mut output, last);

            Ok(output)
        }
    }
}


#[webidl_union]
pub enum ArrayBufferViewOrArrayBuffer<'a> {
    View(ArrayBufferView<'a>),
    Buffer(ArrayBuffer<'a>),
}

#[webidl_dictionary]
pub struct TextDecodeOptions {
    #[webidl(default = false)]
    pub stream: bool,
}
