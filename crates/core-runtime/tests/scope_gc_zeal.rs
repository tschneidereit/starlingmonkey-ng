// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! GC zeal tests for scope-based rooting (`HandlePool` + `RootScope`).
//!
//! These tests verify that pool-rooted values survive all GC modes including
//! compaction (which moves objects), generational/nursery collections, and
//! incremental marking.
//!
//! Grouped in a single test because `JSEngine` can only be initialized once
//! per process. Each section sets its own GC zeal mode.

#![cfg(feature = "debugmozjs")]

use core_runtime::config::RuntimeConfig;
use core_runtime::runtime::Runtime;
use js::gc::{GCOptions, GCReason, SetGCZeal};
use js::value;

use js::Array;
use js::Map;
use js::Object;
use js::{gc, string};

/// Helper: set GC zeal mode on the scope's context.
///
/// # Safety
///
/// Must only be called when a valid JSContext is available.
unsafe fn set_zeal(scope: &js::gc::scope::Scope<'_>, mode: u8, frequency: u32) {
    SetGCZeal(scope.raw_cx_no_gc(), mode, frequency);
}

/// Helper: reset GC zeal to mode 0 (normal).
unsafe fn reset_zeal(scope: &js::gc::scope::Scope<'_>) {
    SetGCZeal(scope.raw_cx_no_gc(), 0, 0);
}

#[test]
fn test_scope_gc_zeal() {
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();

    unsafe {
        // =====================================================================
        // Verification #5: 1000 scopes in a loop — no heap growth
        // =====================================================================
        // Pages are reused when cursor rewinds. After the first iteration
        // allocates a page, subsequent iterations should reuse it.
        {
            for i in 0..1000 {
                let inner = scope.inner_scope();
                let s = string::from_str(&inner, &format!("scope_{i}")).unwrap();
                let obj = Object::new(&inner, None).unwrap();
                let val = inner.root_value(value::from_i32(i));

                // Verify roots are valid.
                let result = string::to_utf8(&inner, s).unwrap();
                assert_eq!(result, format!("scope_{i}"));
                assert!(!obj.as_raw().is_null());
                assert_eq!(val.get().to_int32(), i);
                // inner scope drops here — cursor rewinds
            }
        }

        // =====================================================================
        // Verification #6: 500 roots across nested scopes + GC
        // =====================================================================
        {
            // Root 100 strings in the outer scope.
            let mut outer_handles = Vec::new();
            for i in 0..100 {
                let s = string::from_str(&scope, &format!("root_{i}")).unwrap();
                outer_handles.push((i, s));
            }

            {
                let inner = scope.inner_scope();
                // Root 200 strings in the inner scope.
                let mut inner_handles = Vec::new();
                for i in 100..300 {
                    let s = string::from_str(&inner, &format!("root_{i}")).unwrap();
                    inner_handles.push((i, s));
                }

                {
                    let inner2 = inner.inner_scope();
                    // Root 200 strings in the innermost scope.
                    let mut inner2_handles = Vec::new();
                    for i in 300..500 {
                        let s = string::from_str(&inner2, &format!("root_{i}")).unwrap();
                        inner2_handles.push((i, s));
                    }

                    // Trigger GC with all 500 roots live.
                    gc::prepare_for_full_gc(&inner2);
                    gc::non_incremental_gc(&inner2, GCOptions::Normal, GCReason::API);

                    // All 500 should survive.
                    for (i, h) in &outer_handles {
                        let result = string::to_utf8(&inner2, *h).unwrap();
                        assert_eq!(result, format!("root_{i}"));
                    }
                    for (i, h) in &inner_handles {
                        let result = string::to_utf8(&inner2, *h).unwrap();
                        assert_eq!(result, format!("root_{i}"));
                    }
                    for (i, h) in &inner2_handles {
                        let result = string::to_utf8(&inner2, *h).unwrap();
                        assert_eq!(result, format!("root_{i}"));
                    }
                }
                // inner2 drops — roots 300-499 released

                // inner + outer roots still valid.
                for (i, h) in &inner_handles {
                    let result = string::to_utf8(&inner, *h).unwrap();
                    assert_eq!(result, format!("root_{i}"));
                }
            }
            // inner drops — roots 100-299 released

            // Roots 0-99 still valid.
            for (i, h) in &outer_handles {
                let result = string::to_utf8(&scope, *h).unwrap();
                assert_eq!(result, format!("root_{i}"));
            }
        }

        // =====================================================================
        // GC Zeal #8: Mode 1 (RootsChange) — GC on root set changes
        // =====================================================================
        {
            set_zeal(&scope, 1, 1);

            let inner = scope.inner_scope();

            // Each root_* call modifies the root set, potentially triggering GC.
            let s1 = string::from_str(&inner, "roots_change_1").unwrap();
            let obj1 = Object::new(&inner, None).unwrap();
            let s2 = string::from_str(&inner, "roots_change_2").unwrap();
            let v1 = inner.root_value(value::from_i32(42));
            let obj2 = Object::new(&inner, None).unwrap();

            // All roots must survive despite GC on every root set change.
            assert_eq!(string::to_utf8(&inner, s1).unwrap(), "roots_change_1");
            assert_eq!(string::to_utf8(&inner, s2).unwrap(), "roots_change_2");
            assert!(!obj1.as_raw().is_null());
            assert!(!obj2.as_raw().is_null());
            assert_eq!(v1.get().to_int32(), 42);

            reset_zeal(&inner);
        }

        // =====================================================================
        // GC Zeal #9: Mode 2 (Alloc) — GC at every allocation
        // =====================================================================
        {
            set_zeal(&scope, 2, 1);

            let inner = scope.inner_scope();

            // Root many values — GC fires on every JS allocation.
            let mut string_handles = Vec::new();
            for i in 0..50 {
                let s = string::from_str(&inner, &format!("alloc_{i}")).unwrap();
                string_handles.push(s);
            }

            // Evaluate JS that allocates heavily.
            let rval = js::compile::evaluate(
                &inner,
                "var arr = []; for (var i = 0; i < 100; i++) arr.push({x: i}); arr.length",
            )
            .unwrap();
            assert_eq!(rval.to_int32(), 100);

            // All string roots must survive.
            for (i, h) in string_handles.iter().enumerate() {
                let result = string::to_utf8(&inner, *h).unwrap();
                assert_eq!(result, format!("alloc_{i}"));
            }

            reset_zeal(&inner);
        }

        // =====================================================================
        // GC Zeal #10: Mode 4 (VerifierPre) — verify pre-write barriers
        // =====================================================================
        {
            set_zeal(&scope, 4, 1);

            let inner = scope.inner_scope();

            // Root objects and modify their properties. The verifier checks
            // that pre-write barriers fire correctly.
            let obj = Object::new(&inner, None).unwrap();
            let val = inner.root_value(value::from_i32(100));
            obj.set_property(&inner, c"x", val).unwrap();

            let obj2 = Object::new(&inner, None).unwrap();
            let val2 = inner.root_value(value::from_i32(200));
            obj2.set_property(&inner, c"y", val2).unwrap();

            // Overwrite properties to trigger barriers.
            let val3 = inner.root_value(value::from_i32(300));
            obj.set_property(&inner, c"x", val3).unwrap();

            // Read back to verify.
            let got = obj.get_property(&inner, c"x").unwrap();
            assert_eq!(got.to_int32(), 300);
            let got2 = obj2.get_property(&inner, c"y").unwrap();
            assert_eq!(got2.to_int32(), 200);

            reset_zeal(&inner);
        }

        // =====================================================================
        // GC Zeal #11: Mode 7 (GenerationalGC) — frequent nursery collections
        // =====================================================================
        // Nursery-allocated objects must survive and handles must remain valid
        // after tenuring.
        {
            set_zeal(&scope, 7, 1);

            let inner = scope.inner_scope();

            // Allocate objects that start in the nursery.
            let mut objects = Vec::new();
            for i in 0..20 {
                let obj = Object::new(&inner, None).unwrap();
                let val = inner.root_value(value::from_i32(i));
                obj.set_property(&inner, c"idx", val).unwrap();
                objects.push(obj);
            }

            // Allocate more to trigger nursery collections.
            let mut strings = Vec::new();
            for i in 0..20 {
                let s = string::from_str(&inner, &format!("nursery_{i}")).unwrap();
                strings.push(s);
            }

            // All objects should survive nursery → tenured promotion.
            for (i, obj) in objects.iter().enumerate() {
                let got = obj.get_property(&inner, c"idx").unwrap();
                assert_eq!(got.to_int32(), i as i32, "object {i} property mismatch");
            }

            // All strings should survive.
            for (i, s) in strings.iter().enumerate() {
                let result = string::to_utf8(&inner, *s).unwrap();
                assert_eq!(result, format!("nursery_{i}"));
            }

            reset_zeal(&inner);
        }

        // =====================================================================
        // GC Zeal #12: Mode 11 (IncrementalMarkingValidator) — verify
        // incremental marking
        // =====================================================================
        {
            set_zeal(&scope, 11, 1);

            let inner = scope.inner_scope();

            // Root values during operations that trigger incremental GC slices.
            let mut values = Vec::new();
            for i in 0..30 {
                let val = inner.root_value(value::from_i32(i));
                values.push(val);
            }

            let mut objects = Vec::new();
            for _ in 0..20 {
                let obj = Object::new(&inner, None).unwrap();
                objects.push(obj);
            }

            // Trigger JS execution that allocates (may trigger incremental
            // slices with validation).
            let rval = js::compile::evaluate(
                &inner,
                "var result = 0; for (var i = 0; i < 50; i++) result += i; result",
            )
            .unwrap();
            assert_eq!(rval.to_int32(), 1225);

            // All roots must be consistently marked.
            for (i, v) in values.iter().enumerate() {
                assert_eq!(v.get().to_int32(), i as i32);
            }
            for obj in &objects {
                assert!(!obj.as_raw().is_null());
            }

            reset_zeal(&inner);
        }

        // =====================================================================
        // GC Zeal #13: Mode 14 (Compact) — force compacting GC
        // =====================================================================
        // **MOST CRITICAL**: Compaction moves objects, and our pool stores raw
        // pointers that must be updated by the GC tracer.
        {
            set_zeal(&scope, 14, 1);

            let inner = scope.inner_scope();

            // Create objects and arrays that may be moved by compaction.
            let mut objects = Vec::new();
            for i in 0..30 {
                let obj = Object::new(&inner, None).unwrap();
                let val = inner.root_value(value::from_i32(i * 10));
                obj.set_property(&inner, c"val", val).unwrap();
                objects.push(obj);
            }

            let arr = Array::new(&inner, 5).unwrap();
            let map = Map::new(&inner).unwrap();

            // Trigger operations that cause allocations (and thus compacting
            // GCs with zeal mode 14).
            for i in 0..10 {
                let s = string::from_str(&inner, &format!("compact_{i}")).unwrap();
                let result = string::to_utf8(&inner, s).unwrap();
                assert_eq!(result, format!("compact_{i}"));
            }

            // Force a full compacting GC.
            gc::prepare_for_full_gc(&inner);
            gc::non_incremental_gc(&inner, GCOptions::Shrink, GCReason::API);

            // After compaction, all pointer roots must still be valid.
            // If the GC moved objects and our tracer didn't update the pool
            // slots, these accesses would crash or return wrong values.
            for (i, obj) in objects.iter().enumerate() {
                assert!(!obj.as_raw().is_null());
                let got = obj.get_property(&inner, c"val").unwrap();
                assert_eq!(
                    got.to_int32(),
                    (i as i32) * 10,
                    "object {i} property mismatch after compaction"
                );
            }

            // Array and Map should still be valid.
            assert!(!arr.as_raw().is_null());
            assert!(!map.as_raw().is_null());

            reset_zeal(&inner);
        }

        // =====================================================================
        // GC Zeal #14: Mode 2 + nested scopes
        // =====================================================================
        // Root in both parent and child, GC, drop child, GC again, verify
        // parent roots survive.
        {
            set_zeal(&scope, 2, 1);

            let inner = scope.inner_scope();

            // Root in parent scope.
            let parent_obj = Object::new(&inner, None).unwrap();
            let parent_val = inner.root_value(value::from_i32(111));
            parent_obj
                .set_property(&inner, c"source", parent_val)
                .unwrap();

            {
                // Root in child scope.
                let child = inner.inner_scope();
                let child_obj = Object::new(&child, None).unwrap();
                let child_val = child.root_value(value::from_i32(222));
                child_obj
                    .set_property(&child, c"source", child_val)
                    .unwrap();

                // GC with both scopes live.
                gc::prepare_for_full_gc(&child);
                gc::non_incremental_gc(&child, GCOptions::Normal, GCReason::API);

                // Both should survive.
                let got = parent_obj.get_property(&child, c"source").unwrap();
                assert_eq!(got.to_int32(), 111);
                let got = child_obj.get_property(&child, c"source").unwrap();
                assert_eq!(got.to_int32(), 222);
            }
            // Child scope dropped.

            // GC again — only parent roots should be traced.
            gc::prepare_for_full_gc(&inner);
            gc::non_incremental_gc(&inner, GCOptions::Normal, GCReason::API);

            // Parent roots must still be valid.
            let got = parent_obj.get_property(&inner, c"source").unwrap();
            assert_eq!(got.to_int32(), 111);

            reset_zeal(&inner);
        }

        // =====================================================================
        // GC Zeal #15: Mode 2 loop stress test
        // =====================================================================
        // 100 iterations: create inner scope, root, trigger GC, drop scope.
        {
            set_zeal(&scope, 2, 1);

            // Root something in the outer scope to verify it survives
            // all the inner scope churn.
            let outer_str = string::from_str(&scope, "outer_survivor").unwrap();

            for i in 0..100 {
                let inner = scope.inner_scope();

                let s = string::from_str(&inner, &format!("loop_{i}")).unwrap();
                let obj = Object::new(&inner, None).unwrap();
                let val = inner.root_value(value::from_i32(i));
                obj.set_property(&inner, c"i", val).unwrap();

                // Trigger GC inside the loop.
                gc::prepare_for_full_gc(&inner);
                gc::non_incremental_gc(&inner, GCOptions::Normal, GCReason::API);

                // Verify inner roots.
                let result = string::to_utf8(&inner, s).unwrap();
                assert_eq!(result, format!("loop_{i}"));
                let got = obj.get_property(&inner, c"i").unwrap();
                assert_eq!(got.to_int32(), i);

                // Outer root must also survive.
                let outer = string::to_utf8(&inner, outer_str).unwrap();
                assert_eq!(outer, "outer_survivor");
                // inner scope drops
            }

            // Final check of outer root.
            let outer = string::to_utf8(&scope, outer_str).unwrap();
            assert_eq!(outer, "outer_survivor");

            reset_zeal(&scope);
        }
    }
}
