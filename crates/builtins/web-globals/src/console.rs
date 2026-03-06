// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Minimal `console` object implementation.
//!
//! Provides `console.log`, `console.warn`, `console.error`, `console.info`,
//! and `console.debug`. Each method converts its arguments to strings via
//! `ToString` and prints them space-separated to stdout (for `log`, `info`,
//! `debug`) or stderr (for `warn`, `error`).

use std::io::Write;
use std::ptr::NonNull;

use js::gc::scope::Scope;
use js::native::CallArgs;

/// Format call arguments as a space-separated string.
///
/// Each argument is converted to a string via the JS `ToString` operation.
fn format_args(scope: &Scope<'_>, args: &CallArgs) -> String {
    let mut parts = Vec::with_capacity(args.argc_ as usize);
    for i in 0..args.argc_ {
        // SAFETY: `args.get(i)` dereferences vp which is valid for the duration
        // of the native call.
        let val = *args.get(i);
        // Fast path for strings; slow path calls JS ToString.
        let js_str = if val.is_string() {
            NonNull::new(val.to_string()).map(|p| scope.root_string(p))
        } else {
            let handle = scope.root_value(val);
            js::string::to_string_slow(scope, handle).ok()
        };
        match js_str {
            Some(s) => match js::string::to_utf8(scope, s) {
                Ok(utf8) => parts.push(utf8),
                Err(_) => parts.push(String::from("[error converting to UTF-8]")),
            },
            None => parts.push(String::from("[error converting to string]")),
        }
    }
    parts.join(" ")
}

/// Print to stdout (for `log`, `info`, `debug`).
fn print_stdout(scope: &Scope<'_>, args: &CallArgs) {
    let output = format_args(scope, args);
    let _ = writeln!(std::io::stdout(), "{output}");
}

/// Print to stderr (for `warn`, `error`).
fn print_stderr(scope: &Scope<'_>, args: &CallArgs) {
    let output = format_args(scope, args);
    let _ = writeln!(std::io::stderr(), "{output}");
}

#[core_runtime::jsnamespace(name = "console")]
pub mod console_ns {
    use js::gc::scope::Scope;
    use js::native::CallArgs;

    pub fn log(scope: &Scope<'_>, args: &CallArgs) {
        super::print_stdout(scope, args);
    }

    pub fn info(scope: &Scope<'_>, args: &CallArgs) {
        super::print_stdout(scope, args);
    }

    pub fn debug(scope: &Scope<'_>, args: &CallArgs) {
        super::print_stdout(scope, args);
    }

    pub fn warn(scope: &Scope<'_>, args: &CallArgs) {
        super::print_stderr(scope, args);
    }

    pub fn error(scope: &Scope<'_>, args: &CallArgs) {
        super::print_stderr(scope, args);
    }
}
