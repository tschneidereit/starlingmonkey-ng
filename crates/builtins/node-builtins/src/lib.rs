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

pub mod assert;
pub mod fs;
pub mod path;
pub mod process;

pub fn add_to_global(scope: &js::prelude::Scope<'_>, global: js::Object<'_>) {
    assert::assert_ns::register(scope);
    fs::register(scope);
    path::register(scope);
    process::add_to_global(scope, global);
}
