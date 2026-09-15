// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! `AssociatedMemory` across a compacting GC.
//!
//! Memory reported through `JS::AddAssociatedMemory` is keyed by the object's address, and a
//! compacting GC relocates tenured objects. An address captured before such a collection names a
//! released arena afterwards, and the engine's own accounting has been rekeyed to the new one, so
//! removing through the old address crashes a debug build ("Association not found") and corrupts
//! the zone's malloc counters in a release build. Both the update path and the finalizer must
//! therefore use the object's current address.

// This file contains nothing platform-specific, so skip it on wasm32.
#![cfg(not(target_arch = "wasm32"))]
// These tests are only really useful with GC Zeal modes on, which requires debugmozjs.
#![cfg(feature = "debugmozjs")]

use std::cell::Cell;

use core_runtime::config::RuntimeConfig;
use core_runtime::runtime::Runtime;
use core_runtime::{jsclass, jsmethods};
use js::gc::{self, AssociatedMemory, GCOptions, GCReason, SetGCZeal};

thread_local! {
    /// Number of `Accounted` objects finalized so far.
    static FINALIZED: Cell<u32> = const { Cell::new(0) };
    /// Number of `Derived` objects finalized so far.
    static DERIVED_FINALIZED: Cell<u32> = const { Cell::new(0) };
}

#[jsclass]
struct Accounted {
    accounted: AssociatedMemory,
}

#[jsmethods]
impl Accounted {
    #[constructor]
    fn construct() -> Self {
        Self {
            accounted: AssociatedMemory::default(),
        }
    }

    /// Attribute `bytes` to this object, replacing the previous amount.
    #[method]
    fn account(&self, bytes: i32) {
        let mut data = self.data_mut();
        // SAFETY: `self` is a rooted handle to the object owning this private data.
        unsafe { data.accounted.set(self.as_raw(), bytes as usize) };
    }

    #[getter]
    fn bytes(&self) -> i32 {
        self.data().accounted.bytes() as i32
    }

    #[destructor]
    fn release(&mut self, object: *mut js::native::JSObject) {
        // SAFETY: `object` is the object being finalized, whose private data this is.
        unsafe { self.accounted.release(object) };
        FINALIZED.with(|n| n.set(n.get() + 1));
    }
}

/// A subclass with a destructor of its own.
#[jsclass(extends = Accounted)]
struct Derived {
    parent: AccountedImpl,
}

#[jsmethods]
impl Derived {
    #[constructor]
    fn construct() -> Self {
        Self {
            parent: AccountedImpl::construct(),
        }
    }

    #[destructor]
    fn note_finalization(&mut self) {
        DERIVED_FINALIZED.with(|n| n.set(n.get() + 1));
    }
}

/// A subclass without a destructor, so the chain has to pass straight through it.
#[jsclass(extends = Derived)]
struct Grandchild {
    parent: DerivedImpl,
}

#[jsmethods]
impl Grandchild {
    #[constructor]
    fn construct() -> Self {
        Self {
            parent: DerivedImpl::construct(),
        }
    }
}

fn eval(scope: &js::gc::scope::Scope<'_>, code: &str) {
    js::compile::evaluate_with_filename(scope, code, "test.js", 1)
        .unwrap_or_else(|_| panic!("evaluating {code:?} threw"));
}

/// Relocate every arena in the zone. Only a `DEBUG_GC`-reason compacting collection relocates
/// unconditionally, and zeal mode 14 (Compact) at frequency 1 turns each allocation into one.
fn compact_everything(scope: &js::gc::scope::Scope<'_>) {
    // SAFETY: `scope` is the active scope, and zeal is cleared again below.
    unsafe { SetGCZeal(scope.raw_cx_no_gc(), 14, 1) };
    {
        let inner = scope.inner_scope();
        for i in 0..8 {
            let s = js::JSString::from_str(&inner, &format!("mover_{i}")).unwrap();
            assert_eq!(s.to_utf8(&inner).unwrap(), format!("mover_{i}"));
        }
    }
    gc::prepare_for_full_gc(scope);
    gc::non_incremental_gc(scope, GCOptions::Shrink, GCReason::API);
    // SAFETY: as above.
    unsafe { SetGCZeal(scope.raw_cx_no_gc(), 0, 0) };
}

#[test]
fn associated_memory_survives_relocation() {
    core_runtime::runtime::register_global_initializer(|scope, global| {
        Accounted::add_to_global(scope, global);
    });
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();

    eval(
        &scope,
        "globalThis.held = new Accounted(); held.account(1048576);",
    );

    compact_everything(&scope);

    // Updating the amount after the object moved: the removal of the previous amount must go
    // through the object's new address.
    eval(
        &scope,
        "held.account(2097152); if (held.bytes !== 2097152) throw new Error('wrong amount');",
    );

    compact_everything(&scope);

    // Finalization after the object moved: the finalizer's address is the only valid one.
    eval(&scope, "globalThis.held = null;");
    gc::prepare_for_full_gc(&scope);
    gc::non_incremental_gc(&scope, GCOptions::Shrink, GCReason::API);

    assert_eq!(FINALIZED.with(|n| n.get()), 1);
}

/// A subclass inherits its parent's private data, and with it the parent's accounting. The
/// parent's destructor must still run, at every depth of the hierarchy, or the bytes stay
/// attributed to an object that no longer exists.
#[test]
fn destructors_chain_to_the_parent_class() {
    core_runtime::runtime::register_global_initializer(|scope, global| {
        Grandchild::add_to_global(scope, global);
    });
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();

    eval(
        &scope,
        "globalThis.held = new Grandchild(); held.account(1048576);\
         if (held.bytes !== 1048576) throw new Error('inherited accessor missed the parent data');",
    );

    compact_everything(&scope);

    eval(&scope, "globalThis.held = null;");
    gc::prepare_for_full_gc(&scope);
    gc::non_incremental_gc(&scope, GCOptions::Shrink, GCReason::API);

    assert_eq!(DERIVED_FINALIZED.with(|n| n.get()), 1);
    assert_eq!(FINALIZED.with(|n| n.get()), 1);
}
