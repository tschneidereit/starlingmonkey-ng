// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! JSON parsing and serialization.
//!
//! This module wraps SpiderMonkey's JSON API, providing safe access to
//! `JSON.parse` and `JSON.stringify` operations.

use crate::gc::scope::Scope;
use mozjs::gc::{HandleObject, HandleString, HandleValue, MutableHandleValue};
use mozjs::jsapi::{JSONWriteCallback, Value};
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2;

use super::error::ExnThrown;

/// Parse a JSON string into a JS value.
///
/// Accepts a Rust `&str` and parses it using SpiderMonkey's JSON parser.
pub fn parse(scope: &Scope<'_>, json: &str) -> Result<Value, ExnThrown> {
    let utf16: Vec<u16> = json.encode_utf16().collect();
    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
    // SAFETY: utf16 is a valid buffer that lives for the duration of this call.
    let ok = unsafe {
        wrappers2::JS_ParseJSON(
            scope.cx_mut(),
            utf16.as_ptr(),
            utf16.len() as u32,
            rval.handle_mut(),
        )
    };
    ExnThrown::check(ok)?;
    Ok(rval.get())
}

/// Parse a JSON string (represented as a `JSString`) into a JS value.
pub fn parse_js_string(scope: &Scope<'_>, json_str: HandleString) -> Result<Value, ExnThrown> {
    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
    let ok = unsafe { wrappers2::JS_ParseJSON1(scope.cx_mut(), json_str, rval.handle_mut()) };
    ExnThrown::check(ok)?;
    Ok(rval.get())
}

/// Parse a JSON string from UTF-16 chars into a JS value.
///
/// # Safety
///
/// `chars` must point to a valid UTF-16 buffer of at least `len` code units.
pub unsafe fn parse_utf16(
    scope: &Scope<'_>,
    chars: *const u16,
    len: u32,
) -> Result<Value, ExnThrown> {
    rooted!(in(scope.raw_cx_no_gc()) let mut rval = UndefinedValue());
    let ok = wrappers2::JS_ParseJSON(scope.cx_mut(), chars, len, rval.handle_mut());
    ExnThrown::check(ok)?;
    Ok(rval.get())
}

/// Parse a JSON string with a reviver function.
///
/// Accepts a Rust `&str` and parses it with a JS reviver function.
pub fn parse_with_reviver(
    scope: &Scope<'_>,
    json: &str,
    reviver: HandleValue,
) -> Result<Value, ExnThrown> {
    let utf16: Vec<u16> = json.encode_utf16().collect();
    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
    // SAFETY: utf16 is a valid buffer that lives for the duration of this call.
    let ok = unsafe {
        wrappers2::JS_ParseJSONWithReviver(
            scope.cx_mut(),
            utf16.as_ptr(),
            utf16.len() as u32,
            reviver,
            rval.handle_mut(),
        )
    };
    ExnThrown::check(ok)?;
    Ok(rval.get())
}

/// Parse JSON with a reviver function (JS string input).
pub fn parse_js_string_with_reviver(
    scope: &Scope<'_>,
    json_str: HandleString,
    reviver: HandleValue,
) -> Result<Value, ExnThrown> {
    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
    let ok = unsafe {
        wrappers2::JS_ParseJSONWithReviver1(scope.cx_mut(), json_str, reviver, rval.handle_mut())
    };
    ExnThrown::check(ok)?;
    Ok(rval.get())
}

/// Stringify a JS value to JSON using a callback to receive the output.
///
/// `replacer` can be null for no replacer, or a function/array object.
/// `space` controls indentation (number or string value, or undefined for none).
///
/// # Safety
///
/// `callback` must be a valid function pointer. `data` is passed through
/// to the callback and must remain valid for the duration.
pub unsafe fn stringify(
    scope: &Scope<'_>,
    value: MutableHandleValue,
    replacer: HandleObject,
    space: HandleValue,
    callback: JSONWriteCallback,
    data: *mut std::os::raw::c_void,
) -> Result<(), ExnThrown> {
    let ok = wrappers2::JS_Stringify(scope.cx_mut(), value, replacer, space, callback, data);
    ExnThrown::check(ok)
}
