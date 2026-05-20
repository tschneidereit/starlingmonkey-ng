// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

pub mod config;
pub mod event_loop;
pub mod invocation;
pub mod module;
pub mod runtime;

pub mod test_util;

use js::error::ExnThrown;
pub use js::macros::{
    jsclass, jsglobals, jsmethods, jsmodule, jsnamespace, webidl_dictionary, webidl_interface,
    webidl_methods, webidl_namespace, webidl_union, Traceable,
};

use crate::runtime::Runtime;

/// Run a JavaScript script or module based on the provided configuration.
///
/// This is the main entry point for the StarlingMonkey runtime. It:
/// 1. Initializes the SpiderMonkey JS engine (once per process)
/// 2. Creates a global object and realm
/// 3. Installs builtin globals and modules.
/// 4. Optionally runs an initializer script
/// 5. Executes the content script (from `--eval` or a file path)
///    in either ES module mode (default) or legacy script mode
/// 6. Delegates to `drive_event_loop` to run the event loop.
///
/// The `drive_event_loop` callback receives the `Runtime` and
/// `InvocationState` and is responsible for driving the event loop to
/// completion using whatever executor and timer mechanism the embedding
/// provides. It must call `runtime.unregister_invocation()` when done.
///
/// On native targets, the callback typically creates an async runtime, e.g.
/// Tokio, and calls `block_on(run_to_completion(..., tokio::time::sleep))`.
/// On WASIp3, it spawns the event loop via `wit_bindgen::spawn`.
pub fn run(
    config: config::RuntimeConfig,
    drive_event_loop: impl FnOnce(
        std::rc::Rc<Runtime>,
        invocation::InvocationState,
    ) -> Result<(), String>,
) -> Result<(), String> {
    let runtime = Runtime::init(&config);

    // Determine source and filename.
    let (source, filename) = if let Some(ref eval) = config.eval_script {
        (eval.clone(), "<eval>".to_string())
    } else {
        let path = &config.script_path;
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(e) => {
                return Err(format!("Error reading script '{}': {}", path, e));
            }
        };
        (source, path.clone())
    };

    // Create a per-invocation state with its own event loop and register
    // it with the runtime so the GC can trace it.
    let mut invocation = invocation::InvocationState::new();

    // SAFETY: `invocation` lives on this stack frame (or is moved into
    // the spawned wasm32 task which keeps it alive) and won't be freed
    // until we unregister below (or the spawned task unregisters it).
    unsafe { runtime.register_invocation(&invocation) };

    // Evaluate the script and drain initial microtasks inside a scope
    // block. The scope borrows `runtime` (via Rc::deref), so it must
    // be dropped before we can move `runtime` into the wasm32 spawn path.
    {
        let scope = runtime.default_global();

        let eval_result = unsafe {
            event_loop::with_event_loop(invocation.event_loop_mut(), |_| {
                if config.module_mode() {
                    module::evaluate_module(&scope, &source, &filename)
                } else {
                    js::compile::evaluate_with_filename(&scope, &source, &filename, 1)
                }
            })
        };

        if eval_result.is_err() {
            runtime.unregister_invocation(&invocation);
            let exn = ExnThrown::capture(&scope);
            println!("exn: {exn}");
            return Err(format!("Script evaluation failed with error {exn}"));
        }

        // Always drain microtasks first — promise reactions (e.g. from
        // `Promise.resolve().then(...)`) must run even if no event-loop
        // tasks are queued.
        event_loop::run_microtasks(&scope);
    }

    if !invocation.event_loop().is_alive() {
        runtime.unregister_invocation(&invocation);
        return Ok(());
    }

    drive_event_loop(runtime, invocation)
}

/// Extract and print the pending JS exception, if any.
///
/// # Safety
///
/// Called from within an active realm context.
pub unsafe fn report_pending_exception(scope: &js::gc::scope::Scope<'_>) {
    use js::exception;

    if !exception::is_pending(scope) {
        eprintln!("Error: script execution failed (no exception details available)");
        return;
    }

    let exc_val = match exception::get_pending(scope) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("Error: script execution failed (could not retrieve exception)");
            return;
        }
    };
    exception::clear(scope);

    // Try to convert the exception to a string.
    // TODO: use mozjs's better abstractions for this.
    match js::JSString::from_value(scope, exc_val) {
        Ok(js_str) => match js_str.to_utf8(scope) {
            Ok(msg) => eprintln!("Error: {}", msg),
            Err(_) => eprintln!("Error: script execution failed"),
        },
        Err(_) => {
            eprintln!("Error: script execution failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::config::RuntimeConfig;
    use super::*;
    use crate::test_util::test_tempdir;

    fn config_from(args: &[&str]) -> RuntimeConfig {
        RuntimeConfig::from_args(args.iter().map(|s| s.to_string())).unwrap()
    }

    /// Dummy event loop driver for tests whose scripts don't use timers
    /// or async work. If the event loop is alive, something unexpected
    /// happened.
    fn noop_driver(
        runtime: std::rc::Rc<runtime::Runtime>,
        invocation: invocation::InvocationState,
    ) -> Result<(), String> {
        assert!(
            !invocation.event_loop().is_alive(),
            "Event loop should not be alive"
        );
        runtime.unregister_invocation(&invocation);
        Ok(())
    }

    #[test]
    fn run_eval_module_mode() {
        let config = config_from(&["starling", "-e", "globalThis._x = 1 + 2;"]);
        assert!(config.module_mode());
        run(config, noop_driver)
            .map_err(|e| println!("{e}"))
            .expect("Run failed");
    }

    #[test]
    fn run_eval_legacy_script() {
        let config = config_from(&["starling", "-e", "var x = 42;", "--legacy-script"]);
        assert!(!config.module_mode());
        run(config, noop_driver)
            .map_err(|e| println!("{e}"))
            .expect("Run failed");
    }

    #[test]
    fn run_file_module_mode() {
        let dir = test_tempdir();
        let script = dir.path().join("test.mjs");
        std::fs::write(&script, "const x = 1 + 2;\n").unwrap();

        let config = config_from(&["starling", &script.to_string_lossy()]);
        run(config, noop_driver)
            .map_err(|e| println!("{e}"))
            .expect("Run failed");
    }

    #[test]
    fn run_file_legacy_script() {
        let dir = test_tempdir();
        let script = dir.path().join("test.js");
        std::fs::write(&script, "var x = 1 + 2;\n").unwrap();

        let config = config_from(&["starling", &script.to_string_lossy(), "--legacy-script"]);
        run(config, noop_driver)
            .map_err(|e| println!("{e}"))
            .expect("Run failed");
    }

    #[test]
    fn run_file_with_imports() {
        let dir = test_tempdir();
        std::fs::write(dir.path().join("helper.js"), "export const V = 10;\n").unwrap();
        let entry = dir.path().join("main.mjs");
        std::fs::write(
            &entry,
            r#"import { V } from "./helper.js"; globalThis._v = V;"#,
        )
        .unwrap();

        let config = config_from(&["starling", &entry.to_string_lossy()]);
        run(config, noop_driver)
            .map_err(|e| println!("{e}"))
            .expect("Run failed");
    }

    #[test]
    fn run_with_initializer_script() {
        let dir = test_tempdir();
        let init = dir.path().join("init.js");
        std::fs::write(&init, "globalThis._initialized = true;\n").unwrap();
        let main = dir.path().join("main.mjs");
        std::fs::write(&main, "const ok = globalThis._initialized;\n").unwrap();

        let config = config_from(&[
            "starling",
            &main.to_string_lossy(),
            "-i",
            &init.to_string_lossy(),
        ]);
        run(config, noop_driver)
            .map_err(|e| println!("{e}"))
            .expect("Run failed");
    }
}
