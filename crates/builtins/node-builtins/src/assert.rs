// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

// Minimal native assert module. Only the functions needed by enabled tests:
//   ok / fail / equal / notEqual
//   strictEqual / notStrictEqual
//   throws
//
// TODO: deepEqual / deepStrictEqual / notDeepStrictEqual
// TODO: rejects / doesNotReject 
// TODO: throws — validate the expected constructor/RegExp/object (currently ignores it)
// TODO: AssertionError as a module export (currently only on globalThis)

use core_runtime::{jsmethods, jsmodule, webidl_interface};
use js::comparison::{loosely_equal, same_value};
use js::conversion::{FromJSVal, ToJSVal};
use js::error::ExnThrown;
use js::exception::{set_pending, take_pending};
use js::function::EmptyArgs;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::native::{CallArgs, ExceptionStackBehavior, Value};
use js::prelude::HandleValue;

#[webidl_interface(js_proto = "Error")]
pub struct AssertionError {
    message: String,
    operator: String,
    generated_message: bool,
    actual: Heap<Value>,
    expected: Heap<Value>,
}

#[jsmethods]
impl AssertionError {
    #[constructor]
    fn new(message: Option<String>) -> Self {
        Self {
            message: message.unwrap_or_else(|| "Assertion failed".to_string()),
            operator: String::new(),
            generated_message: true,
            actual: Heap::from(js::value::undefined()),
            expected: Heap::from(js::value::undefined()),
        }
    }

    #[getter] fn code(&self) -> String { "ERR_ASSERTION".to_string() }
    #[getter] fn name(&self) -> String { "AssertionError".to_string() }
    #[getter] fn message(&self) -> String { self.data().message.clone() }
    #[getter] fn operator(&self) -> String { self.data().operator.clone() }
    #[getter] fn generated_message(&self) -> bool { self.data().generated_message }
    #[getter] fn actual<'r>(&self, scope: &'r Scope<'_>) -> HandleValue<'r> {
        self.data().actual.get(scope)
    }
    #[getter] fn expected<'r>(&self, scope: &'r Scope<'_>) -> HandleValue<'r> {
        self.data().expected.get(scope)
    }
}

fn throw_assertion_error(
    scope: &Scope<'_>,
    message: String,
    operator: &str,
    actual: Value,
    expected: Value,
    generated_message: bool,
) -> ExnThrown {
    let result = unsafe {
        js::class::create_instance_with::<AssertionErrorImpl>(scope, |_| AssertionErrorImpl {
            message,
            operator: operator.to_string(),
            generated_message,
            actual: Heap::from(actual),
            expected: Heap::from(expected),
        })
    };
    match result {
        Ok(obj) => {
            let val = obj.to_jsval(scope).expect("AssertionError to jsval");
            set_pending(scope, val, ExceptionStackBehavior::Capture)
        }
        Err(e) => e,
    }
}

fn value_at(args: &CallArgs, idx: u32) -> Value {
    if args.argc_ > idx { *args.get(idx) } else { js::value::undefined() }
}

fn get_message(scope: &Scope<'_>, args: &CallArgs, idx: u32) -> Option<String> {
    if args.argc_ <= idx { return None; }
    let v = *args.get(idx);
    if v.is_undefined() { return None; }
    String::from_jsval(scope, scope.root_value(v), ()).ok()
}

// Returns (message, generated_message). generated_message is false when the
// caller supplied a custom message, matching the Node.js AssertionError spec.
fn extract_message(scope: &Scope<'_>, args: &CallArgs, idx: u32, default_msg: &str) -> (String, bool) {
    match get_message(scope, args, idx) {
        Some(msg) => (msg, false),
        None => (default_msg.to_string(), true),
    }
}

fn js_truthy(scope: &Scope<'_>, v: Value) -> bool {
    bool::from_jsval(scope, scope.root_value(v), ()).unwrap_or(false)
}

fn same_value_of(scope: &Scope<'_>, a: Value, b: Value) -> Result<bool, ExnThrown> {
    same_value(scope, scope.root_value(a), scope.root_value(b))
}

// NaN is the only value not equal to itself; check via the native double
// representation rather than routing through the JS engine.
fn is_nan(v: Value) -> bool {
    v.is_double() && v.to_double().is_nan()
}

fn loosely_equal_or_nan(scope: &Scope<'_>, a: Value, b: Value) -> Result<bool, ExnThrown> {
    if loosely_equal(scope, scope.root_value(a), scope.root_value(b))? { return Ok(true); }
    Ok(is_nan(a) && is_nan(b))
}

#[jsmodule(name = "node:assert")]
pub mod node_assert {
    use super::*;

    pub fn ok(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let value = value_at(args, 0);
        if js_truthy(scope, value) { return Ok(()); }
        let (msg, gen) = extract_message(scope, args, 1, "The expression evaluated to a falsy value");
        Err(throw_assertion_error(scope, msg, "==", value, js::value::from_bool(true), gen))
    }

    pub fn fail(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let (msg, gen) = extract_message(scope, args, 0, "Failed");
        Err(throw_assertion_error(scope, msg, "fail",
            js::value::undefined(), js::value::undefined(), gen))
    }

    pub fn equal(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let actual = value_at(args, 0);
        let expected = value_at(args, 1);
        if loosely_equal_or_nan(scope, actual, expected)? { return Ok(()); }
        let (msg, gen) = extract_message(scope, args, 2, "Values are not equal");
        Err(throw_assertion_error(scope, msg, "==", actual, expected, gen))
    }

    pub fn not_equal(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let actual = value_at(args, 0);
        let expected = value_at(args, 1);
        if !loosely_equal_or_nan(scope, actual, expected)? { return Ok(()); }
        let (msg, gen) = extract_message(scope, args, 2, "Values are equal");
        Err(throw_assertion_error(scope, msg, "!=", actual, expected, gen))
    }

    pub fn strict_equal(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let actual = value_at(args, 0);
        let expected = value_at(args, 1);
        if same_value_of(scope, actual, expected)? { return Ok(()); }
        let (msg, gen) = extract_message(scope, args, 2, "Values are not strictly equal");
        Err(throw_assertion_error(scope, msg, "===", actual, expected, gen))
    }

    pub fn not_strict_equal(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let actual = value_at(args, 0);
        let expected = value_at(args, 1);
        if !same_value_of(scope, actual, expected)? { return Ok(()); }
        let (msg, gen) = extract_message(scope, args, 2, "Values are strictly equal");
        Err(throw_assertion_error(scope, msg, "!==", actual, expected, gen))
    }

    pub fn throws(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let fn_rooted = scope.root_value(value_at(args, 0));

        // Node.js parameter shifting: throws(fn, message) vs throws(fn, expected, message).
        // If the second argument is a string it is the message, not the expected validator.
        let (expected_idx, msg_idx) = if args.argc_ >= 2 && value_at(args, 1).is_string() {
            (None, 1u32)
        } else {
            (Some(1u32), 2u32)
        };

        match js::Function::call_value(scope, scope.global().handle(), fn_rooted, EmptyArgs) {
            Ok(_) => {
                let (msg, gen) = extract_message(scope, args, msg_idx, "Missing expected exception.");
                let expected = expected_idx.map_or(js::value::undefined(), |i| value_at(args, i));
                Err(throw_assertion_error(scope, msg, "throws",
                    js::value::undefined(), expected, gen))
            }
            Err(ExnThrown) => {
                let _ = take_pending(scope);
                // TODO: validate expected_idx argument (constructor / RegExp / object)
                Ok(())
            }
        }
    }
}

pub mod assert_ns {
    use super::*;

    pub fn register(scope: &js::gc::scope::Scope<'_>) {
        unsafe {
            // Register AssertionError on globalThis for use in throws/catch checks.
            AssertionError::add_to_global(scope, scope.global());
            // Register node:assert as an importable ES module.
            node_assert::register(scope);
        }
    }
}
