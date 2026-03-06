// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Page-based allocator for GC root handles.
//!
//! The [`HandlePool`] is a page freelist that hands out fixed-size
//! [`Page`]s to individual scopes. Each scope owns its own pages and
//! performs bump allocation locally — no shared cursor, no aliasing
//! between scopes.
//!
//! This design guarantees that dropping one scope cannot invalidate
//! another scope's handles, which was an unsoundness issue with the
//! previous shared-cursor approach.
//!
//! # Architecture
//!
//! - **[`HandlePool`]**: Owns a freelist of reusable pages and maintains
//!   an intrusive linked list of active [`ScopeAlloc`]s for GC tracing.
//! - **[`ScopeAlloc`]**: A per-scope bump allocator. Each scope
//!   ([`RootScope`] or [`InnerScope`]) owns one. Allocates from a
//!   current page and requests new pages from the pool when full.
//! - **[`Page`]**: A fixed-size array of 128 tagged `u64` slots.
//!
//! The pool is wrapped in a [`PoolRooter`] — a `#[repr(C)]`
//! `CustomAutoRooter` that sits on the `autoGCRooters` stack for the
//! entire lifetime of the [`Runtime`]. During GC, the rooter walks the
//! active allocator list and traces each scope's live slots.
//!
//! [`RootScope`]: crate::gc::scope::RootScope
//! [`InnerScope`]: crate::gc::scope::InnerScope
//! [`Runtime`]: mozjs::rust::Runtime

use std::cell::{Cell, RefCell, UnsafeCell};
use std::ptr;

use mozjs::context::JSContext;
use mozjs::gc::{CustomAutoRooter, CustomTrace};
use mozjs::glue::{
    CallBigIntRootTracer, CallFunctionRootTracer, CallIdRootTracer, CallObjectRootTracer,
    CallScriptRootTracer, CallStringRootTracer, CallSymbolRootTracer, CallValueRootTracer,
};
use mozjs::jsapi::JSTracer;

/// Number of slots per page. 128 gives ~1.1 KB per page (128 × 8 bytes for
/// values + 128 bytes for tags), which fits comfortably in L1 cache and covers
/// most operations without a second allocation.
const PAGE_SIZE: usize = 128;

/// Type tag for a slot in the handle pool.
///
/// Used during GC tracing to dispatch to the correct `CallXxxRootTracer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SlotTag {
    Object = 0,
    Value = 1,
    String = 2,
    Script = 3,
    Id = 4,
    Symbol = 5,
    Function = 6,
    BigInt = 7,
}

/// A fixed-size page of root slots.
///
/// Tags and values are stored in parallel arrays to avoid alignment padding
/// that would waste memory with a per-slot struct.
pub struct Page {
    tags: [Cell<SlotTag>; PAGE_SIZE],
    values: [UnsafeCell<u64>; PAGE_SIZE],
}

impl Page {
    fn new() -> Self {
        // SAFETY: Cell<SlotTag> where SlotTag is repr(u8), and UnsafeCell<u64>,
        // are both safe to zero-initialize. Unused slots are never read by the
        // tracer (bounded by cursor).
        unsafe { std::mem::zeroed() }
    }
}

// ---------------------------------------------------------------------------
// ScopeAlloc — per-scope bump allocator
// ---------------------------------------------------------------------------

/// A per-scope bump allocator that owns its pages independently.
///
/// Each [`RootScope`](crate::gc::scope::RootScope) and
/// [`InnerScope`](crate::gc::scope::InnerScope) owns a `ScopeAlloc`.
/// Pages are obtained from the [`HandlePool`] freelist and returned on drop.
///
/// `ScopeAlloc` is registered in the pool's intrusive linked list of active
/// allocators so that the GC tracer can discover all live slots.
pub struct ScopeAlloc {
    /// The pool this allocator borrows pages from.
    pool: *const HandlePool,

    /// Current page being allocated into. `None` until the first allocation.
    current_page: Option<Box<Page>>,

    /// Slot index within the current page (0..PAGE_SIZE).
    cursor: usize,

    /// Previously filled pages owned by this scope.
    #[allow(clippy::vec_box)]
    full_pages: Vec<Box<Page>>,

    /// Intrusive linked-list pointers for the pool's active-allocator list.
    /// These are raw pointers because the list is manually managed.
    next: *mut ScopeAlloc,
    prev: *mut ScopeAlloc,
}

impl ScopeAlloc {
    /// Create a new scope allocator, boxed for address stability, and register
    /// it with the pool's active list.
    ///
    /// Returns a `Box<ScopeAlloc>` because the intrusive linked list requires
    /// a stable address. The box must not be moved out of its allocation.
    ///
    /// No page is allocated until the first `alloc` call (lazy initialization).
    ///
    /// # Safety
    ///
    /// `pool` must point to a valid `HandlePool` that outlives this `ScopeAlloc`.
    pub unsafe fn new(pool: *const HandlePool) -> Box<Self> {
        let pool_ref = unsafe { &*pool };
        let mut alloc = Box::new(ScopeAlloc {
            pool,
            current_page: None,
            cursor: 0,
            full_pages: Vec::new(),
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        });
        // Register using the heap address (stable across moves of the Box pointer).
        pool_ref.register(&mut *alloc);
        alloc
    }

    /// Allocate a slot for a rooted value.
    ///
    /// Returns a stable pointer to the value's `u64` storage. The pointer
    /// remains valid until this `ScopeAlloc` is dropped.
    pub fn alloc(&mut self, tag: SlotTag, value: u64) -> *mut u64 {
        // Ensure we have a page to allocate from.
        if self.current_page.is_none() {
            // SAFETY: pool pointer is valid for the lifetime of the Runtime.
            let pool = unsafe { &*self.pool };
            self.current_page = Some(pool.take_page());
        }

        // If the current page is full, move it to full_pages and get a new one.
        if self.cursor == PAGE_SIZE {
            let full = self.current_page.take().unwrap();
            self.full_pages.push(full);
            // SAFETY: pool pointer is valid for the lifetime of the Runtime.
            let pool = unsafe { &*self.pool };
            self.current_page = Some(pool.take_page());
            self.cursor = 0;
        }

        let page = self.current_page.as_ref().unwrap();
        page.tags[self.cursor].set(tag);
        let ptr = page.values[self.cursor].get();
        // SAFETY: This slot is at our current cursor position, not yet in use.
        unsafe { *ptr = value };
        self.cursor += 1;
        ptr
    }

    /// Get the pool pointer that this allocator borrows from.
    ///
    /// Used by `Scope::inner_scope()` to create a new `ScopeAlloc` from
    /// the same pool.
    pub fn pool(&self) -> *const HandlePool {
        self.pool
    }

    /// Trace all live slots for GC.
    ///
    /// # Safety
    ///
    /// Must only be called during GC tracing. `trc` must be a valid `JSTracer`.
    unsafe fn trace(&self, trc: *mut JSTracer) {
        // Trace all full pages (all PAGE_SIZE slots are live).
        for page in &self.full_pages {
            trace_page(trc, page, PAGE_SIZE);
        }
        // Trace the current page up to the cursor.
        if let Some(page) = &self.current_page {
            trace_page(trc, page, self.cursor);
        }
    }
}

impl Drop for ScopeAlloc {
    fn drop(&mut self) {
        // SAFETY: pool pointer is valid for the lifetime of the Runtime,
        // which outlives all scopes.
        let pool = unsafe { &*self.pool };

        // Unregister from the active allocator list.
        pool.unregister(self);

        // Return all pages to the pool freelist.
        if let Some(page) = self.current_page.take() {
            pool.return_page(page);
        }
        for page in self.full_pages.drain(..) {
            pool.return_page(page);
        }
    }
}

/// Trace `count` slots in a page.
///
/// # Safety
///
/// `trc` must be a valid `JSTracer`. `count` must be <= PAGE_SIZE.
unsafe fn trace_page(trc: *mut JSTracer, page: &Page, count: usize) {
    for i in 0..count {
        let tag = page.tags[i].get();
        let ptr = page.values[i].get();

        match tag {
            SlotTag::Object => {
                let obj_ptr = ptr as *mut *mut mozjs::jsapi::JSObject;
                if !(*obj_ptr).is_null() {
                    CallObjectRootTracer(trc, obj_ptr, c"pool-object".as_ptr());
                }
            }
            SlotTag::Value => {
                let val_ptr = ptr as *mut mozjs::jsapi::Value;
                CallValueRootTracer(trc, val_ptr, c"pool-value".as_ptr());
            }
            SlotTag::String => {
                let str_ptr = ptr as *mut *mut mozjs::jsapi::JSString;
                if !(*str_ptr).is_null() {
                    CallStringRootTracer(trc, str_ptr, c"pool-string".as_ptr());
                }
            }
            SlotTag::Script => {
                let scr_ptr = ptr as *mut *mut mozjs::jsapi::JSScript;
                if !(*scr_ptr).is_null() {
                    CallScriptRootTracer(trc, scr_ptr, c"pool-script".as_ptr());
                }
            }
            SlotTag::Id => {
                let id_ptr = ptr as *mut mozjs::jsapi::jsid;
                CallIdRootTracer(trc, id_ptr, c"pool-id".as_ptr());
            }
            SlotTag::Symbol => {
                let sym_ptr = ptr as *mut *mut mozjs::jsapi::JS::Symbol;
                if !(*sym_ptr).is_null() {
                    CallSymbolRootTracer(trc, sym_ptr, c"pool-symbol".as_ptr());
                }
            }
            SlotTag::Function => {
                let fun_ptr = ptr as *mut *mut mozjs::jsapi::JSFunction;
                if !(*fun_ptr).is_null() {
                    CallFunctionRootTracer(trc, fun_ptr, c"pool-function".as_ptr());
                }
            }
            SlotTag::BigInt => {
                let bi_ptr = ptr as *mut *mut mozjs::jsapi::JS::BigInt;
                if !(*bi_ptr).is_null() {
                    CallBigIntRootTracer(trc, bi_ptr, c"pool-bigint".as_ptr());
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// HandlePool — page freelist + active allocator registry
// ---------------------------------------------------------------------------

/// A page freelist and registry of active scope allocators.
///
/// The pool owns reusable pages and maintains an intrusive doubly-linked
/// list of active [`ScopeAlloc`]s. During GC, the tracer walks this list
/// to discover all live root slots.
///
/// # Thread safety
///
/// `HandlePool` uses interior mutability (`Cell`/`UnsafeCell`) and is
/// single-threaded, matching SpiderMonkey's threading model.
#[derive(Default)]
pub struct HandlePool {
    /// Freelist of reusable pages. Pages are returned here when scopes drop
    /// and reused by new scopes, avoiding repeated heap allocation.
    #[allow(clippy::vec_box)]
    freelist: UnsafeCell<Vec<Box<Page>>>,

    /// Head of the intrusive doubly-linked list of active `ScopeAlloc`s.
    /// Null when no scopes are active.
    active_head: Cell<*mut ScopeAlloc>,
}

thread_local! {
    static HANDLE_POOL: RefCell<Option<Box<PoolRooter>>> = const { RefCell::new(None) };
}

/// Create the handle pool for scope-based rooting.
///
/// The pool is inside a PoolRooter (CustomAutoRooter) so it is traced during
/// both minor and major GC via the autoGCRooters chain.
pub(crate) fn init_pool(cx: &mut JSContext) {
    let mut pool_rooter = Box::new(PoolRooter::new(HandlePool::new()));
    // SAFETY: `raw_cx()` requires unsafe. The Box gives us a stable
    // address so the autoGCRooters stack entry remains valid.
    unsafe {
        pool_rooter.add_to_root_stack(cx.raw_cx());
    }

    HANDLE_POOL.with(|hp| {
        let mut borrow = hp.borrow_mut();
        assert!(
            borrow.is_none(),
            "HandlePool already initialized on this thread"
        );
        *borrow = Some(pool_rooter);
    });
}

/// Get a raw pointer to the current thread's HandlePool.
///
/// The returned pointer is valid for the lifetime of the Runtime (until
/// [`shutdown`] is called). It is safe to store in `ScopeAlloc` because
/// the pool outlives all scopes.
///
/// # Panics
///
/// Panics if no pool has been initialized on this thread.
pub(crate) fn current_pool() -> *const HandlePool {
    HANDLE_POOL.with(|hp| {
        let borrow = hp.borrow();
        let rooter = borrow
            .as_ref()
            .expect("No HandlePool on this thread — has a Runtime been created?");
        // Deref through Box<PoolRooter> → PoolRooter → HandlePool
        let pool: &HandlePool = rooter;
        pool as *const HandlePool
    })
}

impl HandlePool {
    /// Create a new empty pool.
    pub fn new() -> Self {
        Self::default()
    }

    /// Take a page from the freelist, or allocate a new one.
    fn take_page(&self) -> Box<Page> {
        // SAFETY: Single-threaded access.
        let fl = unsafe { &mut *self.freelist.get() };
        fl.pop().unwrap_or_else(|| Box::new(Page::new()))
    }

    /// Return a page to the freelist for reuse.
    fn return_page(&self, page: Box<Page>) {
        // SAFETY: Single-threaded access.
        let fl = unsafe { &mut *self.freelist.get() };
        fl.push(page);
    }

    /// Register a `ScopeAlloc` in the active allocator list.
    ///
    /// Pushes to the head of the doubly-linked list.
    fn register(&self, alloc: *mut ScopeAlloc) {
        let head = self.active_head.get();
        // SAFETY: alloc is a valid pointer to a ScopeAlloc being constructed.
        unsafe {
            (*alloc).next = head;
            (*alloc).prev = ptr::null_mut();
            if !head.is_null() {
                (*head).prev = alloc;
            }
        }
        self.active_head.set(alloc);
    }

    /// Unregister a `ScopeAlloc` from the active allocator list.
    fn unregister(&self, alloc: *const ScopeAlloc) {
        // SAFETY: alloc is a valid pointer to a ScopeAlloc being dropped.
        // The linked-list pointers are valid because they were set by register().
        unsafe {
            let prev = (*alloc).prev;
            let next = (*alloc).next;
            if !prev.is_null() {
                (*prev).next = next;
            } else {
                // alloc was the head.
                self.active_head.set(next);
            }
            if !next.is_null() {
                (*next).prev = prev;
            }
        }
    }
}

unsafe impl CustomTrace for HandlePool {
    /// Trace all live slots across all active scope allocators.
    ///
    /// # Safety
    ///
    /// Must only be called during GC tracing (from a `JSTraceDataOp` callback).
    /// `trc` must be a valid `JSTracer` pointer.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn trace(&self, trc: *mut JSTracer) {
        let mut current = self.active_head.get();
        while !current.is_null() {
            // SAFETY: The linked list contains only valid ScopeAlloc pointers
            // because register/unregister maintain the invariant.
            unsafe {
                (*current).trace(trc);
                current = (*current).next;
            }
        }
    }
}

pub type PoolRooter = CustomAutoRooter<HandlePool>;

pub(crate) fn shutdown() {
    HANDLE_POOL.with(|hp| {
        let mut borrow = hp.borrow_mut();
        // Clear the pool rooter from the autoGCRooters stack.
        // SAFETY: This reverses the add_to_root_stack call in init_pool().
        unsafe {
            if let Some(rooter) = borrow.as_mut() {
                rooter.remove_from_root_stack();
            }
        }
        // Drop the pool rooter, which drops the HandlePool and all pages.
        // This must be done after removing from the root stack to avoid
        // use-after-free during GC tracing.
        *borrow = None;
    });
}
