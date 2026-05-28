// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

use core_runtime::event_loop::run_to_completion;

// Re-export everything from core-runtime.
pub use core_runtime::*;

/// Register all built-in global initializers.
///
/// This must be called before `Runtime::init()` to ensure built-in web
/// globals (like `btoa`, `atob`) are installed on every global object.
pub fn register_builtins() {
    runtime::register_global_initializer(web_globals::add_to_global);
    runtime::register_global_initializer(web_url::add_to_global);
    runtime::register_global_initializer(|scope, global| unsafe {
        cpp_builtins::install(scope.cx_mut().raw_cx(), global.handle());
    });
}

/// Apply CLI options that take effect before the runtime is initialized.
///
/// Currently this just installs the worker location URL parsed from
/// `--init-location`. Shared between the native and wasm32 entry points.
fn apply_pre_init_config(config: &config::RuntimeConfig) -> Result<(), String> {
    if let Some(location) = config.init_location.as_deref() {
        let url = url::Url::parse(location)
            .map_err(|e| format!("Invalid --init-location URL {location:?}: {e}"))?;
        web_globals::worker_location::set_init_location(url);
    }
    Ok(())
}

/// Run a JavaScript script or module on native targets.
///
/// Registers all builtin globals and then delegates to [`core_runtime::run()`]
/// with a tokio-based event loop driver.
#[cfg(not(target_arch = "wasm32"))]
pub fn run(config: config::RuntimeConfig) -> Result<(), String> {
    apply_pre_init_config(&config)?;
    register_builtins();
    core_runtime::run(config, drive_event_loop_native)
}

/// Run a JavaScript script or module on wasm32 targets.
///
/// Must be `.await`ed from inside an async-lifted `wasi:cli/run.run` task
/// — the event loop awaits WASIp3 async-lowered imports (e.g.
/// `monotonic_clock::wait_for`), and those are only valid inside an
/// async-lifted task. The cdylib in the `starling` package provides such a
/// task via `wasip3::cli::command::export!`.
#[cfg(target_arch = "wasm32")]
pub async fn run(config: config::RuntimeConfig) -> Result<(), String> {
    apply_pre_init_config(&config)?;
    register_builtins();

    let (runtime, mut invocation) = match core_runtime::setup(config)? {
        Some(pair) => pair,
        None => return Ok(()),
    };

    // Enter the default global's realm for the lifetime of the event loop.
    // Tasks running inside `run_to_completion` (timers, promise reactions,
    // etc.) need an active realm to call into JS.
    let scope = runtime.default_global();
    // SAFETY: `scope` (and the `Runtime` it borrows from) lives until the
    // explicit `drop(scope)` below.
    let raw_cx = unsafe { scope.raw_cx_no_gc() };
    let el = invocation.event_loop_mut();

    // SAFETY: `raw_cx` is valid for the duration of the await — `scope`
    // keeps the `Runtime` alive and the realm entered.
    unsafe {
        run_to_completion(raw_cx, el, |dur| async move {
            let nanos = dur.as_nanos().min(u64::MAX as u128) as u64;
            wasip3::clocks::monotonic_clock::wait_for(nanos).await;
        })
        .await;
    }

    drop(scope);
    runtime.unregister_invocation(&invocation);
    Ok(())
}

/// Drive the event loop on native targets using a tokio current-thread
/// runtime with async timer support.
#[cfg(not(target_arch = "wasm32"))]
fn drive_event_loop_native(
    runtime: std::rc::Rc<runtime::Runtime>,
    mut invocation: invocation::InvocationState,
) -> Result<(), String> {
    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| format!("Failed to create tokio runtime: {e}"))?;

    // Enter the default global's realm for the lifetime of the event loop.
    // Tasks running inside `run_to_completion` (timers, promise reactions,
    // etc.) need an active realm to call into JS.
    let scope = runtime.default_global();
    let el = invocation.event_loop_mut();

    tokio_rt.block_on(async {
        // SAFETY: the scope (and its Runtime) must outlive this future.
        // Since the caller owns both and `block_on` runs synchronously on
        // the same thread, this is guaranteed.
        let raw_cx = unsafe { scope.raw_cx_no_gc() };
        unsafe { run_to_completion(raw_cx, el, tokio::time::sleep).await }
    });

    drop(scope);
    runtime.unregister_invocation(&invocation);
    Ok(())
}
