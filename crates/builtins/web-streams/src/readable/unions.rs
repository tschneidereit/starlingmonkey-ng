// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! The `ReadableStreamController` and `ReadableStreamReader` WebIDL union types,
//! each in a rooted form and a traced `Heap*` form for internal slots.

use super::algorithms::GenericReader;
use super::byob_reader::BYOBReaderImpl;
use super::byte_stream_controller::ReadableByteStreamControllerImpl;
use super::default_controller::ReadableStreamDefaultControllerImpl;
use super::default_reader::DefaultReaderImpl;
use super::{
    BYOBReader, DefaultReader, ReadableByteStreamController, ReadableStream,
    ReadableStreamDefaultController,
};
use js::conversion::{ConversionError, ToJSVal};
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::{Object, Promise};

/// <https://streams.spec.whatwg.org/#typedefdef-readablestreamcontroller>
#[derive(Clone, Copy)]
pub(crate) enum ReadableStreamController<'s> {
    Default(ReadableStreamDefaultController<'s>),
    Byte(ReadableByteStreamController<'s>),
}

/// A stream's `[[controller]]` slot value.
#[js::must_root]
#[derive(core_runtime::Traceable)]
pub(crate) enum HeapReadableStreamController {
    Default(Heap<ReadableStreamDefaultControllerImpl>),
    Byte(Heap<ReadableByteStreamControllerImpl>),
}

impl HeapReadableStreamController {
    pub(crate) fn get<'s>(&self, scope: &'s Scope<'_>) -> ReadableStreamController<'s> {
        match self {
            Self::Default(c) => ReadableStreamController::Default(c.get(scope)),
            Self::Byte(c) => ReadableStreamController::Byte(c.get(scope)),
        }
    }
}

impl From<ReadableStreamDefaultController<'_>> for HeapReadableStreamController {
    fn from(controller: ReadableStreamDefaultController<'_>) -> Self {
        Self::Default(Heap::from(controller))
    }
}

impl From<ReadableByteStreamController<'_>> for HeapReadableStreamController {
    fn from(controller: ReadableByteStreamController<'_>) -> Self {
        Self::Byte(Heap::from(controller))
    }
}

/// <https://streams.spec.whatwg.org/#typedefdef-readablestreamreader>
#[derive(Clone, Copy)]
pub(crate) enum ReadableStreamReader<'s> {
    Default(DefaultReader<'s>),
    Byob(BYOBReader<'s>),
}

impl<'s> ReadableStreamReader<'s> {
    pub(crate) fn as_object(&self) -> Object<'s> {
        match self {
            Self::Default(r) => r.as_object(),
            Self::Byob(r) => r.as_object(),
        }
    }
}

impl<'s> ToJSVal<'s> for ReadableStreamReader<'s> {
    fn to_jsval_raw(&self, scope: &'s Scope<'_>) -> Result<js::native::Value, ConversionError> {
        self.as_object().to_jsval_raw(scope)
    }
}

impl GenericReader for ReadableStreamReader<'_> {
    fn generic_stream<'r>(&self, scope: &'r Scope<'_>) -> Option<ReadableStream<'r>> {
        match self {
            Self::Default(r) => r.generic_stream(scope),
            Self::Byob(r) => r.generic_stream(scope),
        }
    }
    fn set_generic_stream(&self, stream: &ReadableStream<'_>) {
        match self {
            Self::Default(r) => r.set_generic_stream(stream),
            Self::Byob(r) => r.set_generic_stream(stream),
        }
    }
    fn clear_generic_stream(&self) {
        match self {
            Self::Default(r) => r.clear_generic_stream(),
            Self::Byob(r) => r.clear_generic_stream(),
        }
    }
    fn generic_closed_promise<'r>(&self, scope: &'r Scope<'_>) -> Promise<'r> {
        match self {
            Self::Default(r) => r.generic_closed_promise(scope),
            Self::Byob(r) => r.generic_closed_promise(scope),
        }
    }
    fn set_generic_closed_promise(&self, promise: Promise<'_>) {
        match self {
            Self::Default(r) => r.set_generic_closed_promise(promise),
            Self::Byob(r) => r.set_generic_closed_promise(promise),
        }
    }
    fn as_generic(&self) -> ReadableStreamReader<'_> {
        *self
    }
}

/// A stream's `[[reader]]` slot value, or the byte tee's current reader.
#[js::must_root]
#[derive(core_runtime::Traceable)]
pub(crate) enum HeapReadableStreamReader {
    Default(Heap<DefaultReaderImpl>),
    Byob(Heap<BYOBReaderImpl>),
}

/// A null default-reader `Heap`, for setup-style constructors that set the slot before the object
/// is exposed. Calling `get()` on it panics.
impl Default for HeapReadableStreamReader {
    fn default() -> Self {
        Self::Default(Heap::default())
    }
}

impl HeapReadableStreamReader {
    pub(crate) fn get<'s>(&self, scope: &'s Scope<'_>) -> ReadableStreamReader<'s> {
        match self {
            Self::Default(r) => ReadableStreamReader::Default(r.get(scope)),
            Self::Byob(r) => ReadableStreamReader::Byob(r.get(scope)),
        }
    }
}

impl From<ReadableStreamReader<'_>> for HeapReadableStreamReader {
    fn from(reader: ReadableStreamReader<'_>) -> Self {
        match reader {
            ReadableStreamReader::Default(r) => Self::Default(Heap::from(r)),
            ReadableStreamReader::Byob(r) => Self::Byob(Heap::from(r)),
        }
    }
}
