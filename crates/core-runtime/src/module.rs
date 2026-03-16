// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Native ES module support for SpiderMonkey.
//!
//! This module provides an ergonomic way to define ES modules backed by native
//! Rust functions and values, inspired by rquickjs's `ModuleDef` pattern but
//! built on SpiderMonkey's module compilation pipeline.
//!
//! # Strategy
//!
//! SpiderMonkey has no "synthetic module" API — every module must start as JS
//! source text that is compiled with `CompileModule`. The approach here is:
//!
//! 1. Generate JS source with `export var name;` declarations
//! 2. `CompileModule` → `ModuleLink` → `ModuleEvaluate`
//! 3. Retrieve the module environment via `GetModuleEnvironment`
//! 4. Populate it with native values/functions using `JS_SetProperty` /
//!    `JS_DefineFunction`
//!
//! A module resolve hook maps specifier strings to compiled module objects
//! via a thread-local registry.
//!
//! # Example
//!
//! ```rust,ignore
//! #[::core_runtime::jsmodule]
//! mod my_math {
//!     pub const PI: f64 = 3.14159;
//!     pub fn add(a: f64, b: f64) -> f64 { a + b }
//! }
//!
//! // Register and use:
//! let rt = Runtime::init(&config);
//! let scope = rt.default_global();
//! unsafe {
//!     register_module::<my_math::js_module>(&scope);
//!     // JS can now: import { PI, add } from "my_math";
//! }
//! ```

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;

use js::conversion::ToJSVal;
use js::heap::{Heap, Trace};
use js::module_raw::{transform_str_to_source_text, CompileOptionsWrapper, SetModulePrivate};
use js::native::{HandleObject, JSContext, JSNative, JSObject, JSString, JSTracer, Value};
use js::prelude::RootScope;
use js::{value, Object};
use oxc_resolver::{ResolveOptions, Resolver};

// ============================================================================
// Module export descriptors
// ============================================================================

/// Describes a single export from a native module.
pub enum ModuleExport {
    /// A native function export.
    Function {
        /// Name as it appears in JS (the export name).
        js_name: &'static str,
        /// The JSNative callback implementing the function.
        native: JSNative,
        /// Number of expected arguments.
        nargs: u32,
    },
    /// A value export (constant). The value is set by the `evaluate` callback.
    Value {
        /// Name as it appears in JS (the export name).
        js_name: &'static str,
    },
}

// ============================================================================
// NativeModule trait
// ============================================================================

/// Trait for types that define a native ES module.
///
/// Implement this trait (usually via `#[jsmodule]`) to expose Rust
/// functions and constants as ES module exports.
pub trait NativeModule: 'static {
    /// The module specifier string used in JS `import` statements.
    /// e.g. `"my_math"` for `import { add } from "my_math";`
    const NAME: &'static str;

    /// Return the list of exports this module provides.
    fn declarations() -> Vec<ModuleExport>;

    /// Populate the module environment with native values.
    ///
    /// Called after `ModuleEvaluate` — the module environment object is
    /// passed in so you can set property values for `Value` exports.
    /// Function exports are set up automatically before this is called.
    ///
    /// # Safety
    ///
    /// `scope` must be valid. `env` is the module environment object.
    unsafe fn evaluate(scope: &js::gc::scope::Scope<'_>, env: HandleObject) -> bool;
}

// ============================================================================
// Module registry (thread-local)
// ============================================================================

/// A cached compiled module object, stored in a `Heap` so SpiderMonkey's
/// moving GC can update the pointer during compaction.
/// Traced by `trace_module_registry`, so allowed to contain unrooted interior.
#[js::allow_unrooted_interior]
struct ModuleEntry {
    module_obj: Box<Heap<*mut JSObject>>,
}

thread_local! {
    static MODULE_REGISTRY: RefCell<HashMap<String, ModuleEntry>> = RefCell::new(HashMap::new());

    /// The resolver instance, created once per thread via `init_module_loader`.
    static RESOLVER: RefCell<Option<Resolver>> = const { RefCell::new(None) };

    /// Fallback base directory for the entry module (before any module objects exist).
    static BASE_PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Trace all cached module objects.
///
/// # Safety
///
/// `trc` must be a valid `JSTracer` pointer provided by SpiderMonkey's GC.
unsafe fn trace_module_registry(trc: *mut JSTracer) {
    MODULE_REGISTRY.with(|reg| {
        for entry in reg.borrow().values() {
            entry.module_obj.trace(trc);
        }
    });
}

/// C-compatible trampoline for [`trace_module_registry`].
unsafe extern "C" fn trace_module_registry_cb(trc: *mut JSTracer, _data: *mut std::ffi::c_void) {
    trace_module_registry(trc);
}

/// Register the module registry as a GC root tracer.
///
/// Called automatically by `Runtime::init` — only needed when using a
/// raw mozjs `Runtime` directly.
pub fn init_module_gc_tracer(cx: &mut js::native::JSContext) {
    unsafe {
        js::gc::add_extra_gc_roots_tracer(cx, Some(trace_module_registry_cb), ptr::null_mut())
    };
}

/// Remove the module registry GC root tracer.
pub fn remove_module_gc_tracer(cx: &js::native::JSContext) {
    unsafe {
        js::gc::remove_extra_gc_roots_tracer(cx, Some(trace_module_registry_cb), ptr::null_mut())
    };
}

// ============================================================================
// Module resolve hook
// ============================================================================

/// The module resolve hook called by SpiderMonkey when processing `import`.
///
/// Resolution strategy:
/// 1. Check the native module registry for an exact specifier match
/// 2. Fall back to filesystem resolution using `oxc_resolver`
#[js::allow_unrooted]
unsafe extern "C" fn module_resolve_hook(
    cx: *mut js::native::RawJSContext,
    _referencing_private: js::native::RawHandle<Value>,
    module_request: js::native::RawHandle<*mut JSObject>,
) -> *mut JSObject {
    // Extract the specifier string from the ModuleRequest object
    let specifier_str =
        unsafe { js::module_raw::GetModuleRequestSpecifier(cx as _, module_request) };
    if specifier_str.is_null() {
        return ptr::null_mut();
    }

    let specifier = match unsafe { jsstring_to_string(cx, specifier_str) } {
        Some(s) => s,
        None => return ptr::null_mut(),
    };

    // 1. Check module registry (covers both native and file-backed modules)
    let cached = MODULE_REGISTRY.with(|reg| {
        reg.borrow()
            .get(&specifier)
            .map(|entry| entry.module_obj.get())
    });
    if let Some(obj) = cached {
        return obj;
    }

    // 2. Resolve via filesystem using oxc_resolver
    match resolve_file_module(cx, &specifier) {
        Ok(obj) => obj,
        Err(msg) => {
            let c_msg = CString::new(msg)
                .unwrap_or_else(|_| CString::new("Module resolution failed").unwrap());
            // SAFETY: cx is a valid RawJSContext from the resolve hook.
            let mut js_cx = unsafe {
                js::native::JSContext::from_ptr(std::ptr::NonNull::new_unchecked(cx as _))
            };
            js::error::report_error_ascii(&mut js_cx, &c_msg);
            ptr::null_mut()
        }
    }
}

/// Resolve a specifier to a file on disk, compile it as a module, and cache it.
///
/// Only *compiles* the module — linking and evaluation are handled by
/// SpiderMonkey's module pipeline (the caller of the resolve hook).
///
/// # Safety
///
/// `cx` must be a valid JSContext pointer. Called from the resolve hook.
#[js::allow_unrooted]
unsafe fn resolve_file_module(
    cx: *mut js::native::RawJSContext,
    specifier: &str,
) -> Result<*mut JSObject, String> {
    // Determine the base directory for resolution.
    let base_dir = BASE_PATH
        .with(|bp| bp.borrow().clone())
        .ok_or_else(|| format!("Module '{}' not found (no base path configured)", specifier))?;

    // Resolve using oxc_resolver
    let resolved_path = RESOLVER.with(|r| {
        let borrow = r.borrow();
        let resolver = borrow
            .as_ref()
            .expect("resolver not initialized — call init_module_loader first");
        resolver
            .resolve(&base_dir, specifier)
            .map(|res| res.path().to_path_buf())
            .map_err(|e| format!("Cannot resolve module '{}': {}", specifier, e))
    })?;

    let canonical_key = resolved_path.to_string_lossy().to_string();

    // Check if already compiled under the canonical path
    let cached = MODULE_REGISTRY.with(|reg| {
        reg.borrow()
            .get(&canonical_key)
            .map(|entry| entry.module_obj.get())
    });
    if let Some(obj) = cached {
        return Ok(obj);
    }

    // Read source from disk
    let source = std::fs::read_to_string(&resolved_path)
        .map_err(|e| format!("Failed to read '{}': {}", resolved_path.display(), e))?;

    // Compile (but do NOT link or evaluate — SpiderMonkey handles that)
    let c_filename =
        CString::new(canonical_key.as_bytes()).map_err(|_| "Invalid filename".to_string())?;
    let options = CompileOptionsWrapper::new_raw(cx as _, c_filename, 1);
    let mut src = transform_str_to_source_text(&source);
    let module_obj = unsafe { js::module_raw::CompileModule1(cx as _, options.ptr, &mut src) };
    if module_obj.is_null() {
        return Err(format!(
            "Failed to compile module '{}'",
            resolved_path.display()
        ));
    }

    // Set the module private so the resolve hook receives a valid
    // referencing module when this module's imports are resolved.
    let private = unsafe { value::from_object(module_obj) };
    unsafe { SetModulePrivate(module_obj, &private) };
    MODULE_REGISTRY.with(|reg| {
        let mut reg = reg.borrow_mut();
        reg.insert(
            canonical_key.clone(),
            ModuleEntry {
                module_obj: Heap::boxed(module_obj),
            },
        );
        if specifier != canonical_key {
            reg.insert(
                specifier.to_string(),
                ModuleEntry {
                    module_obj: Heap::boxed(module_obj),
                },
            );
        }
    });

    Ok(module_obj)
}

/// Convert a JSString to a Rust String.
unsafe fn jsstring_to_string(
    cx: *mut js::native::RawJSContext,
    s: *mut JSString,
) -> Option<String> {
    use js::conversion::jsstr_to_string;
    use std::ptr::NonNull;
    let mut js_cx = JSContext::from_ptr(NonNull::new_unchecked(cx));
    let scope = RootScope::from_current_realm(&mut js_cx);
    NonNull::new(s).map(|nn| jsstr_to_string(&scope, nn))
}

// ============================================================================
// Public API
// ============================================================================

/// Install the module resolve hook and configure the filesystem resolver.
///
/// `rt` is the raw `JSRuntime` pointer on which the resolve hook is installed.
/// `base_path` is the directory used as the starting point for resolving
/// import specifiers (typically the directory containing the entry script).
///
/// This must be called once before any modules are registered or imported.
///
/// # Safety
///
/// `rt` must be a valid `*mut JSRuntime`.
pub unsafe fn init_module_loader(rt: *mut js::native::JSRuntime, base_path: PathBuf) {
    BASE_PATH.with(|bp| *bp.borrow_mut() = Some(base_path));

    unsafe { js::module::set_module_resolve_hook(rt, Some(module_resolve_hook)) };

    RESOLVER.with(|r| {
        *r.borrow_mut() = Some(Resolver::new(ResolveOptions {
            extensions: vec![".js".into(), ".mjs".into(), ".json".into()],
            ..ResolveOptions::default()
        }));
    });
}

/// Clear all module state (registry, resolver, base path).
///
/// Must be called while the `JSContext` is still alive, because
/// `Heap::drop()` fires GC write barriers. Called automatically
/// by `Runtime::drop`.
pub fn clear_module_state() {
    MODULE_REGISTRY.with(|reg| reg.borrow_mut().clear());
    BASE_PATH.with(|bp| *bp.borrow_mut() = None);
    RESOLVER.with(|r| *r.borrow_mut() = None);
}

/// Register a native module, making it available for `import` from JS.
///
/// This:
/// 1. Generates JS source with `export var ...;` for each declaration
/// 2. Compiles it as a module via `CompileModule`
/// 3. Links and evaluates the module
/// 4. Populates the module environment with native functions and values
/// 5. Stores the module in the thread-local registry for the resolve hook
///
/// # Safety
///
/// - [`init_module_loader`] must have been called first.
pub unsafe fn register_module<T: NativeModule>(scope: &js::gc::scope::Scope<'_>) -> bool {
    let declarations = T::declarations();

    // 1. Generate JS module source: `export var name1; export var name2; ...`
    let mut source = String::new();
    for decl in &declarations {
        let name = match decl {
            ModuleExport::Function { js_name, .. } => js_name,
            ModuleExport::Value { js_name } => js_name,
        };
        source.push_str(&format!("export var {};\n", name));
    }

    // 2. Compile module
    let filename = CString::new(T::NAME).unwrap();
    let options = CompileOptionsWrapper::new(scope.cx_mut(), filename, 1);

    let mut src = transform_str_to_source_text(&source);
    let module = match unsafe { js::module::compile_module(scope, options.ptr, &mut src) } {
        Ok(m) => m,
        Err(_) => return false,
    };

    // Set the module private so the resolve hook receives a valid
    // referencing module when this module's imports are resolved.
    let private = unsafe { value::from_object(module.as_raw()) };
    unsafe { SetModulePrivate(module.as_raw(), &private) };

    // 3. Store in registry before linking (resolve hook must find it)
    MODULE_REGISTRY.with(|reg| {
        reg.borrow_mut().insert(
            T::NAME.to_string(),
            ModuleEntry {
                module_obj: Heap::boxed(module.as_raw()),
            },
        );
    });

    // 4. Link
    if js::module::link(scope, module).is_err() {
        MODULE_REGISTRY.with(|reg| {
            reg.borrow_mut().remove(T::NAME);
        });
        return false;
    }

    // 5. Evaluate (runs the `export var ...` initializations)
    if js::module::evaluate(scope, module).is_err() {
        MODULE_REGISTRY.with(|reg| {
            reg.borrow_mut().remove(T::NAME);
        });
        return false;
    }

    // 6. Get the module environment and populate it
    let env = unsafe {
        Object::from_raw(
            scope,
            js::module_raw::GetModuleEnvironment(scope.cx_mut(), module.handle()),
        )
    };
    let env = match env {
        Some(e) => e,
        None => return false,
    };

    // Set up function exports by creating functions and setting them as properties
    for decl in &declarations {
        if let ModuleExport::Function {
            js_name,
            native,
            nargs,
        } = decl
        {
            let c_name = CString::new(*js_name).unwrap();
            let func = match js::Function::new(scope, *native, *nargs, 0, &c_name) {
                Ok(f) => f,
                Err(_) => return false,
            };
            let func_val = scope.root_value(func.as_value());
            if env.set_property(scope, &c_name, func_val).is_err() {
                return false;
            }
        }
    }

    // Let the module implementation set value exports
    if !T::evaluate(scope, env.handle()) {
        return false;
    }

    true
}

/// Evaluate a JS script as a module, with access to registered native modules.
///
/// This compiles the given source as a module, links it (the resolve hook
/// will find registered native modules and resolve file imports), and evaluates it.
///
/// The `filename` is used both as the script origin for error messages and
/// (if it's a real filesystem path) as the base for resolving relative imports.
///
/// # Safety
///
/// - `cx` must be a valid `JSContext` pointer.
/// - [`init_module_loader`] must have been called first.
#[allow(clippy::result_unit_err)]
pub unsafe fn evaluate_module(
    scope: &js::gc::scope::Scope<'_>,
    source: &str,
    filename: &str,
) -> Result<(), ()> {
    let c_filename = CString::new(filename).unwrap();
    let options = CompileOptionsWrapper::new(scope.cx_mut(), c_filename, 1);

    let mut src = transform_str_to_source_text(source);
    let module =
        unsafe { js::module::compile_module(scope, options.ptr, &mut src) }.map_err(|_| ())?;

    // Set the module private so the resolve hook receives a valid
    // referencing module when this module's imports are resolved.
    let private = unsafe { value::from_object(module.as_raw()) };
    unsafe { SetModulePrivate(module.as_raw(), &private) };

    // If the filename is a real path, update the base path for relative
    // imports. The empty-path guard prevents WASI from treating
    // `Path::new("").exists()` as a valid root directory.
    let path = Path::new(filename);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && parent.exists() {
            let abs_parent = if parent.is_absolute() {
                parent.to_path_buf()
            } else {
                std::env::current_dir().unwrap_or_default().join(parent)
            };
            BASE_PATH.with(|bp| *bp.borrow_mut() = Some(abs_parent));
        }
    }

    js::module::link(scope, module).map_err(|_| ())?;
    js::module::evaluate(scope, module).map_err(|_| ())?;

    Ok(())
}

/// Helper to set a value export on a module environment object.
///
/// Used by generated `evaluate` implementations from `#[jsmodule]`.
///
/// # Safety
///
/// - `cx` must be valid.
/// - `env` must be a valid module environment object.
pub unsafe fn set_module_export<'s, V: ToJSVal<'s> + ?Sized>(
    scope: &'s js::gc::scope::Scope<'_>,
    env: HandleObject,
    name: &str,
    value: &V,
) -> bool {
    let c_name = CString::new(name).unwrap();
    let val = match value.to_jsval(scope) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let env_obj = js::Object::from_handle(env).expect("module environment object is null");
    env_obj.set_property(scope, &c_name, val).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RuntimeConfig;
    use crate::runtime::Runtime;
    use crate::test_util::test_tempdir;
    use js::compile::evaluate_with_filename;
    use js::prelude::FromJSVal;

    /// Create a `Runtime` for testing with module support.
    ///
    /// Uses a minimal eval config so no filesystem script path is needed.
    /// The returned `Rc<Runtime>` cleans up all state on drop.
    fn test_runtime() -> std::rc::Rc<Runtime> {
        let config =
            RuntimeConfig::from_args(["starling", "-e", "42"].iter().map(|s| s.to_string()))
                .unwrap();
        Runtime::init(&config)
    }

    /// Helper: read back a globalThis property as f64.
    /// Handles both SpiderMonkey int32 and double representations.
    fn read_global_f64(scope: &js::gc::scope::Scope<'_>, expr: &str) -> f64 {
        let rval = evaluate_with_filename(scope, expr, "test_read.js", 1)
            .expect("evaluate_with_filename failed");
        if rval.is_double() {
            rval.to_double()
        } else if rval.is_int32() {
            rval.to_int32() as f64
        } else {
            panic!("expected number, got neither double nor int32");
        }
    }

    /// Helper: read back a globalThis property as String.
    fn read_global_string(scope: &js::gc::scope::Scope<'_>, expr: &str) -> String {
        let rval = evaluate_with_filename(scope, expr, "test_read.js", 1)
            .expect("evaluate_with_filename failed");
        assert!(rval.is_string());
        String::from_jsval(scope, rval, ()).expect("string conversion failed")
    }

    #[test]
    fn evaluate_module_inline() {
        let rt = test_runtime();
        let scope = rt.default_global();
        unsafe {
            let result = evaluate_module(&scope, "globalThis._moduleTest = 42;", "test_inline.mjs");
            assert!(result.is_ok());
            assert_eq!(read_global_f64(&scope, "globalThis._moduleTest"), 42.0);
        }
    }

    #[test]
    fn evaluate_module_syntax_error_fails() {
        let rt = test_runtime();
        let scope = rt.default_global();
        unsafe {
            let result = evaluate_module(&scope, "this is not valid JS {{{", "bad.mjs");
            assert!(result.is_err());
        }
    }

    #[test]
    fn resolve_file_import() {
        let dir = test_tempdir();
        let module_path = dir.path().join("helper.js");
        std::fs::write(&module_path, "export const VALUE = 99;\n").unwrap();

        let rt = test_runtime();
        rt.reset_module_loader(dir.path().to_path_buf());
        let scope = rt.default_global();
        unsafe {
            let result = evaluate_module(
                &scope,
                r#"
                    import { VALUE } from "./helper.js";
                    globalThis._imported = VALUE;
                "#,
                "entry.mjs",
            );
            assert!(result.is_ok(), "module evaluation failed");
            assert_eq!(read_global_f64(&scope, "globalThis._imported"), 99.0);
        }
    }

    #[test]
    fn resolve_nested_imports() {
        let dir = test_tempdir();

        // a.js imports from b.js, b.js imports from c.js
        std::fs::write(dir.path().join("c.js"), "export const BASE = 10;\n").unwrap();
        std::fs::write(
            dir.path().join("b.js"),
            r#"
                import { BASE } from "./c.js";
                export const DOUBLED = BASE * 2;
            "#,
        )
        .unwrap();

        let rt = test_runtime();
        rt.reset_module_loader(dir.path().to_path_buf());
        let scope = rt.default_global();
        unsafe {
            let result = evaluate_module(
                &scope,
                r#"
                    import { DOUBLED } from "./b.js";
                    globalThis._nested = DOUBLED;
                "#,
                &dir.path().join("entry.mjs").to_string_lossy(),
            );
            assert!(result.is_ok(), "nested module evaluation failed");
            assert_eq!(read_global_f64(&scope, "globalThis._nested"), 20.0);
        }
    }

    #[test]
    fn resolve_missing_module_fails() {
        let dir = test_tempdir();

        let rt = test_runtime();
        rt.reset_module_loader(dir.path().to_path_buf());
        let scope = rt.default_global();
        unsafe {
            let result = evaluate_module(
                &scope,
                r#"import { x } from "./nonexistent.js";"#,
                "entry.mjs",
            );
            assert!(result.is_err());
        }
    }

    #[test]
    fn resolve_with_extension_inference() {
        let dir = test_tempdir();
        std::fs::write(dir.path().join("utils.js"), "export const PI = 3.14;\n").unwrap();

        let rt = test_runtime();
        rt.reset_module_loader(dir.path().to_path_buf());
        let scope = rt.default_global();
        unsafe {
            // Import without .js extension — oxc_resolver should add it
            let result = evaluate_module(
                &scope,
                r#"
                    import { PI } from "./utils";
                    globalThis._pi = PI;
                "#,
                "entry.mjs",
            );
            assert!(result.is_ok(), "extension inference failed");
            assert_eq!(read_global_f64(&scope, "globalThis._pi"), 3.14);
        }
    }

    #[test]
    fn resolve_index_file() {
        let dir = test_tempdir();
        let sub = dir.path().join("mylib");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("index.js"), "export const HELLO = 'world';\n").unwrap();

        let rt = test_runtime();
        rt.reset_module_loader(dir.path().to_path_buf());
        let scope = rt.default_global();
        unsafe {
            let result = evaluate_module(
                &scope,
                r#"
                    import { HELLO } from "./mylib";
                    globalThis._hello = HELLO;
                "#,
                "entry.mjs",
            );
            assert!(result.is_ok(), "index.js resolution failed");
            assert_eq!(read_global_string(&scope, "globalThis._hello"), "world");
        }
    }

    #[test]
    fn duplicate_import_uses_cache() {
        let dir = test_tempdir();

        // A module with a side-effect counter
        std::fs::write(
            dir.path().join("counter.js"),
            r#"
                if (!globalThis._counter) globalThis._counter = 0;
                globalThis._counter++;
                export const dummy = 1;
            "#,
        )
        .unwrap();

        let rt = test_runtime();
        rt.reset_module_loader(dir.path().to_path_buf());
        let scope = rt.default_global();
        unsafe {
            // Two separate entry modules both import counter.js
            let r1 = evaluate_module(
                &scope,
                r#"import { dummy } from "./counter.js";"#,
                "entry1.mjs",
            );
            assert!(r1.is_ok());
            // Second import of same module should be cached (not re-evaluated)
            let r2 = evaluate_module(
                &scope,
                r#"import { dummy } from "./counter.js";"#,
                "entry2.mjs",
            );
            assert!(r2.is_ok());
            assert_eq!(read_global_f64(&scope, "globalThis._counter"), 1.0);
        }
    }

    #[test]
    fn file_and_native_modules_coexist() {
        // Define a minimal native module inline (without the #[jsmodule] macro,
        // which generates code referencing the crate externally).
        struct TestNative;
        impl NativeModule for TestNative {
            const NAME: &'static str = "test_native";
            fn declarations() -> Vec<ModuleExport> {
                vec![ModuleExport::Value {
                    js_name: "NATIVE_VAL",
                }]
            }
            unsafe fn evaluate(scope: &js::gc::scope::Scope<'_>, env: HandleObject) -> bool {
                set_module_export(scope, env, "NATIVE_VAL", &777.0f64)
            }
        }

        let dir = test_tempdir();
        std::fs::write(
            dir.path().join("file_mod.js"),
            "export const FILE_VAL = 888;\n",
        )
        .unwrap();

        let rt = test_runtime();
        rt.reset_module_loader(dir.path().to_path_buf());
        let scope = rt.default_global();
        unsafe {
            assert!(register_module::<TestNative>(&scope));

            let result = evaluate_module(
                &scope,
                r#"
                    import { NATIVE_VAL } from "test_native";
                    import { FILE_VAL } from "./file_mod.js";
                    globalThis._native = NATIVE_VAL;
                    globalThis._file = FILE_VAL;
                "#,
                "entry.mjs",
            );
            assert!(result.is_ok(), "mixed native+file module evaluation failed");
            assert_eq!(read_global_f64(&scope, "globalThis._native"), 777.0);
            assert_eq!(read_global_f64(&scope, "globalThis._file"), 888.0);
        }
    }
}
