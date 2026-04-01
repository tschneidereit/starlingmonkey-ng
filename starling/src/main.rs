// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! StarlingMonkey — a JavaScript runtime built on SpiderMonkey.
//!
//! Parses command-line arguments into a [`RuntimeConfig`](libstarling::config::RuntimeConfig)
//! and delegates execution to [`libstarling::run`].

use std::process::exit;

use libstarling::config::RuntimeConfig;

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

#[test]
fn cli_runs() {
    let config = libstarling::config::RuntimeConfig::from_args(
        ["starling", "-e", "1 + 1"].iter().map(|s| s.to_string()),
    )
    .unwrap();
    libstarling::run(config)
        .map_err(|e| println!("{e}"))
        .expect("Run failed");
}
