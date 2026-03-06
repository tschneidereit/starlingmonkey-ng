// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Shared test helpers for `core-runtime` in-crate tests.

use crate::{config::RuntimeConfig, runtime::Runtime};

/// Create a temp directory that works on both native and wasm targets.
///
/// On WASI, `std::env::temp_dir()` panics because there is no temp filesystem.
/// The wasmtime runner mounts the CWD, so we use `tempdir_in("/tmp")` instead.
/// The component runtime needs to be invoked with a `--dir=/tmp` option for
/// this to work.
pub fn test_tempdir() -> tempfile::TempDir {
    #[cfg(target_arch = "wasm32")]
    {
        tempfile::Builder::new()
            .tempdir_in("/tmp")
            .expect("failed to create temp dir")
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        tempfile::tempdir().expect("failed to create temp dir")
    }
}

/// Convert a JS return value to a Rust String for test assertions.
///
/// Handles string, boolean, int32, double, and undefined values.
pub fn rval_to_string(scope: &js::gc::scope::Scope<'_>, rval: js::native::Value) -> String {
    if rval.is_undefined() {
        "undefined".to_string()
    } else if rval.is_string() {
        let s =
            scope.root_string(std::ptr::NonNull::new(rval.to_string()).expect("null string"));
        js::string::to_utf8(scope, s).expect("utf8 failed")
    } else if rval.is_boolean() {
        rval.to_boolean().to_string()
    } else if rval.is_int32() {
        rval.to_int32().to_string()
    } else if rval.is_double() {
        rval.to_number().to_string()
    } else {
        panic!("unexpected return type");
    }
}

/// Run setup, create a runtime, evaluate JS code, and convert the result to a string.
pub fn eval_with_setup(setup: impl FnOnce(), code: &str) -> String {
    setup();
    let rt =
        Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();
    let rval =
        js::compile::evaluate_with_filename(&scope, code, "test.js", 1).expect("eval failed");
    rval_to_string(&scope, rval)
}

/// Run setup, create a runtime, and check whether JS code throws.
pub fn throws_with_setup(setup: impl FnOnce(), code: &str) -> bool {
    setup();
    let rt =
        Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();
    js::compile::evaluate_with_filename(&scope, code, "test.js", 1).is_err()
}

