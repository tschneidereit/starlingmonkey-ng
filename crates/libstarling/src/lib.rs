// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

// Re-export everything from core-runtime.
pub use core_runtime::*;

/// Register all built-in global initializers.
///
/// This must be called before `Runtime::init()` to ensure built-in web
/// globals (like `btoa`, `atob`) are installed on every global object.
pub fn register_builtins() {
    runtime::register_global_initializer(web_globals::add_to_global);
    runtime::register_global_initializer(|scope, global| unsafe {
        cpp_builtins::install(scope.cx_mut().raw_cx(), global.handle());
    });
}

/// Register WPT (Web Platform Tests) support globals (`evalScript`, etc.).
///
/// This must be called before `Runtime::init()` when running in WPT mode.
pub fn register_wpt_builtins() {
    runtime::register_global_initializer(web_globals::wpt_support::add_to_global);
}

/// Run a JavaScript script or module based on the provided configuration.
///
/// This registers all builtin globals (btoa, atob, etc.) and then delegates
/// to [`core_runtime::run()`]. When `config.wpt_mode` is true, WPT-specific
/// globals like `evalScript` are also installed.
pub fn run(config: config::RuntimeConfig) {
    register_builtins();
    if config.wpt_mode {
        register_wpt_builtins();
    }
    core_runtime::run(config);
}
