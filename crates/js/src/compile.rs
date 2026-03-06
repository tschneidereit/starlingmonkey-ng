// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Script compilation and evaluation.
//!
//! This module wraps SpiderMonkey's compilation and evaluation APIs, with both
//! safe convenience functions (e.g. [`evaluate`]) and lower-level wrappers for
//! advanced use cases.
//!
//! # Quick Start
//!
//! ```ignore
//! use crate::compile;
//! let result = compile::evaluate(&scope, "2 + 2")?;
//! ```

use std::ffi::CString;
use std::ptr::NonNull;

use crate::gc::scope::Scope;
use mozjs::gc::{Handle, HandleFunction, HandleScript};
use mozjs::jsapi::mozilla::Utf8Unit;
use mozjs::jsapi::{
    EnvironmentChain, JSFunction, JSScript, ReadOnlyCompileOptions, SourceText, Value,
};
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::{transform_str_to_source_text, wrappers2, CompileOptionsWrapper};

use super::error::JSError;

/// Evaluate a UTF-8 script string and return its completion value.
///
/// This is the primary entry point for evaluating JavaScript from Rust. It
/// handles compile options creation internally.
pub fn evaluate(scope: &Scope<'_>, script: &str) -> Result<Value, JSError> {
    evaluate_with_filename(scope, script, "<inline>", 1)
}

/// Evaluate a UTF-8 script string with a custom filename and starting line.
pub fn evaluate_with_filename(
    scope: &Scope<'_>,
    script: &str,
    filename: &str,
    lineno: u32,
) -> Result<Value, JSError> {
    evaluate_with_options(scope, script, filename, lineno, false)
}

/// Evaluate a UTF-8 script string in a non-syntactic scope.
///
/// This is equivalent to calling [`evaluate_with_filename`] but with the
/// `nonSyntacticScope` option set. In a non-syntactic scope, top-level `let` and
/// `const` bindings are placed on a shared scope object rather than creating a
/// new lexical scope, making them visible to subsequent `evalScript` calls.
///
/// This is primarily used by the WPT (Web Platform Tests) harness to emulate
/// the behavior of HTML `<script>` tags.
pub fn evaluate_non_syntactic(
    scope: &Scope<'_>,
    script: &str,
    filename: &str,
    lineno: u32,
) -> Result<Value, JSError> {
    evaluate_with_options(scope, script, filename, lineno, true)
}

fn evaluate_with_options(
    scope: &Scope<'_>,
    script: &str,
    filename: &str,
    lineno: u32,
    non_syntactic_scope: bool,
) -> Result<Value, JSError> {
    let filename_cstr =
        CString::new(filename).unwrap_or_else(|_| CString::new("<unknown>").unwrap());
    let options = CompileOptionsWrapper::new(scope.cx(), filename_cstr, lineno);
    if non_syntactic_scope {
        // SAFETY: `options.ptr` is a valid pointer created by `NewCompileOptions`.
        // We are setting the `nonSyntacticScope` field on the base
        // `TransitiveCompileOptions` struct, which is safe because the pointer
        // is valid and we own it exclusively.
        unsafe {
            (*options.ptr)._base.nonSyntacticScope = non_syntactic_scope;
        }
    }
    let mut source = transform_str_to_source_text(script);
    // SAFETY: we're calling into SpiderMonkey with valid compile options and source.
    // The rooted! macro creates a temporary root for the result value.
    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
    let ok = unsafe {
        wrappers2::Evaluate2(scope.cx_mut(), options.ptr, &mut source, rval.handle_mut())
    };
    JSError::check(ok)?;
    Ok(rval.get())
}

/// Evaluate a UTF-16 script and return its completion value.
///
/// # Safety
///
/// `options` must be a valid pointer to `ReadOnlyCompileOptions`.
/// `src_buf` must be a valid pointer to a `SourceText<u16>`.
pub unsafe fn evaluate_utf16_raw(
    scope: &Scope<'_>,
    options: *const ReadOnlyCompileOptions,
    src_buf: *mut SourceText<u16>,
) -> Result<Value, JSError> {
    rooted!(in(scope.raw_cx_no_gc()) let mut rval = UndefinedValue());
    let ok = wrappers2::Evaluate(scope.cx_mut(), options, src_buf, rval.handle_mut());
    JSError::check(ok)?;
    Ok(rval.get())
}

/// Evaluate a UTF-8 script with raw compile options.
///
/// # Safety
///
/// `options` must be a valid pointer to `ReadOnlyCompileOptions`.
/// `src_buf` must be a valid pointer to a `SourceText<Utf8Unit>`.
pub unsafe fn evaluate_utf8_raw(
    scope: &Scope<'_>,
    options: *const ReadOnlyCompileOptions,
    src_buf: *mut SourceText<Utf8Unit>,
) -> Result<Value, JSError> {
    rooted!(in(scope.raw_cx_no_gc()) let mut rval = UndefinedValue());
    let ok = wrappers2::Evaluate2(scope.cx_mut(), options, src_buf, rval.handle_mut());
    JSError::check(ok)?;
    Ok(rval.get())
}

/// Evaluate a UTF-16 script with a custom environment chain.
///
/// # Safety
///
/// `env_chain`, `options`, and `src_buf` must all be valid pointers.
pub unsafe fn evaluate_with_env_raw(
    scope: &Scope<'_>,
    env_chain: *const EnvironmentChain,
    options: *const ReadOnlyCompileOptions,
    src_buf: *mut SourceText<u16>,
) -> Result<Value, JSError> {
    rooted!(in(scope.raw_cx_no_gc()) let mut rval = UndefinedValue());
    let ok = wrappers2::Evaluate1(
        scope.cx_mut(),
        env_chain,
        options,
        src_buf,
        rval.handle_mut(),
    );
    JSError::check(ok)?;
    Ok(rval.get())
}

/// Evaluate a script from a UTF-8 file path.
///
/// # Safety
///
/// `options` must be valid. `filename` must be a valid C string.
pub unsafe fn evaluate_path_raw(
    scope: &Scope<'_>,
    options: *const ReadOnlyCompileOptions,
    filename: *const std::os::raw::c_char,
) -> Result<Value, JSError> {
    rooted!(in(scope.raw_cx_no_gc()) let mut rval = UndefinedValue());
    let ok = wrappers2::EvaluateUtf8Path(scope.cx_mut(), options, filename, rval.handle_mut());
    JSError::check(ok)?;
    Ok(rval.get())
}

/// Compile a UTF-8 script string into a `JSScript` without executing it.
///
/// Uses `"<inline>"` as the filename. For a custom filename, use
/// [`compile_with_filename`].
pub fn compile<'s>(
    scope: &'s Scope<'_>,
    script: &str,
) -> Result<Handle<'s, *mut JSScript>, JSError> {
    compile_with_filename(scope, script, "<inline>", 1)
}

/// Compile a UTF-8 script string with a custom filename and starting line.
pub fn compile_with_filename<'s>(
    scope: &'s Scope<'_>,
    script: &str,
    filename: &str,
    lineno: u32,
) -> Result<Handle<'s, *mut JSScript>, JSError> {
    let filename_cstr =
        CString::new(filename).unwrap_or_else(|_| CString::new("<unknown>").unwrap());
    let options = CompileOptionsWrapper::new(scope.cx(), filename_cstr, lineno);
    let mut source = transform_str_to_source_text(script);
    let script = unsafe { wrappers2::Compile1(scope.cx_mut(), options.ptr, &mut source) };
    NonNull::new(script)
        .map(|p| scope.root_script(p))
        .ok_or(JSError)
}

/// Compile a UTF-16 script into a `JSScript` without executing it.
///
/// # Safety
///
/// `options` and `src_buf` must be valid pointers.
pub unsafe fn compile_utf16_raw<'s>(
    scope: &'s Scope<'_>,
    options: *const ReadOnlyCompileOptions,
    src_buf: *mut SourceText<u16>,
) -> Result<Handle<'s, *mut JSScript>, JSError> {
    let script = wrappers2::Compile(scope.cx_mut(), options, src_buf);
    NonNull::new(script)
        .map(|p| scope.root_script(p))
        .ok_or(JSError)
}

/// Compile a UTF-8 script into a `JSScript` without executing it.
///
/// # Safety
///
/// `options` and `src_buf` must be valid pointers.
pub unsafe fn compile_raw<'s>(
    scope: &'s Scope<'_>,
    options: *const ReadOnlyCompileOptions,
    src_buf: *mut SourceText<Utf8Unit>,
) -> Result<Handle<'s, *mut JSScript>, JSError> {
    let script = wrappers2::Compile1(scope.cx_mut(), options, src_buf);
    NonNull::new(script)
        .map(|p| scope.root_script(p))
        .ok_or(JSError)
}

/// Execute a previously compiled script, returning its completion value.
pub fn execute_script(scope: &Scope<'_>, script: HandleScript) -> Result<Value, JSError> {
    // SAFETY: scope guarantees a realm is entered; script handle is rooted.
    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut rval = UndefinedValue());
    let ok = unsafe { wrappers2::JS_ExecuteScript(scope.cx_mut(), script, rval.handle_mut()) };
    JSError::check(ok)?;
    Ok(rval.get())
}

/// Execute a previously compiled script, discarding the result.
pub fn execute_script_no_rval(scope: &Scope<'_>, script: HandleScript) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::JS_ExecuteScript1(scope.cx_mut(), script) };
    JSError::check(ok)
}

/// Compile a function from UTF-16 source.
///
/// # Safety
///
/// `env_chain`, `options`, `name`, `argnames`, and `src_buf` must be valid.
pub unsafe fn compile_function_utf16_raw<'s>(
    scope: &'s Scope<'_>,
    env_chain: *const EnvironmentChain,
    options: *const ReadOnlyCompileOptions,
    name: *const std::os::raw::c_char,
    nargs: std::os::raw::c_uint,
    argnames: *const *const std::os::raw::c_char,
    src_buf: *mut SourceText<u16>,
) -> Result<Handle<'s, *mut JSFunction>, JSError> {
    let fun = wrappers2::CompileFunction(
        scope.cx_mut(),
        env_chain,
        options,
        name,
        nargs,
        argnames,
        src_buf,
    );
    NonNull::new(fun)
        .map(|p| scope.root_function(p))
        .ok_or(JSError)
}

/// Compile a function from UTF-8 source.
///
/// # Safety
///
/// `env_chain`, `options`, `name`, `argnames`, and `src_buf` must be valid.
pub unsafe fn compile_function_raw<'s>(
    scope: &'s Scope<'_>,
    env_chain: *const EnvironmentChain,
    options: *const ReadOnlyCompileOptions,
    name: *const std::os::raw::c_char,
    nargs: std::os::raw::c_uint,
    argnames: *const *const std::os::raw::c_char,
    src_buf: *mut SourceText<Utf8Unit>,
) -> Result<Handle<'s, *mut JSFunction>, JSError> {
    let fun = wrappers2::CompileFunction1(
        scope.cx_mut(),
        env_chain,
        options,
        name,
        nargs,
        argnames,
        src_buf,
    );
    NonNull::new(fun)
        .map(|p| scope.root_function(p))
        .ok_or(JSError)
}

/// Decompile a script to source text.
pub fn decompile_script<'s>(
    scope: &'s Scope<'_>,
    script: HandleScript,
) -> Result<Handle<'s, *mut mozjs::jsapi::JSString>, JSError> {
    let s = unsafe { wrappers2::JS_DecompileScript(scope.cx_mut(), script) };
    NonNull::new(s).map(|p| scope.root_string(p)).ok_or(JSError)
}

/// Decompile a function to source text.
pub fn decompile_function<'s>(
    scope: &'s Scope<'_>,
    fun: HandleFunction,
) -> Result<Handle<'s, *mut mozjs::jsapi::JSString>, JSError> {
    let s = unsafe { wrappers2::JS_DecompileFunction(scope.cx_mut(), fun) };
    NonNull::new(s).map(|p| scope.root_string(p)).ok_or(JSError)
}

/// Check whether a UTF-8 string is a complete JavaScript compilation unit.
///
/// Returns `true` if the string could be submitted for compilation (even if
/// it would produce errors), `false` if it is incomplete (e.g. missing closing
/// braces).
pub fn is_compilable_unit(scope: &Scope<'_>, source: &str) -> bool {
    let c_src = std::ffi::CString::new(source).unwrap_or_default();
    // Copy the global pointer before taking the mutable borrow on cx.
    let global = scope.global();
    unsafe {
        wrappers2::JS_Utf8BufferIsCompilableUnit(
            scope.cx_mut(),
            global.handle(),
            c_src.as_ptr(),
            source.len(),
        )
    }
}
