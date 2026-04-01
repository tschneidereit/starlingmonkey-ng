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

    if config.wpt_mode {
        register_wpt_builtins();
    }
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

/// Register WPT (Web Platform Tests) support globals (`evalScript`, etc.).
///
/// This must be called before `Runtime::init()` when running in WPT mode.
pub fn register_wpt_builtins() {
    libstarling::runtime::register_global_initializer(wpt_support::add_to_global);
}
