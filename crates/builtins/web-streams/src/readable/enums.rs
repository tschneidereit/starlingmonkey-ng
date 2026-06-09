// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! WebIDL enumerations from <https://streams.spec.whatwg.org/>

use std::borrow::Cow;
use std::fmt;

use js::conversion::{ConversionError, FromJSVal, ToJSVal};
use js::gc::scope::Scope;
use js::prelude::HandleValue;

/// WebIDL enum `ReadableStreamReaderMode`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderMode {
    /// `"byob"`
    Byob,
}

impl fmt::Display for ReaderMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Byob => "byob",
        })
    }
}

impl std::str::FromStr for ReaderMode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "byob" => Ok(Self::Byob),
            _ => Err(()),
        }
    }
}

impl<'s> FromJSVal<'s> for ReaderMode {
    type Config = ();

    fn from_jsval(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        _: (),
    ) -> Result<Self, ConversionError> {
        let s = String::from_jsval(scope, val, ())?;
        match s.as_str() {
            "byob" => Ok(Self::Byob),
            _ => Err(ConversionError::Failure(Cow::Borrowed(
                c"invalid value for 'mode'",
            ))),
        }
    }
}

impl<'s> ToJSVal<'s> for ReaderMode {
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        match self {
            Self::Byob => "byob".to_jsval(scope),
        }
    }
}

/// WebIDL enum `ReadableStreamType`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadableStreamType {
    /// `"bytes"`
    Bytes,
}

impl fmt::Display for ReadableStreamType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Bytes => "bytes",
        })
    }
}

impl std::str::FromStr for ReadableStreamType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bytes" => Ok(Self::Bytes),
            _ => Err(()),
        }
    }
}

impl<'s> FromJSVal<'s> for ReadableStreamType {
    type Config = ();

    fn from_jsval(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        _: (),
    ) -> Result<Self, ConversionError> {
        let s = String::from_jsval(scope, val, ())?;
        match s.as_str() {
            "bytes" => Ok(Self::Bytes),
            _ => Err(ConversionError::Failure(Cow::Borrowed(
                c"invalid value for 'type'",
            ))),
        }
    }
}

impl<'s> ToJSVal<'s> for ReadableStreamType {
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        match self {
            Self::Bytes => "bytes".to_jsval(scope),
        }
    }
}
