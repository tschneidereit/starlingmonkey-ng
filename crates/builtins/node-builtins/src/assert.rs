// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

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

    #[getter]
    fn code(&self) -> String {
        "ERR_ASSERTION".to_string()
    }
    #[getter]
    fn name(&self) -> String {
        "AssertionError".to_string()
    }
    #[getter]
    fn message(&self) -> String {
        self.data().message.clone()
    }
    #[getter]
    fn operator(&self) -> String {
        self.data().operator.clone()
    }
    #[getter]
    fn generated_message(&self) -> bool {
        self.data().generated_message
    }
    #[getter]
    fn actual<'r>(&self, scope: &'r Scope<'_>) -> HandleValue<'r> {
        self.data().actual.get(scope)
    }
    #[getter]
    fn expected<'r>(&self, scope: &'r Scope<'_>) -> HandleValue<'r> {
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
    if args.argc_ > idx {
        *args.get(idx)
    } else {
        js::value::undefined()
    }
}

fn get_message(scope: &Scope<'_>, args: &CallArgs, idx: u32) -> Option<String> {
    if args.argc_ <= idx {
        return None;
    }
    let v = *args.get(idx);
    if v.is_undefined() {
        return None;
    }
    String::from_jsval(scope, scope.root_value(v), ()).ok()
}

// Returns (message, generated_message). generated_message is false when the
// caller supplied a custom message, matching the Node.js AssertionError spec.
fn extract_message(
    scope: &Scope<'_>,
    args: &CallArgs,
    idx: u32,
    default_msg: &str,
) -> (String, bool) {
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
/// Check whether a string property on `expected` is absent or matches the same
/// property on `caught`. Returns true if validation passes (no mismatch).
fn str_prop_matches(
    scope: &Scope<'_>,
    expected: &js::Object,
    caught: &js::Object,
    prop: &std::ffi::CStr,
) -> bool {
    let exp_v = match expected.get_property(scope, prop) {
        Ok(v) if v.is_string() => *v,
        Ok(_) => return true,  // property absent or not a string — nothing to validate
        Err(_) => { let _ = take_pending(scope); return true; }
    };
    let exp_str = match String::from_jsval(scope, scope.root_value(exp_v), ()) {
        Ok(s) => s,
        Err(_) => return true,
    };
    let caught_v = match caught.get_property(scope, prop) {
        Ok(v) => *v,
        Err(_) => { let _ = take_pending(scope); return false; }
    };
    let caught_str = String::from_jsval(scope, scope.root_value(caught_v), ()).unwrap_or_default();
    exp_str == caught_str
}

fn call_regexp_test(scope: &Scope<'_>, regexp: Value, string: Value) -> Result<bool, ExnThrown> {
    let regexp_obj = js::Object::from_value(scope, regexp).map_err(|_| ExnThrown)?;
    let test_fn = regexp_obj.get_property(scope, c"test")?;
    let test_rooted = scope.root_value(*test_fn);
    let result = js::Function::call_value(
        scope,
        regexp_obj.handle(),
        test_rooted,
        &[scope.root_value(string)],
    )?;
    Ok(bool::from_jsval(scope, scope.root_value(*result), ()).unwrap_or(false))
}

fn is_nan(v: Value) -> bool {
    v.is_double() && v.to_double().is_nan()
}

fn loosely_equal_or_nan(scope: &Scope<'_>, a: Value, b: Value) -> Result<bool, ExnThrown> {
    if loosely_equal(scope, scope.root_value(a), scope.root_value(b))? {
        return Ok(true);
    }
    Ok(is_nan(a) && is_nan(b))
}

/// Minimal deep equality check: calls JSON.stringify on both values via the global
/// JSON object, then compares the resulting strings. Handles primitives, arrays,
/// and plain objects. NaN is treated as equal to NaN.
fn deep_equal_values(
    scope: &Scope<'_>,
    a: Value,
    b: Value,
    strict: bool,
) -> Result<bool, ExnThrown> {
    if same_value_of(scope, a, b)? {
        return Ok(true);
    }
    if is_nan(a) && is_nan(b) {
        return Ok(true);
    }
    if !strict {
        if loosely_equal_or_nan(scope, a, b)? {
            return Ok(true);
        }
    }
    // For objects/arrays, compare via JSON.stringify on the global JSON object.
    let json = scope.global().get_property(scope, c"JSON")?;
    let json_obj = js::Object::from_value(scope, json).map_err(|_| ExnThrown)?;
    let stringify_fn = json_obj.get_property(scope, c"stringify")?;
    let stringify_rooted = scope.root_value(*stringify_fn);
    let a_str_val = js::Function::call_value(
        scope, json_obj.handle(), stringify_rooted, &[scope.root_value(a)],
    )?;
    let a_str = String::from_jsval(scope, scope.root_value(*a_str_val), ()).map_err(|_| ExnThrown)?;
    let b_str_val = js::Function::call_value(
        scope, json_obj.handle(), stringify_rooted, &[scope.root_value(b)],
    )?;
    let b_str = String::from_jsval(scope, scope.root_value(*b_str_val), ()).map_err(|_| ExnThrown)?;
    Ok(a_str == b_str)
}

fn is_regexp(scope: &Scope<'_>, obj: &js::Object) -> bool {
    match obj.get_property(scope, c"source") {
        Ok(v) => v.is_string(),
        Err(_) => {
            let _ = take_pending(scope);
            false
        }
    }
}

fn get_str_prop(scope: &Scope<'_>, obj: &js::Object, prop: &std::ffi::CStr) -> Option<String> {
    match obj.get_property(scope, prop) {
        Ok(v) if v.is_string() => String::from_jsval(scope, scope.root_value(*v), ()).ok(),
        Ok(_) => None,
        Err(_) => {
            let _ = take_pending(scope);
            None
        }
    }
}

fn validate_thrown(
    scope: &Scope<'_>,
    args: &CallArgs,
    msg_idx: u32,
    exp_val: Value,
    exp_obj: &js::Object,
    caught_val: Option<Value>,
) -> Result<(), ExnThrown> {
    let caught_or_undef = caught_val.unwrap_or(js::value::undefined());

    // Case 1: RegExp — match against error.message
    if is_regexp(scope, exp_obj) {
        let test_val = caught_val
            .and_then(|cv| {
                js::Object::from_value(scope, cv)
                    .ok()
                    .and_then(|obj| match obj.get_property(scope, c"message") {
                        Ok(v) if v.is_string() => Some(*v),
                        Ok(_) => None,
                        Err(_) => {
                            let _ = take_pending(scope);
                            None
                        }
                    })
            })
            .unwrap_or(caught_or_undef);

        let matched = match call_regexp_test(scope, exp_val, test_val) {
            Ok(b) => b,
            Err(_) => {
                let _ = take_pending(scope);
                false
            }
        };
        if matched {
            return Ok(());
        }
        let (msg, gen) = extract_message(
            scope, args, msg_idx,
            "The error did not match the regular expression.",
        );
        return Err(throw_assertion_error(scope, msg, "throws", caught_or_undef, exp_val, gen));
    }

    // Case 2: Callable — either an Error constructor or a validator function
    if exp_obj.is_callable() {
        let exp_name = get_str_prop(scope, exp_obj, c"name");
        let is_error_ctor = exp_name
            .as_ref()
            .map(|n| n.ends_with("Error"))
            .unwrap_or(false);

        if is_error_ctor {
            let caught_ctor_name = caught_val
                .and_then(|cv| js::Object::from_value(scope, cv).ok())
                .and_then(|obj| match obj.get_property(scope, c"constructor") {
                    Ok(ctor_v) => js::Object::from_value(scope, *ctor_v)
                        .ok()
                        .and_then(|ctor| get_str_prop(scope, &ctor, c"name")),
                    Err(_) => {
                        let _ = take_pending(scope);
                        None
                    }
                });

            if caught_ctor_name.as_ref() != exp_name.as_ref() {
                let (msg, gen) = extract_message(
                    scope, args, msg_idx,
                    "The error did not match the expected type.",
                );
                return Err(throw_assertion_error(scope, msg, "throws", caught_or_undef, exp_val, gen));
            }
            return Ok(());
        }

        // Validator function: call with the caught error, check truthy return
        if let Some(cv) = caught_val {
            let result = js::Function::call_value(
                scope,
                scope.global().handle(),
                scope.root_value(exp_val),
                &[scope.root_value(cv)],
            );
            match result {
                Ok(ret) if js_truthy(scope, *ret) => return Ok(()),
                Ok(_) => {
                    let (msg, gen) = extract_message(
                        scope, args, msg_idx,
                        "The validation function did not return true.",
                    );
                    return Err(throw_assertion_error(scope, msg, "throws", cv, exp_val, gen));
                }
                Err(ExnThrown) => {
                    let _ = take_pending(scope);
                    let (msg, gen) = extract_message(
                        scope, args, msg_idx,
                        "The validation function threw an exception.",
                    );
                    return Err(throw_assertion_error(scope, msg, "throws", cv, exp_val, gen));
                }
            }
        }
        return Ok(());
    }

    // Case 3: Plain object — validate code, name, and message properties
    let mismatch = caught_val
        .and_then(|cv| js::Object::from_value(scope, cv).ok())
        .map(|caught_obj| {
            !str_prop_matches(scope, exp_obj, &caught_obj, c"code")
                || !str_prop_matches(scope, exp_obj, &caught_obj, c"name")
                || !str_prop_matches(scope, exp_obj, &caught_obj, c"message")
        })
        .unwrap_or(true);

    if mismatch {
        let (msg, gen) = extract_message(
            scope, args, msg_idx,
            "The error did not match the expected.",
        );
        return Err(throw_assertion_error(scope, msg, "throws", caught_or_undef, exp_val, gen));
    }
    Ok(())
}

#[jsmodule(name = "node:assert")]
pub mod node_assert {
    use super::*;

    pub fn ok(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let value = value_at(args, 0);
        if js_truthy(scope, value) {
            return Ok(());
        }
        let (msg, gen) =
            extract_message(scope, args, 1, "The expression evaluated to a falsy value");
        Err(throw_assertion_error(
            scope,
            msg,
            "==",
            value,
            js::value::from_bool(true),
            gen,
        ))
    }

    pub fn fail(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let (msg, gen) = extract_message(scope, args, 0, "Failed");
        Err(throw_assertion_error(
            scope,
            msg,
            "fail",
            js::value::undefined(),
            js::value::undefined(),
            gen,
        ))
    }

    pub fn equal(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let actual = value_at(args, 0);
        let expected = value_at(args, 1);
        if loosely_equal_or_nan(scope, actual, expected)? {
            return Ok(());
        }
        let (msg, gen) = extract_message(scope, args, 2, "Values are not equal");
        Err(throw_assertion_error(
            scope, msg, "==", actual, expected, gen,
        ))
    }

    pub fn not_equal(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let actual = value_at(args, 0);
        let expected = value_at(args, 1);
        if !loosely_equal_or_nan(scope, actual, expected)? {
            return Ok(());
        }
        let (msg, gen) = extract_message(scope, args, 2, "Values are equal");
        Err(throw_assertion_error(
            scope, msg, "!=", actual, expected, gen,
        ))
    }

    pub fn strict_equal(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let actual = value_at(args, 0);
        let expected = value_at(args, 1);
        if same_value_of(scope, actual, expected)? {
            return Ok(());
        }
        let (msg, gen) = extract_message(scope, args, 2, "Values are not strictly equal");
        Err(throw_assertion_error(
            scope, msg, "===", actual, expected, gen,
        ))
    }

    pub fn not_strict_equal(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let actual = value_at(args, 0);
        let expected = value_at(args, 1);
        if !same_value_of(scope, actual, expected)? {
            return Ok(());
        }
        let (msg, gen) = extract_message(scope, args, 2, "Values are strictly equal");
        Err(throw_assertion_error(
            scope, msg, "!==", actual, expected, gen,
        ))
    }

    pub fn deep_equal(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let actual = value_at(args, 0);
        let expected = value_at(args, 1);
        if deep_equal_values(scope, actual, expected, false)? {
            return Ok(());
        }
        let (msg, gen) = extract_message(scope, args, 2, "Values are not deep equal");
        Err(throw_assertion_error(
            scope, msg, "==", actual, expected, gen,
        ))
    }

    pub fn deep_strict_equal(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let actual = value_at(args, 0);
        let expected = value_at(args, 1);
        if deep_equal_values(scope, actual, expected, true)? {
            return Ok(());
        }
        let (msg, gen) = extract_message(scope, args, 2, "Values are not deep strictly equal");
        Err(throw_assertion_error(
            scope, msg, "===", actual, expected, gen,
        ))
    }

    pub fn if_error(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let err = value_at(args, 0);
        // Pass if err is null or undefined
        if same_value_of(scope, err, js::value::null())?
            || same_value_of(scope, err, js::value::undefined())?
        {
            return Ok(());
        }
        // Re-throw the error value as-is
        js::exception::set_pending(scope, scope.root_value(err), ExceptionStackBehavior::Capture);
        Err(ExnThrown)
    }

    /// assert.match(string, regexp[, message])
    pub fn r#match(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let string = value_at(args, 0);
        let regexp = value_at(args, 1);
        let matches = call_regexp_test(scope, regexp, string)?;
        if matches { return Ok(()); }
        let (msg, gen) = extract_message(scope, args, 2, "The input did not match the regular expression");
        Err(throw_assertion_error(scope, msg, "match", string, regexp, gen))
    }

    /// assert.doesNotMatch(string, regexp[, message])
    pub fn does_not_match(scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        let string = value_at(args, 0);
        let regexp = value_at(args, 1);
        let matches = call_regexp_test(scope, regexp, string)?;
        if !matches { return Ok(()); }
        let (msg, gen) = extract_message(scope, args, 2, "The input was expected to not match the regular expression");
        Err(throw_assertion_error(scope, msg, "doesNotMatch", string, regexp, gen))
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
                let (msg, gen) =
                    extract_message(scope, args, msg_idx, "Missing expected exception.");
                let expected = expected_idx.map_or(js::value::undefined(), |i| value_at(args, i));
                Err(throw_assertion_error(
                    scope,
                    msg,
                    "throws",
                    js::value::undefined(),
                    expected,
                    gen,
                ))
            }
            Err(ExnThrown) => {
                let caught_val: Option<Value> = take_pending(scope).ok().map(|h| *h);

                if let Some(exp_idx) = expected_idx {
                    let exp_val = value_at(args, exp_idx);
                    if exp_val.is_object() {
                        if let Ok(exp_obj) = js::Object::from_value(scope, exp_val) {
                            return validate_thrown(
                                scope, args, msg_idx, exp_val, &exp_obj, caught_val,
                            );
                        }
                    }
                }
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
