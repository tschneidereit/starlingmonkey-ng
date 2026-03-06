// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Scope-based rooting for SpiderMonkey GC values.
//!
//! This module provides [`RootScope`] and [`Scope`], a scope-based approach
//! to rooting GC values inspired by V8's `HandleScope`. A scope manages
//! roots in a bump-allocated page that is traced during GC and freed when
//! the scope drops.
//!
//! # Design
//!
//! Each scope owns its own [`ScopeAlloc`](mozjs::gc::pool::ScopeAlloc) — a
//! per-scope bump allocator that draws pages from the [`HandlePool`]
//! freelist. This ensures that different scopes have disjoint storage:
//! dropping one scope cannot invalidate another scope's handles.
//!
//! The pool is wrapped in a `PoolRooter` that sits on the `autoGCRooters`
//! stack for the entire lifetime of the [`Runtime`]. During GC, the
//! rooter walks all active scope allocators and traces their live slots.
//!
//! # Type hierarchy
//!
//! - [`Scope<'cx>`] — the core scope type. Holds a `JSContext` pointer
//!   and a per-scope bump allocator. All rooting methods and `cx()`/`cx_mut()`
//!   access are defined here. This is the parameter type for most
//!   [`mozjs::js`](mozjs::js) API functions.
//!
//! - [`RootScope<'cx, S>`] — an owning scope that manages realm entry.
//!   Parameterised by a typestate marker:
//!   - [`NoRealm`]: no realm entered yet. Only `cx()`/`cx_mut()` and
//!     `enter_realm()` are available.
//!   - [`EnteredRealm`]: a realm is active. `Deref`s to `&Scope<'cx>`,
//!     providing all rooting methods.
//!
//! - [`InnerScope<'parent>`] — a nested scope with its own allocator.
//!   `Deref`s to `&Scope<'parent>`, so it can be used wherever `&Scope`
//!   is expected. Values rooted via the inner scope are freed when it
//!   drops, independently of the parent scope's roots.
//!
//! # Nested scopes
//!
//! Call [`Scope::inner_scope`] to create a child scope. Because each
//! scope owns separate storage, rooting on the parent while an inner
//! scope is alive is safe — the parent's handles are not affected when
//! the inner scope drops.
//!
//! # Example
//!
//! ```ignore
//! use crate::gc::scope::RootScope;
//!
//! let scope = RootScope::new_global(rt.cx(), &SIMPLE_GLOBAL_CLASS, options);
//! let s = js::string::from_str(&scope, "hello")?;
//! // s is a Handle<'_, *mut JSString> — rooted in the scope
//! ```
//!
//! [`HandlePool`]: mozjs::gc::pool::HandlePool
//! [`Runtime`]: mozjs::rust::Runtime

use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::ptr::NonNull;

use super::pool::{ScopeAlloc, SlotTag};
use crate::object::Object;
use mozjs::context::JSContext;
use mozjs::gc::Handle;
use mozjs::jsapi::JSContext as RawJSContext;
use mozjs::jsapi::JS::{BigInt, Symbol};
use mozjs::jsapi::{
    jsid, JSAutoRealm, JSClass, JSFunction, JSObject, JSScript, JSString, OnNewGlobalHookOption,
    Value,
};

// ---------------------------------------------------------------------------
// Typestate markers
// ---------------------------------------------------------------------------

/// Typestate marker: no realm has been entered.
pub struct NoRealm(());

/// Typestate marker: a realm is active.
pub struct EnteredRealm(());

// ---------------------------------------------------------------------------
// Scope — the core rooting type
// ---------------------------------------------------------------------------

/// The core scope type for rooting GC values.
///
/// Each `Scope` owns its own [`ScopeAlloc`] — a per-scope bump allocator
/// backed by pages from the [`HandlePool`]. This ensures that different
/// scopes have disjoint storage and cannot corrupt each other's roots.
///
/// Both [`RootScope<'cx, EnteredRealm>`] and [`InnerScope`] deref to
/// `&Scope`, so all API functions that take `scope: &Scope<'_>` work
/// transparently with either.
///
/// `'cx` is the lifetime of the parent context or scope.
///
/// [`ScopeAlloc`]: mozjs::gc::pool::ScopeAlloc
/// [`HandlePool`]: mozjs::gc::pool::HandlePool
pub struct Scope<'cx> {
    /// Raw pointer to the JSContext, wrapped in `UnsafeCell` for interior
    /// mutability (same rationale as `RootScope`).
    raw_cx: UnsafeCell<*mut RawJSContext>,

    /// Per-scope bump allocator, boxed for stable heap address.
    ///
    /// The `Box` guarantees that the `ScopeAlloc`'s address doesn't change
    /// when the `Scope` is moved (e.g. during `RootScope::enter_realm`).
    /// This is essential because the pool's intrusive linked list stores
    /// raw pointers to active allocators.
    ///
    /// Wrapped in `UnsafeCell` because rooting methods take `&self`
    /// (to allow multiple live handles) but the allocator needs mutation.
    alloc: UnsafeCell<Box<ScopeAlloc>>,

    _phantom: PhantomData<&'cx mut ()>,
}

impl<'cx> Scope<'cx> {
    /// Create a new Scope with its own boxed allocator.
    fn new(raw_cx: *mut RawJSContext) -> Self {
        let pool = super::pool::current_pool();
        // SAFETY: The pool pointer is valid for the lifetime of the Runtime
        // (until shutdown() is called), which outlives all scopes.
        let alloc = unsafe { ScopeAlloc::new(pool) };
        Scope {
            raw_cx: UnsafeCell::new(raw_cx),
            alloc: UnsafeCell::new(alloc),
            _phantom: PhantomData,
        }
    }

    /// Get a mutable reference to the scope's allocator.
    ///
    /// This uses `UnsafeCell` interior mutability. Safe because:
    /// 1. SpiderMonkey is single-threaded — no concurrent access.
    /// 2. The `&mut ScopeAlloc` borrow is never stored — it's used
    ///    immediately for a single `alloc()` call and released.
    #[allow(clippy::mut_from_ref)]
    fn alloc_mut(&self) -> &mut ScopeAlloc {
        // SAFETY: The Box provides a stable heap address. We deref through
        // the UnsafeCell to get the Box, then deref the Box to get the alloc.
        unsafe { &mut *self.alloc.get() }
    }

    /// Get a reference to the underlying [`JSContext`].
    pub fn cx(&self) -> &JSContext {
        // SAFETY: JSContext is #[repr(transparent)] over NonNull<RawJSContext>.
        unsafe { &*(self.raw_cx.get() as *const JSContext) }
    }

    /// Get a mutable reference to the underlying [`JSContext`].
    ///
    /// Uses interior mutability: rooting methods take `&self` so that multiple
    /// handles can coexist, but JSAPI calls need `&mut JSContext`. This is safe
    /// because:
    /// 1. SpiderMonkey is single-threaded — no concurrent access.
    /// 2. While GC runs concurrently, as long as Rust code ensures all
    ///    GC references are properly rooted, all safety requirements are met.
    #[allow(clippy::mut_from_ref)]
    pub fn cx_mut(&self) -> &mut JSContext {
        // SAFETY: UnsafeCell provides interior mutability. See above.
        unsafe { &mut *(self.raw_cx.get() as *mut JSContext) }
    }

    /// Get the raw JSContext pointer.
    ///
    /// # Safety
    ///
    /// The returned pointer must only be used for operations that cannot
    /// trigger GC.
    pub unsafe fn raw_cx_no_gc(&self) -> *mut RawJSContext {
        *self.raw_cx.get()
    }

    /// Root an object pointer, returning a [`Handle`] tied to this scope.
    pub fn root_object(&self, obj: NonNull<JSObject>) -> Handle<'_, *mut JSObject> {
        let ptr = self.alloc_mut().alloc(SlotTag::Object, obj.as_ptr() as u64);
        // SAFETY: The pointer is in a traced page slot — it is a marked location.
        unsafe { Handle::from_marked_location(ptr as *const *mut JSObject) }
    }

    /// Root a value, returning a [`Handle`] tied to this scope.
    pub fn root_value(&self, val: Value) -> Handle<'_, Value> {
        // SAFETY: Value is repr(C) and always 8 bytes (u64).
        let bits = unsafe { std::mem::transmute::<Value, u64>(val) };
        let ptr = self.alloc_mut().alloc(SlotTag::Value, bits);
        unsafe { Handle::from_marked_location(ptr as *const Value) }
    }

    /// Root a string pointer, returning a [`Handle`] tied to this scope.
    pub fn root_string(&self, s: NonNull<JSString>) -> Handle<'_, *mut JSString> {
        let ptr = self.alloc_mut().alloc(SlotTag::String, s.as_ptr() as u64);
        unsafe { Handle::from_marked_location(ptr as *const *mut JSString) }
    }

    /// Root a script pointer, returning a [`Handle`] tied to this scope.
    pub fn root_script(&self, s: NonNull<JSScript>) -> Handle<'_, *mut JSScript> {
        let ptr = self.alloc_mut().alloc(SlotTag::Script, s.as_ptr() as u64);
        unsafe { Handle::from_marked_location(ptr as *const *mut JSScript) }
    }

    /// Root a property key (jsid), returning a [`Handle`] tied to this scope.
    pub fn root_id(&self, id: jsid) -> Handle<'_, jsid> {
        let bits = unsafe { std::mem::transmute_copy::<jsid, u64>(&id) };
        let ptr = self.alloc_mut().alloc(SlotTag::Id, bits);
        unsafe { Handle::from_marked_location(ptr as *const jsid) }
    }

    /// Root a symbol pointer, returning a [`Handle`] tied to this scope.
    pub fn root_symbol(&self, sym: NonNull<Symbol>) -> Handle<'_, *mut Symbol> {
        let ptr = self.alloc_mut().alloc(SlotTag::Symbol, sym.as_ptr() as u64);
        unsafe { Handle::from_marked_location(ptr as *const *mut Symbol) }
    }

    /// Root a function pointer, returning a [`Handle`] tied to this scope.
    pub fn root_function(&self, fun: NonNull<JSFunction>) -> Handle<'_, *mut JSFunction> {
        let ptr = self
            .alloc_mut()
            .alloc(SlotTag::Function, fun.as_ptr() as u64);
        unsafe { Handle::from_marked_location(ptr as *const *mut JSFunction) }
    }

    /// Root a BigInt pointer, returning a [`Handle`] tied to this scope.
    pub fn root_bigint(&self, bi: NonNull<BigInt>) -> Handle<'_, *mut BigInt> {
        let ptr = self.alloc_mut().alloc(SlotTag::BigInt, bi.as_ptr() as u64);
        unsafe { Handle::from_marked_location(ptr as *const *mut BigInt) }
    }

    /// Create a nested inner scope.
    ///
    /// The inner scope has its own allocator backed by separate pages. Values
    /// rooted in the inner scope are freed when it drops, without affecting
    /// the parent scope's roots.
    ///
    /// The returned [`InnerScope`] dereferences to [`Scope`] and can be used
    /// anywhere a `&Scope` is accepted.
    pub fn inner_scope(&self) -> InnerScope<'_> {
        InnerScope {
            scope: Scope::new(unsafe { *self.raw_cx.get() }),
        }
    }

    /// Get a handle to the global object of the current realm.
    pub fn global(&self) -> Object<'_> {
        use mozjs::rust::wrappers2::CurrentGlobal;
        // SAFETY: We have an entered realm. CurrentGlobal returns a pointer to
        // the global which is rooted by the realm. We root a copy in our scope.
        let global_ptr = unsafe { *CurrentGlobal(self.cx()) };
        // SAFETY: The global object is always non-null when a realm has been entered.
        let nn = unsafe { NonNull::new_unchecked(global_ptr) };
        Object::from_handle(self.root_object(nn))
    }
}

// ---------------------------------------------------------------------------
// RootScope — owning scope with realm management
// ---------------------------------------------------------------------------

/// An owning scope that manages realm entry and contains a [`Scope`].
///
/// `'cx` is the lifetime of the parent context.
/// `S` is a typestate marker ([`NoRealm`] or [`EnteredRealm`]).
///
/// For `EnteredRealm`, the `RootScope` `Deref`s to `&Scope<'cx>`,
/// providing all rooting methods. For `NoRealm`, only `cx()`/`cx_mut()`
/// and `enter_realm()` are available.
pub struct RootScope<'cx, S> {
    /// Raw pointer to the JSContext (for pre-realm operations).
    raw_cx: UnsafeCell<*mut RawJSContext>,

    /// The inner [`Scope`] with its own allocator.
    /// `None` for `NoRealm` (no rooting before entering a realm).
    /// Always `Some` for `EnteredRealm`.
    scope: Option<Scope<'cx>>,

    /// The `JSAutoRealm` that entered the realm (if any).
    /// `None` for `NoRealm` scopes and for scopes created from an existing
    /// realm (e.g. `from_current_realm`).
    ///
    /// Not read directly — kept for its `Drop` side-effect (leaving the realm).
    #[allow(dead_code)]
    realm: Option<JSAutoRealm>,

    _state: PhantomData<S>,
    _phantom: PhantomData<&'cx mut ()>,
}

// --- Constructors ---

impl<'cx> RootScope<'cx, NoRealm> {
    /// Create a new root scope without entering a realm.
    ///
    /// Use [`enter_realm`](RootScope::enter_realm) to enter a realm and obtain
    /// a `RootScope<'_, EnteredRealm>`.
    pub fn new(cx: &'cx mut JSContext) -> RootScope<'cx, NoRealm> {
        let raw_cx = unsafe { cx.raw_cx() };
        RootScope {
            raw_cx: UnsafeCell::new(raw_cx),
            scope: None,
            realm: None,
            _state: PhantomData,
            _phantom: PhantomData,
        }
    }

    /// Enter the realm of the given global object, transitioning this scope
    /// to `EnteredRealm`.
    pub fn enter_realm(self, global: NonNull<JSObject>) -> RootScope<'cx, EnteredRealm> {
        // SAFETY: Single-threaded SpiderMonkey access.
        let raw_cx_ptr = unsafe { *self.raw_cx.get() };
        let realm = JSAutoRealm::new(raw_cx_ptr, global.as_ptr());

        // Forget self — NoRealm has no alloc, so Drop is trivial, but
        // we avoid running it anyway to match the ownership transfer pattern.
        std::mem::forget(self);

        // Create the Scope with its own allocator now that a realm is entered.
        // SAFETY: pool is valid for the lifetime of the Runtime.
        let scope = Scope::new(raw_cx_ptr);

        RootScope {
            raw_cx: UnsafeCell::new(raw_cx_ptr),
            scope: Some(scope),
            realm: Some(realm),
            _state: PhantomData,
            _phantom: PhantomData,
        }
    }
}

impl<'cx> RootScope<'cx, EnteredRealm> {
    /// Create a scope that enters the given realm.
    ///
    /// This is a convenience that combines `new` + `enter_realm`.
    pub fn new_with_realm(
        cx: &'cx mut JSContext,
        global: NonNull<JSObject>,
    ) -> RootScope<'cx, EnteredRealm> {
        RootScope::new(cx).enter_realm(global)
    }

    /// Create a new global object and enter its realm in one step.
    ///
    /// This is the primary way to bootstrap a `Scope` — no `unsafe` needed at
    /// the call site:
    ///
    /// ```ignore
    /// use crate::gc::scope::RootScope;
    /// use mozjs::rust::{JSEngine, Runtime, SIMPLE_GLOBAL_CLASS, RealmOptions};
    ///
    /// let engine = JSEngine::init().unwrap();
    /// let mut rt = Runtime::new(engine.handle());
    /// let scope = RootScope::new_global(rt.cx(), &SIMPLE_GLOBAL_CLASS, RealmOptions::default());
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `JS_NewGlobalObject` returns null (e.g., out of memory).
    pub fn new_global(
        cx: &'cx mut JSContext,
        class: &JSClass,
        options: mozjs::rust::RealmOptions,
    ) -> RootScope<'cx, EnteredRealm> {
        let scope_no_realm = RootScope::new(cx);
        let global = unsafe {
            mozjs::rust::wrappers2::JS_NewGlobalObject(
                scope_no_realm.cx_mut(),
                class,
                std::ptr::null_mut(),
                OnNewGlobalHookOption::FireOnNewGlobalHook,
                &*options,
            )
        };
        let global = NonNull::new(global).expect("JS_NewGlobalObject returned null");
        scope_no_realm.enter_realm(global)
    }

    /// Create a scope when a realm is already entered (e.g. from a callback).
    ///
    /// # Safety
    ///
    /// The caller must ensure that a realm is currently entered on `cx`.
    pub unsafe fn from_current_realm(cx: &'cx mut JSContext) -> RootScope<'cx, EnteredRealm> {
        let raw_cx = cx.raw_cx();
        let scope = Scope::new(raw_cx);
        RootScope {
            raw_cx: UnsafeCell::new(raw_cx),
            scope: Some(scope),
            realm: None,
            _state: PhantomData,
            _phantom: PhantomData,
        }
    }
}

// --- Deref to Scope (EnteredRealm only) ---

impl<'cx> std::ops::Deref for RootScope<'cx, EnteredRealm> {
    type Target = Scope<'cx>;

    fn deref(&self) -> &Scope<'cx> {
        // SAFETY: scope is always Some when S = EnteredRealm — all
        // EnteredRealm constructors set scope to Some.
        self.scope.as_ref().unwrap()
    }
}

// --- JSContext access (all states) ---

impl<'cx, S> RootScope<'cx, S> {
    /// Get a reference to the underlying [`JSContext`].
    pub fn cx(&self) -> &JSContext {
        unsafe { &*(self.raw_cx.get() as *const JSContext) }
    }

    /// Get a mutable reference to the underlying [`JSContext`].
    ///
    /// Uses interior mutability — see [`Scope::cx_mut`] for the rationale.
    #[allow(clippy::mut_from_ref)]
    pub fn cx_mut(&self) -> &mut JSContext {
        unsafe { &mut *(self.raw_cx.get() as *mut JSContext) }
    }

    /// Get the raw JSContext pointer.
    ///
    /// # Safety
    ///
    /// The returned pointer must only be used for operations that cannot
    /// trigger GC.
    pub unsafe fn raw_cx_no_gc(&self) -> *mut RawJSContext {
        *self.raw_cx.get()
    }
}

// ---------------------------------------------------------------------------
// InnerScope — nested scope with its own allocator
// ---------------------------------------------------------------------------

/// A nested scope that owns its own allocator backed by separate pages.
///
/// Created by [`Scope::inner_scope`]. Dereferences to `&Scope`, so all
/// rooting methods and `cx()`/`cx_mut()` are available. Each inner scope
/// has disjoint storage from its parent, allowing concurrent use of both.
pub struct InnerScope<'parent> {
    scope: Scope<'parent>,
}

impl<'parent> InnerScope<'parent> {
    /// Create a further nested scope.
    pub fn inner_scope(&self) -> InnerScope<'_> {
        self.scope.inner_scope()
    }
}

impl<'parent> std::ops::Deref for InnerScope<'parent> {
    type Target = Scope<'parent>;

    fn deref(&self) -> &Scope<'parent> {
        &self.scope
    }
}
