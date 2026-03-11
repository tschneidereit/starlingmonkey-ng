// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Build script for cpp-support: compiles the C++ shim library that provides
//! SpiderMonkey glue code (builtins, event loop, host API, etc.).
//!
//! The SpiderMonkey headers live in the mozjs-sys build output.  mozjs-sys must
//! expose them via `cargo:include=<path>` metadata so that this crate receives
//! the path through the `DEP_MOZJS_INCLUDE` environment variable.

use std::path::{Path, PathBuf};
use std::{env, fs, io};
fn visit_dirs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut results = Vec::new();
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                results.extend(visit_dirs(&path)?);
            } else {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "cpp") {
                    results.push(path);
                }
            }
        }
    }
    Ok(results)
}
fn main() {
    let crate_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let cpp_dir = crate_dir.join("cpp");
    let include_dir = crate_dir.join("include");

    // Collect all .cpp files in the cpp/ directory.
    let sources = visit_dirs(&cpp_dir).expect("failed to read cpp/ directory");

    // mozjs-sys exposes its include directory via cargo metadata.
    // Since mozjs-sys declares `links = "mozjs"`, cargo sets DEP_MOZJS_INCLUDE
    // for direct dependents that have mozjs-sys in [dependencies].
    let mozjs_include = env::var("DEP_MOZJS_INCLUDE")
        .expect("DEP_MOZJS_INCLUDE not set — mozjs-sys must emit `cargo:include=<path>`");

    let mut build = cc::Build::new();
    let target = env::var("TARGET").unwrap_or_default();
    let is_wasm = target.contains("wasm");

    let flags = [
        "-Wall",
        "-Wimplicit-fallthrough",
        "-Wno-unknown-warning-option",
        "-Wno-invalid-offsetof",
        "-fno-sized-deallocation",
        "-fno-aligned-new",
        "-fPIC",
        "-fno-rtti",
        "-fno-exceptions",
        "-fno-math-errno",
        "-pipe",
        "-fno-omit-frame-pointer",
        "-funwind-tables",
        "-DRUST_BINDGEN", // mozjs sets this, so we have to as well.
    ];
    if env::var_os("CARGO_FEATURE_DEBUGMOZJS").is_some() {
        build.flag("-DDEBUG");
    }

    build
        .cpp(true)
        .std("c++20")
        .flags(flags)
        .include(&include_dir)
        .include(&mozjs_include)
        .warnings(false);

    if is_wasm {
        // The `cc` crate auto-links the C++ stdlib, but the wasm linker can't
        // find it without an explicit search path into the WASI sysroot.
        // Suppress the automatic link and provide our own static link instead.
        build.cpp_link_stdlib(None);

        let wasi_sdk = env::var("WASI_SDK_PATH")
            .expect("WASI_SDK_PATH must be set for wasm32-wasip2 builds");
        let sysroot_lib = Path::new(&wasi_sdk)
            .join("share/wasi-sysroot/lib/wasm32-wasip2");
        println!("cargo:rustc-link-search=native={}", sysroot_lib.display());
        println!("cargo:rustc-link-lib=static=c++");
        println!("cargo:rustc-link-lib=static=c++abi");

        build
            .flag(format!(
                "-include{mozjs_include}/js-confdefs.h"
            ))
            .flag("-Qunused-arguments")
            .flag("-mthread-model")
            .flag("single")
            .flag("-m32");
    }

    for source in &sources {
        build.file(source);
    }

    build.compile("cpp_support");

    // Re-run if any source or header changes.
    println!("cargo:rerun-if-changed={}", cpp_dir.display());
    println!("cargo:rerun-if-changed={}", include_dir.display());
    println!("cargo:rerun-if-changed={}", mozjs_include);
}
