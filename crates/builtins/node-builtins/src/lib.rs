// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Node.js compatibility builtins.
//!
//! This crate provides Node.js-style globals and modules for runtime
//! compatibility. Currently implements:
//!
//! - `process`: The global `process` object with standard properties and methods.
//! - `node:assert`: Minimal native assert module (AssertionError, strictEqual, throws, …).
//! - `node:fs`: Minimal native fs module (readFileSync, readdirSync, existsSync, etc.).
//! - `node:fs/promises`: Promise-based fs module.
//! - `node:path`: Minimal native path module (resolve, join, dirname, etc.).
//! - `require()`: Global function that resolves registered native modules.

pub mod assert;
pub mod fs;
pub mod path;
pub mod process;

use js::conversion::FromJSVal;
use js::error::ExnThrown;
use js::function::CallbackArgs;
use js::gc::scope::Scope;
use js::native::Value;
use js::prelude::HandleValue;

fn normalize_specifier(id: &str) -> &str {
    match id {
        "assert" => "node:assert",
        "fs" => "node:fs",
        "fs/promises" => "node:fs/promises",
        "path" => "node:path",
        s => s,
    }
}

fn require_impl(
    scope: &Scope<'_>,
    args: CallbackArgs<'_>,
    _payload: HandleValue,
) -> Result<Value, ExnThrown> {
    if args.is_empty() {
        return Err(
            js::error::TypeError("require() needs at least one argument".to_string()).throw(scope),
        );
    }
    let id = String::from_jsval(scope, args.get(0), ()).map_err(|_| {
        js::error::TypeError("require() argument must be a string".to_string()).throw(scope)
    })?;
    let specifier = normalize_specifier(&id);
    match core_runtime::module::get_module_namespace(scope, specifier) {
        Some(ns) => Ok(ns.as_value()),
        None => Err(js::error::throw_error(scope, &format!("Cannot find module '{id}'"))),
    }
}

pub fn add_to_global(scope: &js::prelude::Scope<'_>, global: js::Object<'_>) {
    assert::assert_ns::register(scope);
    fs::register(scope);
    path::register(scope);
    process::add_to_global(scope, global);

    let require_fn =
        js::Function::new_callback(scope, c"require", 1, require_impl, js::value::undefined())
            .expect("failed to create require function");
    global
        .set_property(scope, c"require", require_fn)
        .expect("failed to set require on global");
}
