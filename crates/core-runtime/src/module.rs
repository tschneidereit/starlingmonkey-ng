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
//!     // JS can now: import { PI, add } from "myMath";
//! }
//! ```
//!
//! The specifier is the `mod` name camelCased, unless overridden with
//! `#[jsmodule(name = "...")]`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;

use js::conversion::ToJSVal;
use js::error::ExnThrown;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::heap::Trace;
use js::module_raw::{transform_str_to_source_text, CompileOptionsWrapper, SetModulePrivate};
use js::native::{HandleObject, JSNative, JSObject, JSString, JSTracer, Value};
use js::prelude::{HandleValue, RootScope};
use js::Object;
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
    /// e.g. `"myMath"` for `import { add } from "myMath";`
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
    unsafe fn evaluate(scope: &Scope<'_>, env: HandleObject) -> bool;
}

// ============================================================================
// Module registry (thread-local)
// ============================================================================

/// A cached compiled module object, stored in a `Heap` so SpiderMonkey's
/// moving GC can update the pointer during compaction.
/// Traced by `trace_module_registry`, so allowed to contain unrooted interior.
#[js::allow_unrooted_interior]
struct ModuleEntry {
    module_obj: Heap<js::object::Object>,
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
/// 1. Check the module registry for an exact match: native modules by name,
///    already-compiled file modules by canonical path.
/// 2. Resolve via `oxc_resolver` against the referencing module's directory
///    (file-backed modules carry their canonical path in their module
///    private), or against the loader's base path for pathless referrers
///    (eval scripts, native modules).
#[js::allow_unrooted]
unsafe extern "C" fn module_resolve_hook(
    cx: *mut js::native::RawJSContext,
    referencing_private: js::native::RawHandle<Value>,
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

    // 1. Check the module registry for an exact match.
    let cached = MODULE_REGISTRY.with(|reg| {
        reg.borrow()
            .get(&specifier)
            .map(|entry| entry.module_obj.as_ptr())
    });
    if let Some(obj) = cached {
        return obj;
    }

    // 2. Resolve via filesystem using oxc_resolver, relative to the referrer.
    let base_dir = unsafe { referrer_base_dir(cx, referencing_private) };
    match resolve_file_module(cx, &specifier, base_dir) {
        Ok(obj) => obj,
        Err(msg) => {
            let c_msg = CString::new(msg).unwrap_or_else(|_| c"Module resolution failed".into());
            // SAFETY: cx is a valid RawJSContext from the resolve hook.
            let scope = RootScope::from_current_realm(cx);
            js::error::report_error_ascii(&scope, &c_msg);
            ptr::null_mut()
        }
    }
}

/// The directory to resolve a relative specifier against: the parent of the
/// referencing module's path if the referrer carries one in its module
/// private, the loader's base path otherwise (eval-script entries and native
/// modules have no path).
unsafe fn referrer_base_dir(
    cx: *mut js::native::RawJSContext,
    referencing_private: js::native::RawHandle<Value>,
) -> Option<PathBuf> {
    if referencing_private.is_string() {
        if let Some(path) = unsafe { jsstring_to_string(cx, referencing_private.to_string()) } {
            if let Some(parent) = Path::new(&path).parent() {
                if !parent.as_os_str().is_empty() {
                    return Some(parent.to_path_buf());
                }
            }
        }
    }
    BASE_PATH.with(|bp| bp.borrow().clone())
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
    base_dir: Option<PathBuf>,
) -> Result<*mut JSObject, String> {
    let base_dir = base_dir
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

    // Key the cache by the canonicalized path, so the same file reached via
    // different specifiers, lexically different paths, or symlinks is one
    // module instance (and its side effects run once). `canonicalize` can fail on
    // wasm32-wasip2; fall back to a lexical normalization so `.`/`..`-equivalent
    // paths still dedup there (only symlink aliasing, which needs the filesystem,
    // remains best-effort when canonicalize is unavailable).
    let canonical_path = std::fs::canonicalize(&resolved_path)
        .unwrap_or_else(|_| lexically_normalize(resolved_path));
    let canonical_key = canonical_path.to_string_lossy().to_string();

    // Check if already compiled under the canonical path
    let cached = MODULE_REGISTRY.with(|reg| {
        reg.borrow()
            .get(&canonical_key)
            .map(|entry| entry.module_obj.as_ptr())
    });
    if let Some(obj) = cached {
        return Ok(obj);
    }

    // Read source from disk
    let source = std::fs::read_to_string(&canonical_path)
        .map_err(|e| format!("Failed to read '{}': {}", canonical_path.display(), e))?;

    // Compile (but do NOT link or evaluate — SpiderMonkey handles that)
    let c_filename =
        CString::new(canonical_key.as_bytes()).map_err(|_| "Invalid filename".to_string())?;
    let options = CompileOptionsWrapper::new_raw(cx as _, c_filename, 1);
    let mut src = transform_str_to_source_text(&source);
    let module_obj = unsafe { js::module_raw::CompileModule1(cx as _, options.ptr, &mut src) };
    if module_obj.is_null() {
        return Err(format!(
            "Failed to compile module '{}'",
            canonical_path.display()
        ));
    }

    // Root the module for the rest of the setup: allocating the path string
    // below can GC, which would move an unrooted module object.
    let scope = RootScope::from_current_realm(cx);
    let module = unsafe { Object::from_raw(&scope, module_obj) }
        .ok_or_else(|| "Compiled module is null".to_string())?;

    // Store the canonical path in the module private: the resolve hook reads
    // it to resolve this module's own relative imports against its directory.
    let path_str = js::JSString::from_str(&scope, &canonical_key)
        .map_err(|_| "Failed to allocate module path string".to_string())?;
    unsafe { SetModulePrivate(module.as_raw(), &path_str.as_value()) };

    MODULE_REGISTRY.with(|reg| {
        reg.borrow_mut().insert(
            canonical_key,
            ModuleEntry {
                // SAFETY: the module is rooted by `scope` and non-null.
                module_obj: unsafe { Heap::from_raw(module.as_raw()) },
            },
        );
    });

    Ok(module.as_raw())
}

/// Resolve `.` and `..` components lexically, without touching the filesystem, so
/// two lexically-equivalent paths produce the same module-cache key. Used as the
/// fallback key when [`std::fs::canonicalize`] is unavailable (it can fail under
/// wasm32-wasip2).
fn lexically_normalize(path: PathBuf) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            // Pop a preceding normal segment; keep a leading `..` (nothing to pop)
            // and never pop past a root/prefix.
            Component::ParentDir => {
                if !matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.push("..");
                } else {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Convert a JSString to a Rust String.
unsafe fn jsstring_to_string(
    cx: *mut js::native::RawJSContext,
    s: *mut JSString,
) -> Option<String> {
    use js::conversion::jsstr_to_string;
    use std::ptr::NonNull;
    let scope = RootScope::from_current_realm(cx);
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
pub unsafe fn register_module<T: NativeModule>(scope: &Scope<'_>) -> bool {
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

    // Native modules carry no path in their module private: the generated
    // `export var` source has no imports to resolve.

    // 3. Store in registry before linking (resolve hook must find it)
    MODULE_REGISTRY.with(|reg| {
        reg.borrow_mut().insert(
            T::NAME.to_string(),
            ModuleEntry {
                // SAFETY: module was just compiled and is non-null.
                module_obj: unsafe { Heap::from_raw(module.as_raw()) },
            },
        );
    });

    // 4.-6. Link, evaluate, and populate the environment. On any failure,
    // remove the registry entry again so a later import of the module fails
    // to resolve instead of finding a half-initialized module whose value
    // exports are undefined.
    let populated = unsafe { link_evaluate_and_populate::<T>(scope, module, &declarations) };
    if !populated {
        MODULE_REGISTRY.with(|reg| {
            reg.borrow_mut().remove(T::NAME);
        });
    }
    populated
}

/// Steps 4.-6. of [`register_module`]: link and evaluate the compiled module,
/// then populate its environment with the native exports.
///
/// # Safety
///
/// Same contract as [`register_module`].
unsafe fn link_evaluate_and_populate<T: NativeModule>(
    scope: &Scope<'_>,
    module: Object<'_>,
    declarations: &[ModuleExport],
) -> bool {
    let env = match unsafe { link_evaluate_and_get_env(scope, module) } {
        Some(env) => env,
        None => return false,
    };

    // Set up function exports by creating functions and setting them as properties
    for decl in declarations {
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
    unsafe { T::evaluate(scope, env.handle()) }
}

/// Steps 4.-5. shared by every synthetic-module path: link the compiled module,
/// evaluate it (running the `export var ...` initializations), and return its
/// environment object, ready for the caller to populate.
///
/// # Safety
///
/// `module` must be a freshly compiled, registered module object on `scope`'s
/// realm.
unsafe fn link_evaluate_and_get_env<'s>(
    scope: &'s Scope<'_>,
    module: Object<'_>,
) -> Option<Object<'s>> {
    // 4. Link
    if js::module::link(scope, module).is_err() {
        return None;
    }

    // 5. Evaluate (runs the `export var ...` initializations)
    if js::module::evaluate(scope, module).is_err() {
        return None;
    }

    // 6. Get the module environment
    unsafe {
        Object::from_raw(
            scope,
            js::module_raw::GetModuleEnvironment(scope.cx_mut(), module.handle()),
        )
    }
}

/// Register a module under an arbitrary specifier, exporting the given named
/// values, and make it available for `import` from JS.
///
/// This generalizes [`register_module`] for callers that synthesize a module's
/// exports at runtime rather than from a static [`NativeModule`] type: the
/// component-model interpreter builds one such module per imported WIT interface,
/// keyed by the interface's WIT specifier (e.g. `"test:iface/x@0.2.0"`), with
/// the import functions and resource classes as exports.
///
/// Each `(name, value)` pair becomes an `export var <name>` binding initialized
/// to `value`. The module is registered under the owned `name` string, so the
/// resolve hook resolves an exact-match `import ... from "<name>"`.
///
/// On any failure the half-registered entry is removed again, matching
/// [`register_module`]: a later import then fails to resolve rather than finding
/// a module whose value exports are undefined.
///
/// # Safety
///
/// - [`init_module_loader`] must have been called first.
/// - `exports`' value handles must be live and rooted for the duration of the
///   call.
pub unsafe fn register_synthetic_module(
    scope: &Scope<'_>,
    name: &str,
    exports: &[(&str, HandleValue)],
) -> Result<(), ExnThrown> {
    // 1. Generate JS module source: `export var name1; export var name2; ...`.
    // The bindings start out `undefined`; step 6 below assigns each its value.
    let mut source = String::new();
    for (export_name, _) in exports {
        source.push_str("export var ");
        source.push_str(export_name);
        source.push_str(";\n");
    }

    // 2. Compile the module.
    let filename = CString::new(name).map_err(|_| ExnThrown)?;
    let options = CompileOptionsWrapper::new(scope.cx_mut(), filename, 1);
    let mut src = transform_str_to_source_text(&source);
    // Synthetic modules carry no path in their module private: the generated
    // `export var` source has no imports of its own to resolve.
    let module = unsafe { js::module::compile_module(scope, options.ptr, &mut src) }?;

    // 3. Store in the registry under the owned specifier before linking, so the
    //    resolve hook finds it.
    MODULE_REGISTRY.with(|reg| {
        reg.borrow_mut().insert(
            name.to_string(),
            ModuleEntry {
                // SAFETY: module was just compiled and is non-null.
                module_obj: unsafe { Heap::from_raw(module.as_raw()) },
            },
        );
    });

    // 4.-6. Link, evaluate, and populate the environment with the export values.
    // On any failure, remove the registry entry again.
    let result = unsafe { link_evaluate_and_populate_values(scope, module, exports) };
    if result.is_err() {
        MODULE_REGISTRY.with(|reg| {
            reg.borrow_mut().remove(name);
        });
    }
    result
}

/// Steps 4.-6. of [`register_synthetic_module`]: link and evaluate the compiled
/// module, then assign each export binding its provided value.
///
/// # Safety
///
/// Same contract as [`register_synthetic_module`].
unsafe fn link_evaluate_and_populate_values(
    scope: &Scope<'_>,
    module: Object<'_>,
    exports: &[(&str, HandleValue)],
) -> Result<(), ExnThrown> {
    let env = unsafe { link_evaluate_and_get_env(scope, module) }.ok_or(ExnThrown)?;

    for (export_name, value) in exports {
        let c_name = CString::new(*export_name).map_err(|_| ExnThrown)?;
        env.set_property(scope, &c_name, *value)?;
    }
    Ok(())
}

/// Register a real ES-module source under an arbitrary specifier, making it
/// importable by name.
///
/// Where [`register_synthetic_module`] populates `export var` bindings with
/// runtime values, this compiles genuine module `source` — with its own
/// `export`s and possibly its own `import`s — and registers it under `name`. The
/// component-model bootstrap uses it for the application's named modules (the
/// componentizer's `modules` list): these may `import` from the synthesized
/// import modules, so the bootstrap registers those first.
///
/// The module is linked and evaluated immediately so its side effects run and
/// its exports are bound. On failure the half-registered entry is removed again,
/// matching [`register_synthetic_module`].
///
/// # Safety
///
/// - [`init_module_loader`] must have been called first.
pub unsafe fn register_source_module(
    scope: &Scope<'_>,
    name: &str,
    source: &str,
) -> Result<(), ExnThrown> {
    let filename = CString::new(name).map_err(|_| ExnThrown)?;
    let options = CompileOptionsWrapper::new(scope.cx_mut(), filename, 1);
    let mut src = transform_str_to_source_text(source);
    let module = unsafe { js::module::compile_module(scope, options.ptr, &mut src) }?;

    // Register before linking so the resolve hook (and the module's own imports)
    // can find it by name.
    MODULE_REGISTRY.with(|reg| {
        reg.borrow_mut().insert(
            name.to_string(),
            ModuleEntry {
                // SAFETY: module was just compiled and is non-null.
                module_obj: unsafe { Heap::from_raw(module.as_raw()) },
            },
        );
    });

    let linked = js::module::link(scope, module)
        .and_then(|()| js::module::evaluate(scope, module).map(drop));
    if linked.is_err() {
        MODULE_REGISTRY.with(|reg| {
            reg.borrow_mut().remove(name);
        });
        return Err(ExnThrown);
    }
    Ok(())
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
pub unsafe fn evaluate_module<'s>(
    scope: &'s Scope<'_>,
    source: &str,
    filename: &str,
) -> Result<HandleValue<'s>, ExnThrown> {
    let c_filename = CString::new(filename).unwrap();
    let options = CompileOptionsWrapper::new(scope.cx_mut(), c_filename, 1);

    let mut src = transform_str_to_source_text(source);
    let module = unsafe { js::module::compile_module(scope, options.ptr, &mut src) }
        .map_err(|_| ExnThrown)?;

    // If the filename is a real path, store its absolute form in the module
    // private: the resolve hook reads it to resolve this entry's relative
    // imports against the entry's own directory. Pathless entries (eval
    // scripts, synthetic filenames) leave the private unset and fall back to
    // the loader's base path. The empty-path guard prevents WASI from
    // treating `Path::new("").exists()` as a valid root directory.
    let path = Path::new(filename);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && parent.exists() {
            let abs = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir().unwrap_or_default().join(path)
            };
            let path_str =
                js::JSString::from_str(scope, &abs.to_string_lossy()).map_err(|_| ExnThrown)?;
            unsafe { SetModulePrivate(module.as_raw(), &path_str.as_value()) };
        }
    }

    js::module::link(scope, module).map_err(|_| ExnThrown)?;
    js::module::evaluate(scope, module).map_err(|_| ExnThrown)
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
    scope: &'s Scope<'_>,
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

    #[test]
    fn lexically_normalize_dedups_dot_segments() {
        // The fallback key when canonicalize is unavailable must collapse
        // `.`/`..` so lexically-different specifiers for the same file dedup.
        assert_eq!(
            lexically_normalize(PathBuf::from("/base/./counter.js")),
            PathBuf::from("/base/counter.js")
        );
        assert_eq!(
            lexically_normalize(PathBuf::from("/base/sub/../counter.js")),
            PathBuf::from("/base/counter.js")
        );
        assert_eq!(
            lexically_normalize(PathBuf::from("/base/a/b/../../counter.js")),
            PathBuf::from("/base/counter.js")
        );
        // A leading `..` on a relative path is preserved (nothing to pop).
        assert_eq!(
            lexically_normalize(PathBuf::from("../x/counter.js")),
            PathBuf::from("../x/counter.js")
        );
    }

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
    fn nested_imports_resolve_against_the_importing_module() {
        let dir = test_tempdir();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();

        // A decoy at the entry's base dir: resolution anchored to the entry
        // (instead of the importing module) would pick this one up.
        std::fs::write(dir.path().join("c.js"), "export const WHO = 'decoy';\n").unwrap();
        std::fs::write(sub.join("c.js"), "export const WHO = 'sub';\n").unwrap();
        std::fs::write(
            sub.join("b.js"),
            r#"import { WHO } from "./c.js"; export const FROM_B = WHO;"#,
        )
        .unwrap();

        let rt = test_runtime();
        rt.reset_module_loader(dir.path().to_path_buf());
        let scope = rt.default_global();
        unsafe {
            let result = evaluate_module(
                &scope,
                r#"
                    import { FROM_B } from "./sub/b.js";
                    globalThis._who = FROM_B;
                "#,
                "entry.mjs",
            );
            assert!(result.is_ok(), "nested module evaluation failed");
            assert_eq!(
                read_global_string(&scope, "globalThis._who"),
                "sub",
                "sub/b.js's './c.js' must resolve next to b.js, not next to the entry"
            );
        }
    }

    #[test]
    fn same_specifier_resolves_per_directory() {
        let dir = test_tempdir();
        let dir1 = dir.path().join("one");
        let dir2 = dir.path().join("two");
        std::fs::create_dir(&dir1).unwrap();
        std::fs::create_dir(&dir2).unwrap();
        std::fs::write(dir1.join("helper.js"), "export const N = 1;\n").unwrap();
        std::fs::write(dir2.join("helper.js"), "export const N = 2;\n").unwrap();

        let rt = test_runtime();
        rt.reset_module_loader(dir.path().to_path_buf());
        let scope = rt.default_global();
        unsafe {
            let r1 = evaluate_module(
                &scope,
                r#"import { N } from "./helper.js"; globalThis._n1 = N;"#,
                &dir1.join("entry1.mjs").to_string_lossy(),
            );
            assert!(r1.is_ok());
            let r2 = evaluate_module(
                &scope,
                r#"import { N } from "./helper.js"; globalThis._n2 = N;"#,
                &dir2.join("entry2.mjs").to_string_lossy(),
            );
            assert!(r2.is_ok());
            assert_eq!(
                (
                    read_global_f64(&scope, "globalThis._n1"),
                    read_global_f64(&scope, "globalThis._n2"),
                ),
                (1.0, 2.0),
                "the same relative specifier must resolve per importing directory"
            );
        }
    }

    #[test]
    fn lexically_different_paths_share_one_instance() {
        let dir = test_tempdir();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(
            dir.path().join("counter.js"),
            r#"
                if (!globalThis._lexCounter) globalThis._lexCounter = 0;
                globalThis._lexCounter++;
                export const dummy = 1;
            "#,
        )
        .unwrap();

        let rt = test_runtime();
        rt.reset_module_loader(dir.path().to_path_buf());
        let scope = rt.default_global();
        unsafe {
            let r1 = evaluate_module(
                &scope,
                r#"import { dummy } from "./counter.js";"#,
                &dir.path().join("entry1.mjs").to_string_lossy(),
            );
            assert!(r1.is_ok());
            // The same file via a lexically different path must hit the cache.
            let r2 = evaluate_module(
                &scope,
                r#"import { dummy } from "./sub/../counter.js";"#,
                &dir.path().join("entry2.mjs").to_string_lossy(),
            );
            assert!(r2.is_ok());
            assert_eq!(
                read_global_f64(&scope, "globalThis._lexCounter"),
                1.0,
                "one file must be one module instance regardless of the path spelling"
            );
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
    fn failed_native_module_is_not_importable() {
        // A native module whose `evaluate` fails must not stay registered:
        // importing it afterwards has to fail instead of yielding a module
        // with undefined value exports.
        struct BrokenNative;
        impl NativeModule for BrokenNative {
            const NAME: &'static str = "broken_native";
            fn declarations() -> Vec<ModuleExport> {
                vec![ModuleExport::Value { js_name: "VAL" }]
            }
            unsafe fn evaluate(_scope: &js::gc::scope::Scope<'_>, _env: HandleObject) -> bool {
                false
            }
        }

        let rt = test_runtime();
        let scope = rt.default_global();
        unsafe {
            assert!(!register_module::<BrokenNative>(&scope));
            let result = evaluate_module(
                &scope,
                r#"import { VAL } from "broken_native";"#,
                "entry.mjs",
            );
            assert!(
                result.is_err(),
                "import of a failed module must not resolve"
            );
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
