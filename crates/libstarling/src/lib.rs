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
/// This registers all builtin globals (btoa, atob, etc.) and then delegates
/// to [`core_runtime::run()`] with a platform-appropriate event loop driver.
pub fn run(config: config::RuntimeConfig) -> Result<(), String> {
    apply_pre_init_config(&config)?;
}

/// Drive the event loop on native targets using a tokio current-thread
/// runtime with async timer support.
#[cfg(not(target_arch = "wasm32"))]
fn drive_event_loop(
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

/// Drive the event loop on wasm32 targets by spawning a WASIp3 async task.
#[cfg(target_arch = "wasm32")]
fn drive_event_loop(
    runtime: std::rc::Rc<runtime::Runtime>,
    mut invocation: invocation::InvocationState,
) -> Result<(), String> {
    wasip3::wit_bindgen::spawn(async move {
        // Enter the default global's realm for the lifetime of the event
        // loop. Tasks running inside `run_to_completion` (timers, promise
        // reactions, etc.) need an active realm to call into JS.
        let scope = runtime.default_global();
        // SAFETY: the scope (and its Runtime) outlive this future — the
        // spawned task owns both via the `Rc`.
        let raw_cx = unsafe { scope.raw_cx_no_gc() };
        let el = invocation.event_loop_mut();
        // SAFETY: raw_cx is valid — the runtime is kept alive by the Rc.
        unsafe {
            run_to_completion(raw_cx, el, |dur| async move {
                let nanos = dur.as_nanos().min(u64::MAX as u128) as u64;
                wasip3::clocks::monotonic_clock::wait_for(nanos).await;
            })
            .await;
        }
        drop(scope);
        runtime.unregister_invocation(&invocation);
    });
    Ok(())
}
