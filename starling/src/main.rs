// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! StarlingMonkey — a JavaScript runtime built on SpiderMonkey.
//!
//! Parses command-line arguments into a [`RuntimeConfig`](libstarling::config::RuntimeConfig)
//! and delegates execution to [`libstarling::run`]. The wasm build is the `starling` cdylib in
//! `lib.rs`. On wasm targets this binary is built but does nothing.

#[cfg(not(target_arch = "wasm32"))]
use std::process::exit;

#[cfg(not(target_arch = "wasm32"))]
use libstarling::config::RuntimeConfig;

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let config = match RuntimeConfig::from_args(std::env::args()) {
        Ok(config) => config,
        Err(e) => {
            let _ = e.print();
            exit(0);
        }
    };

    let _ = libstarling::run(config).map_err(|e| println!("{e}"));
}

#[cfg(target_arch = "wasm32")]
fn main() {
    unreachable!("On wasm targets the `starling` cdylib is the entry point, not this binary");
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[test]
fn cli_runs() {
    let config = libstarling::config::RuntimeConfig::from_args(
        ["starlingmonkey", "-e", "1 + 1"]
            .iter()
            .map(|s| s.to_string()),
    )
    .unwrap();
    libstarling::run(config)
        .map_err(|e| println!("{e}"))
        .expect("Run failed");
}
