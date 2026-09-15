// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! The wasm build of StarlingMonkey: a reactor exporting `wasi:cli/run`,
//! `wasi:http/handler` and `wizer-initialize`.
//!
//! This is a `cdylib` rather than the `starlingmonkey` binary because a bin
//! links `crt1-command.o`, which exports `wasi:cli/run` itself. On
//! wasm32-wasip3 that collides with the `export!`s below, and componentization
//! fails. A `cdylib` links no such startup object.

#![cfg(target_arch = "wasm32")]

/// Runs the static constructors at most once.
///
/// `--wrap=__wasm_call_ctors` (see `build.rs`) redirects every reference to
/// `__wasm_call_ctors` here, so every path that runs the constructors shares
/// one guard. `wit_bindgen::rt::run_ctors_once` guards only its own static, and
/// at least one other path runs them as well: without the wrap, the first
/// `wasi:cli/run` call aborts in SpiderMonkey's `InitializeUptime` with "Must
/// not be called more than once".
#[unsafe(no_mangle)]
pub extern "C" fn __wrap___wasm_call_ctors() {
    static mut RAN: bool = false;
    // SAFETY: wasm32 is single-threaded, and the constructors this guards run
    // before any code that could call it reentrantly.
    unsafe {
        if RAN {
            return;
        }
        RAN = true;
        unsafe extern "C" {
            fn __real___wasm_call_ctors();
        }
        __real___wasm_call_ctors();
    }
}

/// The wasm component exports both `wasi:cli/run` and `wasi:http/handler`, so one build serves
/// HTTP and runs as a command. There is no dispatch between them: the host picks which export to
/// call. `wasmtime run` calls the former, `wasmtime serve` the latter.
///
/// The two differ only in where their configuration comes from. The CLI export is handed argv;
/// the HTTP export is not (`wasmtime serve` passes none), so it reads `STARLINGMONKEY_CONFIG`.
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
    /// declares. A bare `#[export_name]` does not survive componentization, so this declares the
    /// smallest possible world containing just that function. Its fragment merges with the two
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
