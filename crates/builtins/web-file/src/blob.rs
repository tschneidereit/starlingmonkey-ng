// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Implementation of the `Blob` global.
//!
//! Implements [`Blob`](https://w3c.github.io/FileAPI/#blob-section) WebIDL.

use std::{
    cmp::{max, min},
    io::Write,
};

use core_runtime::{webidl_interface, webidl_methods};
use js::{
    builtins::{CastTarget, JSType},
    conversion::{for_of, ConversionError, FromJSVal as _, ToJSVal},
    error::ExnThrown,
    gc::scope::Scope,
    prelude::HandleValue,
    string::Str,
    Array, ArrayBuffer, ArrayBufferView, Object, Promise, Uint8Array,
};
use web_globals::encoding::text_decoder::{ArrayBufferViewOrArrayBuffer, TextDecoder};
use web_streams::readable::ReadableStream;

/// <https://w3c.github.io/FileAPI/#blob-section>
#[webidl_interface]
pub struct Blob {
    buffer: Vec<u8>,
    content_type: String,
    #[no_trace]
    line_endings: Option<BlobLineEndings>,
}

/// <https://w3c.github.io/FileAPI/#enumdef-endingtype>
#[derive(Debug, Clone, Copy, Default)]
pub enum BlobLineEndings {
    #[default]
    /// Use line endings as-is, without transforming them for the current platform.
    Transparent,
    /// Transform string line endings into the current platform's native line endings (e.g. `CRLF` on Windows, `LF` on *nix, etc)
    Native,
}

#[webidl_methods]
impl Blob {
    /// The `Blob()` constructor returns a new
    /// [`Blob`](https://w3c.github.io/FileAPI/#blob-section) object. The
    /// content of the blob consists of the concatenation of the values given in
    /// the parameter `blobParts`.
    #[constructor]
    pub fn new(
        &self,
        scope: &'s Scope<'_>,
        blob_parts: Option<HandleValue>,
        // NB(@zkat): We can't use `webidl_dictionary` here because it'll mess
        // with evaluation order relative to `blobParts` (particularly,
        // execution of getters and ToString)
        options: Option<HandleValue>,
    ) -> Result<(), ExnThrown> {
        if let Some(parts) = blob_parts {
            self.init_blob_parts(scope, parts, options)?;
        }
        if let Some(opts) = options {
            // NB(@zkat): Per WPTs, we have to fall back to defaults if we get `null` for opts.
            if !opts.is_null_or_undefined() {
                self.init_options(scope, opts)?;
            }
        }
        Ok(())
    }

    fn init_blob_parts(
        &self,
        scope: &'s Scope<'_>,
        parts: HandleValue,
        opts: Option<HandleValue>,
    ) -> Result<(), ExnThrown> {
        if !(parts.is_object()
            && for_of(scope, parts, |item| {
                self.append_value(scope, item, opts)?;
                Ok::<(), ConversionError>(())
            })
            .map_err(|e| e.throw(scope))?)
        {
            return Err(js::error::throw_type_error(
                scope,
                c"Blob.constructor: expected blobParts to be an object",
            ));
        }
        Ok(())
    }

    fn append_value(
        &self,
        scope: &'s Scope<'_>,
        val: HandleValue,
        opts: Option<HandleValue>,
    ) -> Result<(), ExnThrown> {
        if val.is_object() {
            let obj = val.to_object();
            // SAFETY: is_instance is unsafe if the pointer is not an object,
            // but we just checked that it is.
            if unsafe { BlobImpl::is_instance(obj) } {
                let other = Blob::from_jsval(scope, val, ()).map_err(|e| e.throw(scope))?;
                let bytes = &other.data().buffer[..];
                self.data_mut().buffer.write_all(bytes).map_err(|_| {
                    js::error::throw_internal_error(scope, c"Write to new Blob interrupted")
                })?;
                return Ok(());
            } else {
                let is_buf_view = unsafe { ArrayBufferView::is_instance(obj) };
                let is_buf = unsafe { ArrayBuffer::is_instance(obj) };
                if is_buf_view || is_buf {
                    let bytes = if is_buf_view {
                        // SAFETY: `bytes` is dropped before any GC happens.
                        unsafe {
                            ArrayBufferView::from_jsval(scope, val, ())
                                .map_err(|e| e.throw(scope))?
                                .bytes()
                        }
                    } else if is_buf {
                        // SAFETY: `bytes` is dropped before any GC happens.
                        unsafe {
                            ArrayBuffer::from_jsval(scope, val, ())
                                .map_err(|e| e.throw(scope))?
                                .bytes()
                        }
                    } else {
                        &[]
                    };
                    if !bytes.is_empty() {
                        self.data_mut().buffer.write_all(bytes).map_err(|_| {
                            js::error::throw_internal_error(scope, c"Write to new Blob interrupted")
                        })?
                    }
                    return Ok(());
                }
            }
        } else if val.is_string() {
            // NB(@zkat): Yes, we're putting regular strings through ToString,
            // but that's fine because it's shortcutted for strings.
            let string = Str::from_value(scope, val)?.to_utf8(scope)?;
            let normalized = match self.get_endings_opt(scope, opts)? {
                BlobLineEndings::Native => Self::normalize_line_ending(string),
                BlobLineEndings::Transparent => string,
            };
            self.data_mut()
                .buffer
                .write_all(normalized.as_bytes())
                .map_err(|_| {
                    js::error::throw_internal_error(scope, c"Write to new Blob interrupted")
                })?;
            return Ok(());
        }

        // FALLBACK: convert and call again.
        let string = Str::from_value(scope, val)?.to_utf8(scope)?;
        self.append_value(
            scope,
            string.to_jsval(scope).map_err(|e| e.throw(scope))?,
            opts,
        )
    }

    fn get_endings_opt(
        &self,
        scope: &'s Scope<'_>,
        opts: Option<HandleValue>,
    ) -> Result<BlobLineEndings, ExnThrown> {
        if let Some(ending) = self.data().line_endings {
            return Ok(ending);
        }

        let opt = if let Some(opts) = opts {
            // NB(@zkat): Per WPTs, we have to fall back to defaults if we get `null` for opts.
            if opts.is_null_or_undefined() {
                return Ok(BlobLineEndings::Transparent);
            }

            if !opts.is_object() {
                return Err(js::error::throw_type_error(
                    scope,
                    c"Blob.constructor: options must be an object",
                ));
            }

            let obj = Object::from_value_coerce(scope, opts).expect(
                "We just checked that it's an object, and it's not null. This shouldn't fail.",
            );

            let has_endings =
                self.data().line_endings.is_some() || obj.has_property(scope, c"endings")?;

            if has_endings && self.data().line_endings.is_none() {
                let endings = Str::from_value(scope, obj.get_property(scope, c"endings")?)?;
                if endings.equals_ascii(scope, c"native")? {
                    BlobLineEndings::Native
                } else {
                    // NB(@zkat): As far as I can tell, we should be treating
                    // invalid `endings` values as `transparent'
                    BlobLineEndings::Transparent
                }
            } else {
                BlobLineEndings::Transparent
            }
        } else {
            BlobLineEndings::Transparent
        };
        self.data_mut().line_endings = Some(opt);
        Ok(opt)
    }

    fn normalize_line_ending(string: String) -> String {
        #[cfg(windows)]
        let native_line_ending = "\r\n";
        #[cfg(not(windows))]
        let native_line_ending = "\n";
        // NB(@zkat): This sparks a certain kind of joy, and does the opposite
        // if you wanted a big weird single-pass loop. Pray the compiler does
        // its job.
        string
            .replace("\r\n", "\n")
            .replace("\r", "\n")
            .replace("\n", native_line_ending)
    }

    fn init_options(&self, scope: &'s Scope<'_>, opts: HandleValue) -> Result<(), ExnThrown> {
        if !opts.is_object() {
            return Err(js::error::throw_type_error(
                scope,
                c"Blob.constructor: options must be an object",
            ));
        }

        let obj = Object::from_value_coerce(scope, opts)
            .expect("We just checked that it's an object, and it's not null. This shouldn't fail.");

        let has_endings =
            self.data().line_endings.is_some() || obj.has_property(scope, c"endings")?;
        let has_type = obj.has_property(scope, c"type")?;

        if !has_endings && !has_type {
            // Use defaults
            return Ok(());
        }

        if has_endings && self.data().line_endings.is_none() {
            let endings = Str::from_value(scope, obj.get_property(scope, c"endings")?)?;
            if endings.equals_ascii(scope, c"native")? {
                self.data_mut().line_endings = Some(BlobLineEndings::Native);
            } else {
                // NB(@zkat): As far as I can tell, we should be treating
                // invalid `endings` values as `transparent'
                self.data_mut().line_endings = Some(BlobLineEndings::Transparent);
            }
        }

        if has_type {
            self.data_mut().content_type = Self::normalize_content_type(
                Str::from_value(scope, obj.get_property(scope, c"type")?)?.to_utf8(scope)?,
            );
        }

        Ok(())
    }

    // 1. If type contains any characters outside the range U+0020 to U+007E, then set t to the empty string.
    // 2. Convert every character in type to ASCII lowercase.
    fn normalize_content_type(ty: String) -> String {
        for ch in ty.chars() {
            let ch = ch as u32;
            if ch < 0x0020 || ch > 0x007E {
                return "".into();
            }
        }
        ty.to_lowercase()
    }

    #[getter]
    pub fn size(&self) -> u64 {
        self.data().buffer.len() as u64
    }

    #[getter(name = "type")]
    pub fn ty(&self) -> String {
        self.data().content_type.to_lowercase()
    }

    /// The arrayBuffer() method of the Blob interface returns a Promise that
    /// resolves with the contents of the blob as binary data contained in an
    /// ArrayBuffer.
    ///
    /// <https://w3c.github.io/FileAPI/#dom-blob-arraybuffer>
    #[method]
    pub fn array_buffer(&self, scope: &'s Scope<'_>) -> Result<Promise<'s>, ExnThrown> {
        let buffer = ArrayBuffer::with_data(scope, &self.data().buffer[..])?;
        Promise::call_original_resolve(scope, buffer.to_jsval(scope).map_err(|e| e.throw(scope))?)
    }

    /// The bytes() method of the Blob interface returns a Promise that resolves
    /// with a Uint8Array containing the contents of the blob as an array of
    /// bytes.
    ///
    /// <https://w3c.github.io/FileAPI/#dom-blob-bytes>
    #[method]
    pub fn bytes(&self, scope: &'s Scope<'_>) -> Result<Promise<'s>, ExnThrown> {
        let buffer = ArrayBuffer::with_data(scope, &self.data().buffer[..])?;
        // NB(@zkat): We _could_ `Uint8Array::with_data` but that would be
        // different from what the spec says, though I'm not sure if there's any
        // tangible difference if the ArrayBuffer is never visible outside of
        // this function?
        let arr = Uint8Array::with_buffer(scope, buffer, 0, buffer.byte_length())?;
        Promise::call_original_resolve(scope, arr.to_jsval(scope).map_err(|e| e.throw(scope))?)
    }

    /// The slice() method of the Blob interface creates and returns a new Blob
    /// object which contains data from a subset of the blob on which it's
    /// called.
    ///
    /// <https://w3c.github.io/FileAPI/#dfn-slice>
    #[method]
    pub fn slice(
        &self,
        scope: &'s Scope<'_>,
        start: Option<i64>,
        end: Option<i64>,
        content_type: Option<String>,
    ) -> Result<Self, ExnThrown> {
        let size = self.data().buffer.len();

        let start = start.unwrap_or(0);
        let end = end.unwrap_or(size as i64);

        let start = if start < 0 {
            max(start + size as i64, 0)
        } else {
            min(start, size as i64)
        } as usize;

        let end = if end < 0 {
            max(end + size as i64, 0)
        } else {
            min(end, size as i64)
        } as usize;

        let options = Object::new_plain(scope)?;
        options.set_property(scope, c"type", content_type.unwrap_or_else(|| "".into()))?;
        let blob = Blob::new(
            scope,
            Some(
                Array::with_contents(
                    scope,
                    &[
                        Uint8Array::with_data(scope, &self.data().buffer[start..end])?
                            .to_jsval(scope)
                            .map_err(|e| e.throw(scope))?,
                    ],
                )?
                .to_jsval(scope)
                .map_err(|e| e.throw(scope))?,
            ),
            Some(
                options
                    .handle()
                    .to_jsval(scope)
                    .map_err(|e| e.throw(scope))?,
            ),
        )?;
        Ok(blob)
    }

    /// The text() method of the Blob interface returns a Promise that resolves
    /// with a string containing the contents of the blob, interpreted as UTF-8.
    ///
    /// <https://w3c.github.io/FileAPI/#dom-blob-text>
    #[method]
    pub fn text(&self, scope: &'s Scope<'_>) -> Result<Promise<'s>, ExnThrown> {
        let buffer = ArrayBuffer::with_data(scope, &self.data().buffer[..])?;
        let decoder = TextDecoder::new(scope, None, None)?;
        let text = decoder.decode(
            scope,
            Some(ArrayBufferViewOrArrayBuffer::Buffer(buffer)),
            None,
        )?;
        Promise::call_original_resolve(scope, text.to_jsval(scope).map_err(|e| e.throw(scope))?)
    }

    /// The stream() method of the Blob interface returns a ReadableStream which
    /// upon reading returns the data contained within the Blob.
    ///
    /// <https://w3c.github.io/FileAPI/#stream-method-algo>
    #[method]
    pub fn stream(&self, scope: &'s Scope<'_>) -> Result<ReadableStream<'s>, ExnThrown> {
        ReadableStream::from_bytes(scope, &self.data().buffer[..])
    }

    // TODO(@zkat): Pending implementation of `TextEncoderStream`
    // This is equivalent to piping `blob.stream()` through a `TextDecoderStream`.
    // #[method]
    // pub fn text_stream(&self, scope: &'s Scope<'_>) -> Result<DefaultReader<'s>, ExnThrown> {

    // }
}
