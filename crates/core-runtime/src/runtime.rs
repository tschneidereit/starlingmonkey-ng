// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

use std::{
    cell::{RefCell, UnsafeCell},
    env,
    ffi::c_void,
    path::Path,
    process,
    ptr::NonNull,
    rc::Rc,
    sync::{Mutex, OnceLock},
};

use crate::{config::RuntimeConfig, event_loop, module, report_pending_exception};
use js::{
    engine::{JSEngine, JSEngineHandle, MozJSRuntime, RealmOptions},
    gc::scope::Scope,
    heap::{Heap, Trace},
    native::JS_GetRuntime,
    native::{JSObject, JSRuntime, JSTracer},
    prelude::RootScope,
    Object,
};

// ---------------------------------------------------------------------------
// Global initializer registry
// ---------------------------------------------------------------------------

/// Callback type for installing additional globals on a newly created global object.
///
/// Registered initializers are called during `Runtime::new_global()` after
/// the built-in timer globals have been installed.
///
/// # Safety
///
/// The callback must only perform safe JS operations within the provided scope.
type GlobalInitFn = unsafe fn(&Scope<'_>, Object<'_>);

thread_local! {
    static GLOBAL_INITIALIZERS: RefCell<Vec<GlobalInitFn>> = const { RefCell::new(Vec::new()) };
}

/// Register a function to be called whenever a new global object is created.
///
/// This is used by builtins crates (e.g., `web-globals`) to install their
/// functions and constants on every global without creating a dependency
/// from `core-runtime` to the builtins crate.
///
/// Must be called before `Runtime::init()` to take effect on the default global.
pub fn register_global_initializer(init: GlobalInitFn) {
    GLOBAL_INITIALIZERS.with(|inits| inits.borrow_mut().push(init));
}

/// Clear all registered global initializers (used between tests).
pub fn clear_global_initializers() {
    GLOBAL_INITIALIZERS.with(|inits| inits.borrow_mut().clear());
}

// ---------------------------------------------------------------------------
// Engine singleton
// ---------------------------------------------------------------------------

/// Wrapper to allow `JSEngine` inside a `Mutex` in a `static`.
///
/// `JSEngine` is `!Send + !Sync` (via `PhantomData<*mut ()>`), but its
/// actual state is just an `Arc<AtomicU32>` handle refcount. Thread safety
/// for JS *execution* is enforced at the `Runtime` level, not the engine
/// level. `JS_Init` / `JS_ShutDown` are process-global operations.
struct EngineState(Option<JSEngine>);

// SAFETY: See above — the `!Send` bound on `JSEngine` is conservative.
// We only call `JS_Init` once and `JS_ShutDown` once (at exit), and
// `JSEngineHandle` (the thing handed out to runtimes) is already
// `Send + Sync`.
unsafe impl Send for EngineState {}

/// Process-global engine singleton.
///
/// Uses `OnceLock` for thread-safe one-time initialization and `Mutex`
/// for interior mutability so the `atexit` handler can take and drop the
/// engine to call `JS_ShutDown()` cleanly.
static ENGINE: OnceLock<Mutex<EngineState>> = OnceLock::new();

unsafe extern "C" {
    fn atexit(func: unsafe extern "C" fn()) -> std::os::raw::c_int;
}

/// `atexit` callback: takes the `JSEngine` out of the global and drops it,
/// which calls `JS_ShutDown()` and prevents SpiderMonkey's C++ static
/// destructors from crashing on process exit.
unsafe extern "C" fn shutdown_engine() {
    if let Some(mutex) = ENGINE.get() {
        // If the lock is poisoned we're already in a bad state; just skip.
        if let Ok(mut guard) = mutex.lock() {
            drop(guard.0.take());
        }
    }
}

/// Get a `JSEngineHandle` for creating new `MozJSRuntime`s.
///
/// Initializes the engine on first call (once per process) and registers
/// an `atexit` handler so `JS_ShutDown()` runs at process exit.
/// `JSEngineHandle` is `Send + Sync`, so it can be used from any thread.
fn engine_handle() -> JSEngineHandle {
    let mutex = ENGINE.get_or_init(|| {
        let engine = JSEngine::init().expect("failed to init JS engine");
        // SAFETY: `shutdown_engine` is a valid function pointer.
        unsafe {
            atexit(shutdown_engine);
        }
        Mutex::new(EngineState(Some(engine)))
    });
    mutex
        .lock()
        .unwrap()
        .0
        .as_ref()
        .expect("JS engine has been shut down")
        .handle()
}

/// The StarlingMonkey runtime wrapper around SpiderMonkey.
///
/// Each `Runtime` owns a SpiderMonkey `JSContext` (via `MozJSRuntime`), a
/// default global object, and the associated module loader state. Multiple
/// `Runtime` instances can be created consecutively on the same thread
/// (e.g. in tests); each cleans up its state on drop.
///
/// Uses `UnsafeCell` for interior mutability because SpiderMonkey contexts
/// are inherently single-threaded, and the scope-based API requires `&mut`
/// access to the context even though we store the runtime behind `Rc`.
///
/// A `Runtime` instance always roots itself and traces its members.
#[js::allow_unrooted_interior]
pub struct Runtime {
    /// Default global, declared before `mozjs_rt` so it drops first.
    /// `Heap::drop()` fires a GC write barrier, which requires the
    /// SpiderMonkey context to still be alive.
    default_global: Heap<*mut JSObject>,
    mozjs_rt: UnsafeCell<MozJSRuntime>,
    /// The event loop task registry. Stores all pending async tasks
    /// (timers, promise resolutions, I/O completions, etc.) and is
    /// traced during GC via the runtime's extra-roots-tracer.
    event_loop: RefCell<crate::event_loop::EventLoop>,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime").finish()
    }
}

impl Runtime {
    pub fn init_from_env() -> Rc<Self> {
        let config = RuntimeConfig::from_env().unwrap_or_else(|e| {
            eprintln!("Error loading runtime config: {}", e);
            process::exit(1);
        });
        Self::init(&config)
    }

    pub fn init_from_args() -> Rc<Self> {
        let config = RuntimeConfig::from_args(env::args()).unwrap_or_else(|e| {
            eprintln!("Error loading runtime config: {}", e);
            process::exit(1);
        });
        Self::init(&config)
    }

    /// Get a mutable reference to the inner MozJS runtime.
    ///
    /// # Safety
    ///
    /// SpiderMonkey is single-threaded; this is sound as long as no two
    /// `&mut MozJSRuntime` references exist simultaneously (enforced by
    /// the single-threaded usage pattern).
    #[allow(clippy::mut_from_ref)]
    fn mozjs_rt_mut(&self) -> &mut MozJSRuntime {
        unsafe { &mut *self.mozjs_rt.get() }
    }

    /// Initialize a new runtime and return a reference-counted handle to it.
    ///
    /// The runtime owns the SpiderMonkey context and all global objects created
    /// in that context. The caller is responsible for keeping the `Rc<Runtime>`
    /// alive for as long as the runtime is needed.
    pub fn init(config: &RuntimeConfig) -> Rc<Self> {
        let mut mozjs_rt =
            unsafe { MozJSRuntime::create_with_internal_job_queues(engine_handle(), None) };
        js::gc::init(mozjs_rt.cx());

        let rt = Rc::new(Self {
            mozjs_rt: UnsafeCell::new(mozjs_rt),
            default_global: Heap::default(),
            event_loop: RefCell::new(crate::event_loop::EventLoop::new()),
        });

        // Register runtime GC tracer, passing a raw pointer to the Rc's
        // inner allocation so the callback can trace `default_global`
        // without a thread-local lookup.
        //
        // SAFETY: Rc heap-allocates the `Runtime`, so its address is stable.
        // We remove this tracer in `Drop`, guaranteeing the pointer remains
        // valid for the tracer's entire lifetime.
        let self_ptr = Rc::as_ptr(&rt) as *mut c_void;
        unsafe {
            js::gc::add_extra_gc_roots_tracer(
                rt.mozjs_rt_mut().cx(),
                Some(trace_runtime_cb),
                self_ptr,
            );
        }

        // Register GC tracer for the module registry so cached module
        // objects are properly traced.
        module::init_module_gc_tracer(rt.mozjs_rt_mut().cx());

        // Determine base directory for import resolution.
        let base_path = if config.eval_script.is_some() {
            // For eval scripts, resolve relative to cwd.
            std::env::current_dir().unwrap()
        } else {
            let p = Path::new(&config.script_path);
            let abs = if p.is_absolute() {
                p.to_path_buf()
            } else {
                std::env::current_dir().unwrap_or_default().join(p)
            };
            abs.parent().map(|p| p.to_path_buf()).unwrap()
        };
        unsafe {
            module::init_module_loader(rt.rt(), base_path);
        }

        // Create the default global and register builtins.
        // `new_global` sets `default_global` before registering classes
        // so the GC trace callback can trace the ClassRegistry during
        // class registration (where compacting GC may fire).
        drop(rt.new_global());
        rt.run_initializer_script(config);

        rt
    }

    pub fn new_global(&self) -> RootScope<'_, js::gc::scope::EnteredRealm> {
        let cx = self.mozjs_rt_mut().cx();
        let scope = RootScope::new_global(
            cx,
            &js::class::STARLING_GLOBAL_CLASS,
            RealmOptions::default(),
        );

        // Store the global in `default_global` before any class registration.
        // Class registration allocates JS objects, which can trigger compacting
        // GC (especially under GC zeal mode 14). The GC trace callback needs
        // `default_global` to be set so it can find and trace the ClassRegistry's
        // Heap entries — otherwise compaction moves prototype objects without
        // updating the Heap pointers, leaving them stale.
        self.default_global.set(scope.global().handle().get());

        unsafe {
            event_loop::timer::install_timer_globals(&scope, scope.global());
        }

        // Call any registered global initializers (e.g., web-globals, WPT builtins).
        GLOBAL_INITIALIZERS.with(|inits| {
            for init in inits.borrow().iter() {
                unsafe { init(&scope, scope.global()) };
            }
        });

        scope
    }

    /// Enter the default global realm and return a rooting scope for it.
    pub fn default_global(&self) -> RootScope<'_, js::gc::scope::EnteredRealm> {
        let global = NonNull::new(self.default_global.get()).expect("default global should be set");
        RootScope::new_with_realm(self.mozjs_rt_mut().cx(), global)
    }

    fn run_initializer_script(&self, config: &RuntimeConfig) {
        let scope = self.default_global();
        // Run initializer script if provided (always as legacy script).
        if let Some(ref init_path) = config.initializer_script_path {
            let init_source = std::fs::read_to_string(init_path).unwrap_or_else(|e| {
                eprintln!("Error reading initializer script '{}': {}", init_path, e);
                process::exit(1);
            });
            let filename = init_path.as_str();
            if js::compile::evaluate_with_filename(&scope, &init_source, filename, 1).is_err() {
                eprintln!("Error evaluating initializer script '{init_path}':");
                unsafe { report_pending_exception(&scope) };
                process::exit(1);
            }
        }
    }

    /// Returns the `JSRuntime` object.
    pub fn rt(&self) -> *mut JSRuntime {
        // SAFETY: cx_no_gc only needs shared access.
        let rt = unsafe { &*self.mozjs_rt.get() };
        unsafe { JS_GetRuntime(rt.cx_no_gc()) }
    }

    /// Create a `Scope` for the current realm.
    ///
    /// # Safety
    ///
    /// A realm must already be entered on this runtime's context.
    pub unsafe fn scope(&self) -> RootScope<'_, js::gc::scope::EnteredRealm> {
        RootScope::from_current_realm(self.mozjs_rt_mut().cx().raw_cx())
    }

    /// Returns the underlying mozjs `Runtime`.
    pub fn mozjs_rt(&self) -> &MozJSRuntime {
        // SAFETY: shared access is fine for reading.
        unsafe { &*self.mozjs_rt.get() }
    }

    /// Returns a reference to the event loop task registry.
    ///
    /// The event loop is behind a `RefCell` because the `Runtime` is
    /// stored behind `Rc` while the event loop needs `&mut` access
    /// during task execution.
    pub fn event_loop(&self) -> &RefCell<crate::event_loop::EventLoop> {
        &self.event_loop
    }

    /// Re-initialize the module loader.
    ///
    /// Clears any existing module state (registry, cached modules, resolver)
    /// and sets up a fresh module resolve hook rooted at `base_path`.
    /// Useful in tests to point imports at a temp directory.
    pub fn reset_module_loader(&self, base_path: std::path::PathBuf) {
        module::clear_module_state();
        unsafe {
            module::init_module_loader(self.rt(), base_path);
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // Clear module state while tracers are still registered.
        // Heap::drop fires GC write barriers which can trigger GC under
        // GC zeal — the module tracer must still be registered.
        module::clear_module_state();

        // Clear global initializers so they don't accumulate across
        // multiple Runtime instances (e.g. in test suites).
        clear_global_initializers();

        // Remove GC tracers (module registry is empty, so the tracer
        // is a no-op even if called during barrier processing).
        // The class registry is owned by the global object and cleaned
        // up by its finalize hook — no explicit clearing needed.
        let self_ptr = self as *const Self as *mut c_void;
        unsafe {
            js::gc::remove_extra_gc_roots_tracer(
                self.mozjs_rt().cx_no_gc(),
                Some(trace_runtime_cb),
                self_ptr,
            );
        }
        module::remove_module_gc_tracer(self.mozjs_rt().cx_no_gc());
        js::gc::shutdown();
    }
}

/// GC trace callback for this `Runtime`'s `default_global` Heap and
/// the per-global class registry.
///
/// `data` is a raw pointer to the `Runtime` (passed via
/// `add_extra_gc_roots_tracer`). SpiderMonkey calls this during GC so
/// that the stored `Heap<*mut JSObject>` is properly traced and updated,
/// and the class registry's prototype Heaps are kept in sync with
/// compacting GC.
///
/// # Safety
///
/// - `trc` must be a valid `JSTracer` pointer provided by SpiderMonkey's GC.
/// - `data` must point to a live `Runtime` instance.
#[js::allow_unrooted]
unsafe extern "C" fn trace_runtime_cb(trc: *mut JSTracer, data: *mut c_void) {
    let rt = &*(data as *const Runtime);
    rt.default_global.trace(trc);
    // Trace the per-global class registry stored in the global's reserved slot.
    let global = rt.default_global.get();
    if !global.is_null() {
        js::class::trace_class_registry_for_global(trc, global);
    }
    // Trace all tasks in the event loop (they may hold JS object references).
    //
    // Use `as_ptr()` to bypass `RefCell` borrow tracking. GC tracing runs
    // with JS execution paused (stop-the-world), so the `&mut EventLoop`
    // on the caller's stack isn't being actively used — no aliasing hazard.
    // A normal `borrow()` would panic when the event loop driver already
    // holds a `borrow_mut()`.
    let el = unsafe { &*rt.event_loop.as_ptr() };
    el.trace(trc);
}
