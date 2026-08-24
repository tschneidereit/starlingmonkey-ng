// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! StarlingMonkey — a JavaScript runtime built on SpiderMonkey.
//!
//! Parses command-line arguments into a [`RuntimeConfig`](libstarling::config::RuntimeConfig)
//! and delegates execution to [`libstarling::run`].

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

/// The wasm component exports **both** `wasi:cli/run` and `wasi:http/handler`, so one build serves
/// HTTP and runs as a command. There is no dispatch between them: the host picks which export to
/// call — `wasmtime run` the former, `wasmtime serve` the latter.
///
/// The two differ only in where their configuration comes from. The CLI export is handed argv;
/// the HTTP export is not (`wasmtime serve` passes none), so it reads `STARLINGMONKEY_CONFIG`.
#[cfg(target_arch = "wasm32")]
mod wasm_entry {

    use libstarling::config::RuntimeConfig;

    struct StarlingCli;

    impl wasip3::exports::cli::run::Guest for StarlingCli {
        async fn run() -> Result<(), ()> {
            let config = match RuntimeConfig::from_args(std::env::args()) {
                Ok(config) => config,
                Err(e) => {
                    let _ = e.print();
                    return Err(());
                }
            };

            libstarling::run(config).await.map_err(|e| {
                eprintln!("{e}");
            })
        }
    }

    struct StarlingHttp;

    impl wasip3::exports::http::handler::Guest for StarlingHttp {
        async fn handle(
            request: wasip3::http::types::Request,
        ) -> Result<wasip3::http::types::Response, wasip3::http::types::ErrorCode> {
            libstarling::serve_wasm::handle(request).await
        }
    }

    wasip3::cli::command::export!(StarlingCli);
    wasip3::http::service::export!(StarlingHttp);

    /// Pre-initialization entry point for `wasmtime wizer`, which runs it and snapshots the
    /// initialized instance.
    ///
    /// Wizer calls a component-level function export, and a component exports only what its world
    /// declares — a bare `#[export_name]` does not survive componentization. So this declares the
    /// smallest possible world containing just that function; its fragment merges with the two
    /// `export!`s above into one component exporting all three.
    ///
    /// The export has to survive the snapshot (`--keep-init-func=true`): dropping it, which is
    /// wizer's default, leaves the component referring to a core export that is no longer there,
    /// and the result fails to load. See `scripts/test-wizer.sh`.
    mod wizer {
        wit_bindgen::generate!({
            inline: r#"
                package local:wizer;
                world wizer {
                    export wizer-initialize: async func();
                }
            "#,
            world: "wizer",
        });

        struct Init;

        impl Guest for Init {
            async fn wizer_initialize() {
                // A failure here would otherwise be baked into the snapshot as a runtime that
                // silently starts from scratch on every request.
                if let Err(e) = libstarling::serve_wasm::pre_initialize().await {
                    panic!("pre-initialization failed: {e}");
                }
            }
        }

        export!(Init);
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    unreachable!("On wasm32-wasip3, an exported Guest impl above is the entry point");
}

#[cfg(all(test, not(target_arch = "wasm32")))]
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
