// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

use std::{
    cell::{RefCell, UnsafeCell},
    env,
    ffi::c_void,
    process,
    ptr::NonNull,
    rc::Rc,
    sync::{Mutex, OnceLock},
};

use crate::{
    config::RuntimeConfig, event_loop, invocation::InvocationRegistry, module,
    report_pending_exception,
};
use js::{
    engine::{JSEngine, JSEngineHandle, MozJSRuntime, RealmOptions},
    gc::{handle::Heap, scope::Scope},
    heap::Trace,
    native::JS_GetRuntime,
    native::{JSRuntime, JSTracer},
    prelude::RootScope,
    Object,
};

// ---------------------------------------------------------------------------
// Global initializer registry
// ---------------------------------------------------------------------------

/// Callback type for installing additional globals on a newly created global object.
///
/// Registered initializers are called during `Runtime::new_global()`.
type GlobalInitFn = for<'a> fn(&'a Scope<'a>, Object<'a>);

thread_local! {
    static GLOBAL_INITIALIZERS: RefCell<Vec<GlobalInitFn>> = const { RefCell::new(Vec::new()) };
}

/// Register a function to be called whenever a new global object is created.
///
/// This is used by builtins crates (e.g., `web-globals`) to install their
/// functions and constants on every global without creating a dependency
/// from `core-runtime` to the builtins crate.
///
/// Must be called before `Runtime::init()` to take effect on the default
/// global. Registration is idempotent and applies to all future runtimes and global objects.
pub fn register_global_initializer(init: GlobalInitFn) {
    GLOBAL_INITIALIZERS.with(|inits| {
        let mut inits = inits.borrow_mut();
        if !inits.contains(&init) {
            inits.push(init);
        }
    });
}

/// Clear all registered global initializers (used between tests that need
/// disjoint initializer sets).
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
            // A `Runtime` can still be alive here, e.g because `process::exit` was called from an
            // error path. Its `MozJSRuntime` holds an engine handle, and `JSEngine::drop` asserts
            // the handle count is zero, so dropping anyway would trap.
            // To avoid that, we leave the
            // engine in the static instead and skip `JS_ShutDown`:
            // We're about to exit anyway, so process cleanup will handle it.
            if guard.0.as_ref().is_some_and(|engine| engine.can_shutdown()) {
                drop(guard.0.take());
            }
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
    default_global: Heap<js::object::Object>,
    mozjs_rt: UnsafeCell<MozJSRuntime>,
    /// Registry of live [`InvocationState`](crate::invocation::InvocationState)
    /// instances. The GC trace callback iterates this to trace all event
    /// loops across concurrent invocations.
    invocations: RefCell<InvocationRegistry>,
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
            invocations: RefCell::new(InvocationRegistry::new()),
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

        unsafe {
            module::init_module_loader(rt.rt(), config.base_path());
        }

        // Create the default global and register builtins.
        drop(rt.new_global());
        rt.run_initializer_script(config);

        rt
    }

    /// Create a new global object (and realm), install all registered global initializers on it,
    /// and return a scope entered into it.
    ///
    /// The first global created becomes the runtime's default global (see
    /// [`default_global`](Self::default_global)), later ones are additional realms and leave the
    /// default untouched.
    pub fn new_global(&self) -> RootScope<'_, js::gc::scope::EnteredRealm> {
        let cx = self.mozjs_rt_mut().cx();
        let scope = RootScope::new_global(
            cx,
            &js::class::STARLING_GLOBAL_CLASS,
            RealmOptions::default(),
        );

        if !self.default_global.is_initialized() {
            self.default_global.set(scope.global());
        }

        unsafe {
            event_loop::timer::install_timer_globals(&scope, scope.global());
        }

        // Call any registered global initializers (e.g., web-globals, WPT builtins).
        // Snapshot the list first: an initializer (or JS it runs) may itself
        // register further initializers, and calling out under the borrow
        // would panic. Initializers registered mid-call apply to subsequent
        // globals only.
        let inits = GLOBAL_INITIALIZERS.with(|inits| inits.borrow().clone());
        for init in inits {
            init(&scope, scope.global());
        }

        scope
    }

    /// Enter the default global realm and return a rooting scope for it.
    pub fn default_global(&self) -> RootScope<'_, js::gc::scope::EnteredRealm> {
        // SAFETY: Raw JSObject pointer is used to create a rooting scope.
        let global = unsafe {
            NonNull::new(self.default_global.as_ptr()).expect("default global should be set")
        };
        RootScope::new_with_realm(self.mozjs_rt_mut().cx(), global)
    }

    fn run_initializer_script(&self, config: &RuntimeConfig) {
        // Run initializer script if provided (always as legacy script).
        let Some(ref init_path) = config.initializer_script_path else {
            return;
        };
        let scope = self.default_global();
        let init_source = std::fs::read_to_string(init_path).unwrap_or_else(|e| {
            eprintln!("Error reading initializer script '{}': {}", init_path, e);
            process::exit(1);
        });
        let filename = init_path.as_str();

        // The initializer runs with its own event loop active, so scheduling APIs such as
        // `setTimeout` work during initialization. Its microtasks drain before the content script
        // runs.
        let invocation = crate::invocation::InvocationState::new();
        // SAFETY: `invocation` stays at this address until the guard drops at
        // the end of this function.
        let _guard = unsafe { crate::invocation::InvocationGuard::new(self, &invocation) };
        let failed = event_loop::with_event_loop(invocation.event_loop(), |_| {
            let failed =
                js::compile::evaluate_with_filename(&scope, &init_source, filename, 1).is_err();
            if !failed {
                event_loop::run_microtasks(&scope);
            }
            failed
        });
        if failed {
            eprintln!("Error evaluating initializer script '{init_path}':");
            unsafe { report_pending_exception(&scope) };
            process::exit(1);
        }

        // Nothing drives this loop after initialization: leftover async work
        // (a live timer, an in-flight promise-backed operation) would be
        // silently dropped, so make it a hard error instead.
        if invocation.event_loop().is_alive() {
            eprintln!(
                "Error: initializer script '{init_path}' left asynchronous work \
                 (timers or pending operations) behind. Initializer scripts must \
                 complete synchronously"
            );
            process::exit(1);
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

    /// Returns a reference to the invocation registry.
    ///
    /// The registry is behind a `RefCell` because the `Runtime` is
    /// stored behind `Rc` while the registry needs `&mut` access for
    /// registration/unregistration.
    pub fn invocations(&self) -> &RefCell<InvocationRegistry> {
        &self.invocations
    }

    /// Register an invocation for GC tracing.
    ///
    /// # Safety
    ///
    /// The caller must ensure `state` remains valid and at a stable address
    /// until [`unregister_invocation`](Self::unregister_invocation) is called.
    pub unsafe fn register_invocation(&self, state: &crate::invocation::InvocationState) {
        self.invocations.borrow_mut().register(state as *const _);
    }

    /// Unregister a previously registered invocation.
    pub fn unregister_invocation(&self, state: &crate::invocation::InvocationState) {
        self.invocations.borrow_mut().unregister(state as *const _);
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
        js::gc::shutdown(self.mozjs_rt().cx_no_gc());
    }
}

/// GC trace callback for this `Runtime`'s `default_global` Heap and the
/// event loops of all registered invocations.
///
/// `data` is a raw pointer to the `Runtime` (passed via
/// `add_extra_gc_roots_tracer`).
///
/// # Safety
///
/// - `trc` must be a valid `JSTracer` pointer provided by SpiderMonkey's GC.
/// - `data` must point to a live `Runtime` instance.
#[js::allow_unrooted]
unsafe extern "C" fn trace_runtime_cb(trc: *mut JSTracer, data: *mut c_void) {
    let rt = &*(data as *const Runtime);
    rt.default_global.trace(trc);
    // Trace all event loops across registered invocations.
    //
    // Use `as_ptr()` to bypass `RefCell` borrow tracking. GC tracing runs
    // with JS execution paused (stop-the-world), so the `&mut` references
    // on the caller's stack aren't being actively used — no aliasing hazard.
    // A normal `borrow()` would panic when the invocation registry is
    // already borrowed mutably.
    let invocations = unsafe { &*rt.invocations.as_ptr() };
    invocations.trace(trc);
}
