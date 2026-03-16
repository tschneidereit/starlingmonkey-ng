// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Integration tests for the `js` API.
//!
//! Tests that need a JS runtime are grouped in a single test because
//! `JSEngine` can only be initialized once per process.

use core_runtime::config::RuntimeConfig;
use core_runtime::runtime::Runtime;

use js::builtins::IsPrimitive;
use js::builtins::{Boolean, Double, Int32, Null, StringPrimitive, SymbolPrimitive, Undefined};
use js::conversion::{ConversionBehavior, FromJSVal, ToJSVal};
use js::error::{CapturedError, ExnThrown};
use js::rooted;
use js::value;
use js::Array;
use js::Date;
use js::Map;
use js::Object;
use js::Promise;
use js::Set;

// --- Pure tests (no engine needed) ---

#[test]
fn test_value_constructors() {
    let v = value::undefined();
    assert!(v.is_undefined());

    let v = value::null();
    assert!(v.is_null());

    let v = value::from_bool(true);
    assert!(v.is_boolean());
    assert!(v.to_boolean());

    let v = value::from_i32(42);
    assert!(v.is_int32());
    assert_eq!(v.to_int32(), 42);

    let v = value::from_u32(100);
    assert!(v.is_int32() || v.is_double());

    let v = value::from_f64(3.14);
    assert!(v.is_double());
    assert!((v.to_double() - 3.14).abs() < f64::EPSILON);
}

#[test]
fn test_value_constructors_extra() {
    // value:: constructors cover the same ground as the old IntoJSVal impls.
    let v = value::from_bool(true);
    assert!(v.is_boolean());
    assert!(v.to_boolean());

    let v = value::from_i32(42);
    assert!(v.is_int32());
    assert_eq!(v.to_int32(), 42);

    let v = value::from_u32(99);
    assert!(v.is_int32() || v.is_double());

    let v = value::from_f64(2.71);
    assert!(v.is_double());

    let v = value::undefined();
    assert!(v.is_undefined());
}

#[test]
fn test_jserror_check() {
    assert!(ExnThrown::check(true).is_ok());
    assert!(ExnThrown::check(false).is_err());
    match ExnThrown::check(false) {
        Err(ExnThrown) => {}
        other => panic!("expected ExnThrown, got {other:?}"),
    }
}

#[test]
fn test_jserror_display() {
    let e = ExnThrown;
    assert_eq!(format!("{e}"), "JavaScript exception pending");

    let e = CapturedError {
        message: Some("bad thing".into()),
        filename: Some("test.js".into()),
        lineno: 10,
        column: 5,
        stack: None,
    };
    assert_eq!(format!("{e}"), "bad thing at test.js:10");
}

// --- Engine-dependent tests (single test function) ---

#[test]
fn test_js_api_with_runtime() {
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();

    // --- String operations ---
    let s = js::JSString::from_str(&scope, "hello world").unwrap();
    let result = s.to_utf8(&scope).unwrap();
    assert_eq!(result, "hello world");

    // --- Object creation ---
    let obj = Object::new(&scope, None).unwrap();
    assert!(!obj.as_raw().is_null());

    // --- Array creation ---
    let arr = Array::new(&scope, 5).unwrap();
    let len = arr.length(&scope).unwrap();
    assert_eq!(len, 5);

    // --- Evaluate and extract ---
    let rval = js::compile::evaluate(&scope, "2 + 3").unwrap();
    let result = i32::from_jsval(&scope, rval, ConversionBehavior::Default).unwrap();
    assert_eq!(result, 5);

    // --- GC operations ---
    js::gc::maybe_gc(&scope);
    js::gc::prepare_for_full_gc(&scope);

    // --- ToJSVal ---
    assert!(Option::<i32>::None.to_jsval(&scope).unwrap().is_null());
    assert_eq!(Some(42i32).to_jsval(&scope).unwrap().to_int32(), 42);
    assert_eq!(7i8.to_jsval(&scope).unwrap().to_int32(), 7);
    assert_eq!(300u16.to_jsval(&scope).unwrap().to_int32(), 300);
    assert!((1.5f32.to_jsval(&scope).unwrap().to_double() - 1.5).abs() < 0.001);

    // --- FromJSVal for bool ---
    {
        let v = scope.root_value(value::from_bool(false));
        assert_eq!(bool::from_jsval(&scope, v, ()).unwrap(), false);
    }

    // --- FromJSVal for i32 ---
    {
        let v = scope.root_value(value::from_i32(-7));
        assert_eq!(
            i32::from_jsval(&scope, v, ConversionBehavior::Default).unwrap(),
            -7
        );
    }

    // --- FromJSVal for f64 ---
    {
        let v = scope.root_value(value::from_f64(1.5));
        assert!((f64::from_jsval(&scope, v, ()).unwrap() - 1.5).abs() < f64::EPSILON);
    }

    // --- FromJSVal for u32 ---
    {
        let v = scope.root_value(value::from_u32(100));
        assert_eq!(
            u32::from_jsval(&scope, v, ConversionBehavior::Default).unwrap(),
            100
        );
    }

    // --- Comparison operations ---
    {
        rooted!(&in(&scope) let v1 = value::from_i32(42));
        rooted!(&in(&scope) let v2 = value::from_i32(42));
        rooted!(&in(&scope) let v3 = value::from_f64(42.0));

        // Strict equality: 42 === 42
        let eq = js::comparison::strictly_equal(&scope, v1.handle(), v2.handle()).unwrap();
        assert!(eq, "42 === 42 should be true");

        // Loose equality: 42 == 42.0
        let eq = js::comparison::loosely_equal(&scope, v1.handle(), v3.handle()).unwrap();
        assert!(eq, "42 == 42.0 should be true");

        // SameValue: Object.is(42, 42)
        let eq = js::comparison::same_value(&scope, v1.handle(), v2.handle()).unwrap();
        assert!(eq, "Object.is(42, 42) should be true");
    }

    // --- Map operations ---
    {
        let map = Map::new(&scope).unwrap();

        assert_eq!(map.size(&scope), 0);

        rooted!(&in(&scope) let key = value::from_i32(1));
        rooted!(&in(&scope) let val = value::from_i32(100));
        map.insert(&scope, key.handle(), val.handle()).unwrap();
        assert_eq!(map.size(&scope), 1);

        assert!(map.has(&scope, key.handle()).unwrap());

        let result_val = map.lookup(&scope, key.handle()).unwrap();
        assert_eq!(result_val.to_int32(), 100);

        assert!(map.delete(&scope, key.handle()).unwrap());
        assert_eq!(map.size(&scope), 0);
    }

    // --- Set operations ---
    {
        let set = Set::new(&scope).unwrap();

        assert_eq!(set.size(&scope), 0);

        rooted!(&in(&scope) let key = value::from_i32(42));
        set.add(&scope, key.handle()).unwrap();
        assert_eq!(set.size(&scope), 1);
        assert!(set.has(&scope, key.handle()).unwrap());

        assert!(set.delete(&scope, key.handle()).unwrap());
        assert_eq!(set.size(&scope), 0);
    }

    // --- Array builtins ---
    {
        let arr = Array::new(&scope, 3).unwrap();
        let len = arr.length(&scope).unwrap();
        assert_eq!(len, 3);
    }

    // --- Date builtins ---
    {
        let rval = js::compile::evaluate(&scope, "new Date(2024, 0, 15)").unwrap();
        let date_obj = Object::from_raw_obj(&scope, rval.to_object()).unwrap();
        assert!(Date::is_date(&scope, date_obj.handle()).unwrap());
        let js_date = date_obj.cast::<Date>().unwrap();
        assert!(js_date.is_valid(&scope).unwrap());
    }

    // --- Promise builtins ---
    {
        let rval = js::compile::evaluate(&scope, "new Promise(function(resolve) { resolve(42) })")
            .unwrap();
        let promise_obj = Object::from_raw_obj(&scope, rval.to_object()).unwrap();
        assert!(Promise::is_promise(promise_obj.handle()));
        let js_promise = promise_obj.cast::<Promise>().unwrap();
        let _id = js_promise.id();
    }

    // --- Primitive type checks ---
    {
        assert!(Undefined::is_value(value::undefined()));
        assert!(!Undefined::is_value(value::null()));

        assert!(Null::is_value(value::null()));
        assert!(!Null::is_value(value::undefined()));

        assert!(Boolean::is_value(value::from_bool(true)));
        assert!(!Boolean::is_value(value::from_i32(1)));

        assert!(Int32::is_value(value::from_i32(42)));
        assert!(!Int32::is_value(value::from_f64(42.0)));

        assert!(Double::is_value(value::from_f64(3.14)));

        assert!(!StringPrimitive::is_value(value::from_i32(0)));
        assert!(!SymbolPrimitive::is_value(value::from_i32(0)));
    }

    // --- JSON operations ---
    {
        let rval = js::json::parse(&scope, r#"{"a": 1}"#).unwrap();
        assert!(rval.is_object());
    }

    // --- Compile operations ---
    {
        assert!(js::compile::is_compilable_unit(&scope, "2 + 3"));
        // Incomplete source — missing closing brace
        assert!(!js::compile::is_compilable_unit(&scope, "function f() {"));
    }

    // --- String char_at ---
    {
        let s = js::JSString::from_str(&scope, "ABC").unwrap();
        let ch = s.char_at(&scope, 0).unwrap();
        assert_eq!(ch, b'A' as u16);
        let ch = s.char_at(&scope, 2).unwrap();
        assert_eq!(ch, b'C' as u16);
    }

    // --- TryCatch operations ---
    {
        use js::try_catch::TryCatch;

        // TryCatch with no exception
        {
            let tc = TryCatch::new(&scope);
            let result = js::compile::evaluate(tc.scope(), "2 + 3");
            assert!(result.is_ok());
            assert!(!tc.has_caught(), "no exception should be pending");
        }

        // TryCatch catches a thrown exception
        {
            let tc = TryCatch::new(&scope);
            let result = js::compile::evaluate(tc.scope(), "throw new Error('test error')");
            assert!(result.is_err());
            assert!(tc.has_caught(), "exception should have been caught");

            // Inspect the exception value without clearing
            let exc = tc.exception();
            assert!(exc.is_some(), "should have an exception value");
            assert!(exc.unwrap().is_object(), "Error is an object");

            // Capture clears the exception
            let captured = tc.capture();
            assert!(
                captured.message.as_deref().unwrap().contains("test error"),
                "captured message should contain 'test error', got: {:?}",
                captured.message,
            );
            assert!(!tc.has_caught(), "capture should clear the exception");
        }

        // TryCatch reset clears exception
        {
            let tc = TryCatch::new(&scope);
            let _ = js::compile::evaluate(tc.scope(), "throw 'reset me'");
            assert!(tc.has_caught());
            tc.reset();
            assert!(!tc.has_caught(), "reset should clear exception");
        }

        // TryCatch rethrow re-sets exception
        {
            let tc = TryCatch::new(&scope);
            let _ = js::compile::evaluate(tc.scope(), "throw 42");
            assert!(tc.has_caught());
            let exc = tc.exception().unwrap();
            tc.reset();
            assert!(!tc.has_caught());
            tc.rethrow(exc);
            assert!(tc.has_caught(), "rethrow should re-set exception");

            // Clean up for subsequent tests
            tc.reset();
        }

        // TryCatch with syntax error
        {
            let tc = TryCatch::new(&scope);
            let result = js::compile::evaluate(tc.scope(), "function(");
            assert!(result.is_err());
            // Syntax errors are also caught
            let captured = tc.capture();
            assert!(captured.message.is_some());
        }
    }

    // --- Script .run() ---
    {
        let scope = scope.inner_scope();

        let script = js::compile::compile(&scope, "1 + 2").expect("compile should succeed");
        let result = js::compile::execute_script(&scope, script).expect("run should succeed");
        assert!(result.is_int32());
        assert_eq!(result.to_int32(), 3);
    }

    // --- Closure-based callbacks ---
    {
        use js::try_catch::TryCatch;

        let scope = scope.inner_scope();

        // Simple closure returning a constant
        {
            let fun = js::Function::new_closure(&scope, c"forty_two", 0, |_scope, _args| {
                Ok(value::from_i32(42))
            })
            .expect("new_closure should succeed");

            let fun_val = scope.root_value(fun.as_value());
            scope
                .global()
                .set_property(&scope, c"fortyTwo", fun_val)
                .expect("set_property");

            let result =
                js::compile::evaluate(&scope, "fortyTwo()").expect("evaluate should succeed");
            assert!(result.is_int32());
            assert_eq!(result.to_int32(), 42);
        }

        // Closure that reads arguments
        {
            let fun = js::Function::new_closure(&scope, c"add", 2, |_scope, args| {
                let a = args.get_i32(0).unwrap_or(0);
                let b = args.get_i32(1).unwrap_or(0);
                Ok(value::from_i32(a + b))
            })
            .expect("new_closure should succeed");

            let fun_val = scope.root_value(fun.as_value());
            scope
                .global()
                .set_property(&scope, c"add", fun_val)
                .expect("set_property");

            let result =
                js::compile::evaluate(&scope, "add(10, 32)").expect("evaluate should succeed");
            assert!(result.is_int32());
            assert_eq!(result.to_int32(), 42);
        }

        // Closure that returns an error
        {
            let fun = js::Function::new_closure(&scope, c"fail", 0, |_scope, _args| Err(ExnThrown))
                .expect("new_closure should succeed");

            let fun_val = scope.root_value(fun.as_value());
            scope
                .global()
                .set_property(&scope, c"fail", fun_val)
                .expect("set_property");

            let tc = TryCatch::new(&scope);
            let result = js::compile::evaluate(tc.scope(), "fail()");
            assert!(result.is_err(), "calling fail() should throw");
            assert!(tc.has_caught());
            tc.reset();
        }

        // Closure with CallbackArgs metadata
        {
            let fun = js::Function::new_closure(&scope, c"argInfo", 0, |_scope, args| {
                Ok(value::from_i32(args.len() as i32))
            })
            .expect("new_closure");

            let fun_val = scope.root_value(fun.as_value());
            scope
                .global()
                .set_property(&scope, c"argInfo", fun_val)
                .expect("set_property");

            let result = js::compile::evaluate(&scope, "argInfo(1, 2, 3)").expect("evaluate");
            assert_eq!(result.to_int32(), 3);
        }
    }
}
