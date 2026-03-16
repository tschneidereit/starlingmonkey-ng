// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Integration tests for scope-based rooting (`RootScope`).
//!
//! Tests that need a JS runtime are grouped in a single test because
//! `JSEngine` can only be initialized once per process.

use core_runtime::config::RuntimeConfig;
use core_runtime::runtime::Runtime;
use js::rooted;
use js::value;

#[test]
fn test_scope_rooting() {
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();

    // --- Basic rooting ---
    {
        // Root a string via the scope.
        let s = js::JSString::from_str(&scope, "scope test").unwrap();
        let result = s.to_utf8(&scope).unwrap();
        assert_eq!(result, "scope test");

        // Root an object via the scope.
        let obj = js::Object::new(&scope, None).unwrap();
        assert!(!obj.as_raw().is_null());

        // Root a value via the scope.
        let val = scope.root_value(value::from_i32(42));
        assert_eq!(val.get().to_int32(), 42);

        // Global handle works.
        let g = scope.global();
        assert!(!g.as_raw().is_null());
    }

    // --- Inner scope ---
    {
        // Root something in the outer scope.
        let outer_str = js::JSString::from_str(&scope, "outer").unwrap();

        {
            // Create an inner scope.
            let inner = scope.inner_scope();

            // Root something in the inner scope.
            let inner_str = js::JSString::from_str(&inner, "inner").unwrap();
            let result = inner_str.to_utf8(&inner).unwrap();
            assert_eq!(result, "inner");

            // Outer values are still accessible via the inner scope.
            let result = outer_str.to_utf8(&inner).unwrap();
            assert_eq!(result, "outer");
        }
        // Inner scope dropped — values rooted there are released.

        // Outer-scope values are still valid.
        let result = outer_str.to_utf8(&scope).unwrap();
        assert_eq!(result, "outer");
    }

    // --- GC survival ---
    {
        // Root some values.
        let s = js::JSString::from_str(&scope, "survives gc").unwrap();
        let obj = js::Object::new(&scope, None).unwrap();
        let val = scope.root_value(value::from_i32(99));

        // Trigger GC.
        js::gc::prepare_for_full_gc(&scope);
        js::gc::maybe_gc(&scope);

        // Values should still be valid after GC.
        let result = s.to_utf8(&scope).unwrap();
        assert_eq!(result, "survives gc");
        assert!(!obj.as_raw().is_null());
        assert_eq!(val.get().to_int32(), 99);
    }

    // --- Evaluate script ---
    {
        let rval = js::compile::evaluate(&scope, "1 + 2 + 3").unwrap();
        assert_eq!(rval.to_int32(), 6);
    }

    // --- Mixed rooted! and scope ---
    {
        // Use rooted! macro alongside scope-based rooting.
        rooted!(&in(&scope) let mut val = value::from_i32(10));

        // scope-based root
        let scope_val = scope.root_value(value::from_i32(20));

        // Both should work.
        assert_eq!(val.get().to_int32(), 10);
        assert_eq!(scope_val.get().to_int32(), 20);

        // Modify the rooted! value.
        val.set(value::from_i32(30));
        assert_eq!(val.get().to_int32(), 30);

        // scope value unchanged.
        assert_eq!(scope_val.get().to_int32(), 20);
    }

    // --- Many roots of the same type ---
    {
        // Root many strings — tests that the arena grows correctly.
        let mut handles = Vec::new();
        for i in 0..100 {
            let s = js::JSString::from_str(&scope, &format!("string_{i}")).unwrap();
            handles.push(s);
        }

        // Trigger GC to verify all roots are traced.
        js::gc::prepare_for_full_gc(&scope);
        js::gc::maybe_gc(&scope);

        // Verify all strings survived.
        for (i, h) in handles.iter().enumerate() {
            let result = h.to_utf8(&scope).unwrap();
            assert_eq!(result, format!("string_{i}"));
        }
    }

    // --- Nested inner scopes ---
    {
        let s0 = js::JSString::from_str(&scope, "level0").unwrap();
        {
            let inner1 = scope.inner_scope();
            let s1 = js::JSString::from_str(&inner1, "level1").unwrap();
            {
                let inner2 = inner1.inner_scope();
                let s2 = js::JSString::from_str(&inner2, "level2").unwrap();

                // All levels accessible.
                assert_eq!(s0.to_utf8(&inner2).unwrap(), "level0");
                assert_eq!(s1.to_utf8(&inner2).unwrap(), "level1");
                assert_eq!(s2.to_utf8(&inner2).unwrap(), "level2");
            }
            // inner2 dropped, s2 released.

            // s0 and s1 still valid.
            assert_eq!(s0.to_utf8(&inner1).unwrap(), "level0");
            assert_eq!(s1.to_utf8(&inner1).unwrap(), "level1");
        }
        // inner1 dropped, s1 released.

        // s0 still valid.
        assert_eq!(s0.to_utf8(&scope).unwrap(), "level0");
    }

    // --- Parent-scope rooting during inner scope lifetime ---
    //
    // This test verifies that rooting on the parent scope while an inner
    // scope is alive does NOT corrupt the parent's roots. With per-scope
    // page ownership, the parent and inner scope have disjoint storage,
    // so dropping the inner scope cannot affect the parent's handles.
    {
        let inner = scope.inner_scope();

        // Root on the parent scope while inner scope exists.
        let parent_str = js::JSString::from_str(&scope, "parent-rooted").unwrap();

        // Root on the inner scope.
        let _inner_str = js::JSString::from_str(&inner, "inner-rooted").unwrap();

        // Drop the inner scope — only inner-scope roots should be freed.
        drop(inner);

        // Trigger GC — parent_str must still be traced and survive.
        js::gc::prepare_for_full_gc(&scope);
        js::gc::maybe_gc(&scope);

        // parent_str should still be valid.
        let result = parent_str.to_utf8(&scope).unwrap();
        assert_eq!(result, "parent-rooted");
    }

    // --- Interleaved parent/inner rooting with GC ---
    //
    // A more aggressive version: interleave parent and inner allocations,
    // drop the inner scope, GC, then verify parent handles.
    {
        let inner = scope.inner_scope();

        let p1 = js::JSString::from_str(&scope, "parent-1").unwrap();
        let _i1 = js::JSString::from_str(&inner, "inner-1").unwrap();
        let p2 = js::JSString::from_str(&scope, "parent-2").unwrap();
        let _i2 = js::JSString::from_str(&inner, "inner-2").unwrap();
        let p3 = js::JSString::from_str(&scope, "parent-3").unwrap();

        drop(inner);

        // GC — all parent handles must survive.
        js::gc::prepare_for_full_gc(&scope);
        js::gc::maybe_gc(&scope);

        assert_eq!(p1.to_utf8(&scope).unwrap(), "parent-1");
        assert_eq!(p2.to_utf8(&scope).unwrap(), "parent-2");
        assert_eq!(p3.to_utf8(&scope).unwrap(), "parent-3");
    }
}
