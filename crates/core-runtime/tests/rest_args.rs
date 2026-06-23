// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Integration tests for typed variadic arguments via `RestArgs<T>`.
//!
//! `RestArgs<T>` collects every JS argument from the position where it appears
//! (after the fixed parameters) into a `Vec<T>`, converting each element with
//! `FromJSVal`. It is valid on `#[method]` and `#[static_method]`; the macro
//! rejects it on constructors and `#[jsnamespace]` functions, and rejects the
//! bare `RestArgs<Value>` element type (unrooted JSVals are not GC-safe).
//!
//! Supported inner types: `f64`/`f32`, `String`, `bool`, the integer types
//! (`i8`..`u64`, converted with `ConversionBehavior::Default`), class newtypes,
//! and `HandleValue<'_>` (each handle aliases the engine-rooted argv slot, so a
//! `Vec` of them is GC-safe — the [`handle_value_tests`] module proves this
//! under a forced collection). Only the owned `Value` element type and the bare
//! `RestArgs` default are rejected by the macro. Those compile-time rejections
//! aren't exercised here because the repo has no `trybuild` harness.

use core_runtime::jsclass;
use core_runtime::jsmethods;
use core_runtime::test_util::{eval_with_setup, throws_with_setup};

#[jsclass]
struct Variadic {}

#[jsmethods]
impl Variadic {
    #[constructor]
    fn construct() -> Self {
        Self {}
    }

    /// Typed `f64` variadic. Exercises `.iter()` and the lenient `ToNumber`
    /// coercion that `f64::from_jsval` applies to each argument.
    #[static_method]
    fn sum(rest: RestArgs<f64>) -> f64 {
        rest.iter().sum()
    }

    /// A fixed leading parameter followed by variadic strings. Exercises
    /// collection starting after the fixed args and `Deref`-to-slice (`join`).
    #[method]
    fn join(&self, sep: String, rest: RestArgs<String>) -> String {
        rest.join(sep.as_str())
    }

    /// Exercises `RestArgs::len`.
    #[method]
    fn count(&self, rest: RestArgs<String>) -> u32 {
        rest.len() as u32
    }

    /// Exercises `RestArgs::is_empty`.
    #[method]
    fn is_empty_rest(&self, rest: RestArgs<String>) -> bool {
        rest.is_empty()
    }

    /// Exercises the by-value `IntoIterator` impl.
    #[static_method]
    fn any_true(rest: RestArgs<bool>) -> bool {
        rest.into_iter().any(|b| b)
    }

    /// Variadic of class newtypes. A wrong-typed argument drives the
    /// `ConversionError::Failure` arm of the collection loop (a thrown
    /// `TypeError` from the failed `cast`), distinct from the `ExnPending`
    /// arm that a `String` conversion of a `Symbol` takes.
    #[static_method]
    fn count_selves(rest: RestArgs<Variadic>) -> u32 {
        rest.len() as u32
    }

    /// Typed `i32` variadic. Integer element types convert with
    /// `ConversionBehavior::Default` (`ToInt32`: truncate toward zero, then wrap
    /// modulo 2^32), not the `()` config that `f64`/`String` use.
    #[static_method]
    fn sum_ints(rest: RestArgs<i32>) -> i32 {
        rest.iter().sum()
    }

    /// A wide integer element type, confirming the fix spans `i8`..`u64` rather
    /// than `i32` alone. Returned as `f64` so JS sees the exact small sums.
    #[static_method]
    fn sum_longs(rest: RestArgs<i64>) -> f64 {
        rest.iter().sum::<i64>() as f64
    }
}

fn setup() {
    core_runtime::runtime::register_global_initializer(|scope, global| {
        Variadic::add_to_global(scope, global);
    });
}

fn eval(code: &str) -> String {
    eval_with_setup(setup, code)
}

fn throws(code: &str) -> bool {
    throws_with_setup(setup, code)
}

// ============================================================================
// Typed numeric collection
// ============================================================================

#[test]
fn sum_collects_numbers() {
    assert_eq!(eval("Variadic.sum(1, 2, 3)"), "6");
}

#[test]
fn sum_handles_floats() {
    assert_eq!(eval("Variadic.sum(1.5, 2.5)"), "4");
}

#[test]
fn sum_with_no_args_is_empty() {
    assert_eq!(eval("Variadic.sum()"), "0");
}

#[test]
fn sum_with_single_arg() {
    assert_eq!(eval("Variadic.sum(42)"), "42");
}

/// Each element is converted with the lenient `ToNumber`, so a numeric string
/// and a boolean coerce rather than throwing.
#[test]
fn sum_coerces_each_element() {
    assert_eq!(eval("Variadic.sum(1, '2', true)"), "4");
}

// ============================================================================
// Fixed parameter + variadic strings
// ============================================================================

#[test]
fn join_collects_after_fixed_param() {
    assert_eq!(eval("new Variadic().join('-', 'a', 'b', 'c')"), "a-b-c");
}

#[test]
fn join_with_empty_rest_is_empty_string() {
    assert_eq!(eval("new Variadic().join('-')"), "");
}

#[test]
fn join_with_single_rest_element() {
    assert_eq!(eval("new Variadic().join('-', 'solo')"), "solo");
}

/// The fixed `sep` argument is not collected into the rest; only arguments at
/// or beyond the rest position are.
#[test]
fn join_does_not_swallow_the_separator() {
    assert_eq!(eval("new Variadic().join(',', '1', '2')"), "1,2");
}

// ============================================================================
// len() / is_empty()
// ============================================================================

#[test]
fn count_reports_rest_length() {
    assert_eq!(eval("new Variadic().count('a', 'b', 'c', 'd')"), "4");
    assert_eq!(eval("new Variadic().count()"), "0");
}

#[test]
fn is_empty_rest_reflects_argument_count() {
    assert_eq!(eval("new Variadic().isEmptyRest()"), "true");
    assert_eq!(eval("new Variadic().isEmptyRest('x')"), "false");
}

// ============================================================================
// Boolean variadic (IntoIterator by value)
// ============================================================================

#[test]
fn any_true_finds_a_truthy_argument() {
    assert_eq!(eval("Variadic.anyTrue(false, false, true)"), "true");
}

#[test]
fn any_true_is_false_when_all_falsy() {
    assert_eq!(eval("Variadic.anyTrue(false, false)"), "false");
    assert_eq!(eval("Variadic.anyTrue()"), "false");
}

/// `ToBoolean` coerces each argument; truthy values count.
#[test]
fn any_true_coerces_arguments() {
    assert_eq!(eval("Variadic.anyTrue(0, '', 'non-empty')"), "true");
}

// ============================================================================
// Variadic of class newtypes
// ============================================================================

#[test]
fn count_selves_collects_instances() {
    assert_eq!(
        eval("Variadic.countSelves(new Variadic(), new Variadic())"),
        "2"
    );
    assert_eq!(eval("Variadic.countSelves()"), "0");
}

// ============================================================================
// Integer variadics (ConversionBehavior config)
// ============================================================================

#[test]
fn sum_ints_collects_integers() {
    assert_eq!(eval("Variadic.sumInts(1, 2, 3)"), "6");
    assert_eq!(eval("Variadic.sumInts()"), "0");
}

/// Each element runs through `ToInt32`: a non-integer number truncates toward
/// zero and a numeric string coerces.
#[test]
fn sum_ints_applies_to_int32() {
    assert_eq!(eval("Variadic.sumInts(1, 2.7, '3')"), "6");
    assert_eq!(eval("Variadic.sumInts(-1.9)"), "-1");
}

/// `ToInt32` wraps modulo 2^32, so 2^32 maps to 0.
#[test]
fn sum_ints_wraps_modulo_2_32() {
    assert_eq!(eval("Variadic.sumInts(4294967296)"), "0");
    assert_eq!(eval("Variadic.sumInts(4294967297)"), "1");
}

#[test]
fn sum_longs_handles_wide_integers() {
    assert_eq!(eval("Variadic.sumLongs(1000, 2000, 3000)"), "6000");
    assert_eq!(eval("Variadic.sumLongs()"), "0");
}

// ============================================================================
// .length excludes the rest parameter
// ============================================================================

/// A function's `.length` counts required (non-`Option`, non-`RestArgs`)
/// parameters, so the variadic tail never contributes.
#[test]
fn length_ignores_rest_parameter() {
    // Only the rest param: length 0.
    assert_eq!(eval("Variadic.sum.length"), "0");
    assert_eq!(eval("Variadic.anyTrue.length"), "0");
    assert_eq!(eval("Variadic.prototype.count.length"), "0");
    // One fixed param ahead of the rest: length 1.
    assert_eq!(eval("Variadic.prototype.join.length"), "1");
}

// ============================================================================
// Conversion-error propagation
// ============================================================================

/// `String::from_jsval` runs `ToString`, which throws on a `Symbol`. The
/// pending exception propagates out of the collection loop (the `ExnPending`
/// arm), so the call throws.
#[test]
fn string_rest_throws_on_symbol() {
    assert!(throws("new Variadic().count(Symbol('x'))"));
}

/// A wrong-typed argument to a class-newtype rest fails the `cast`, producing
/// a `ConversionError::Failure` that is thrown as a `TypeError` (the
/// `throw_type_error` arm).
#[test]
fn class_rest_throws_on_wrong_type() {
    assert!(throws("Variadic.countSelves({})"));
    assert!(throws("Variadic.countSelves(new Variadic(), 42)"));
}

// ============================================================================
// RestArgs<HandleValue> — raw rooted values
// ============================================================================
//
// `HandleValue` collects each argument as a handle that aliases the
// engine-rooted argv slot it was read from, so a `Vec<HandleValue>` held across
// a moving GC stays valid: the collector traces the argv on the VM stack and
// updates the slots in place. (An owned `RestArgs<Value>` would copy the JSVals
// out into untraced storage and is rejected by the macro for that reason.)

mod handle_value_tests {
    use core_runtime::jsclass;
    use core_runtime::jsmethods;
    use core_runtime::test_util::eval_with_setup;
    use js::prelude::{FromJSVal, HandleValue, Scope};

    #[jsclass]
    struct ValueVariadic {}

    #[jsmethods]
    impl ValueVariadic {
        #[constructor]
        fn construct() -> Self {
            Self {}
        }

        /// Collect raw `any` values into a `Vec<HandleValue>` and report the
        /// count — confirms the element type is accepted at all.
        #[static_method]
        fn count_values(rest: RestArgs<HandleValue<'_>>) -> u32 {
            rest.len() as u32
        }

        /// Hold the collected handles across a forced compacting collection,
        /// then read property `x` from each object. If the handles didn't alias
        /// the rooted argv, the relocated objects would be read through stale
        /// pointers and this would crash or return garbage.
        #[static_method]
        fn sum_x_after_gc<'r>(scope: &'r Scope<'_>, rest: RestArgs<HandleValue<'_>>) -> i32 {
            // Force a full, compacting collection with the values still held.
            js::gc::prepare_for_full_gc(scope);
            js::gc::non_incremental_gc(scope, js::gc::GCOptions::Shrink, js::gc::GCReason::API);

            let mut total = 0;
            for v in rest.iter() {
                let obj =
                    js::Object::from_jsval(scope, *v, ()).expect("argument must be an object");
                let xv = obj.get_property(scope, c"x").expect("missing property x");
                total += if xv.is_int32() {
                    xv.to_int32()
                } else {
                    xv.to_double() as i32
                };
            }
            total
        }
    }

    fn setup() {
        core_runtime::runtime::register_global_initializer(|scope, global| {
            ValueVariadic::add_to_global(scope, global);
        });
    }

    fn eval(code: &str) -> String {
        eval_with_setup(setup, code)
    }

    #[test]
    fn collects_raw_values() {
        assert_eq!(eval("ValueVariadic.countValues(1, 'two', {}, null)"), "4");
        assert_eq!(eval("ValueVariadic.countValues()"), "0");
    }

    /// Calling `sumXAfterGc` runs a real compacting GC mid-method while the
    /// handles are live; the objects are freshly allocated (movable) and read
    /// back afterwards.
    #[test]
    fn handles_survive_compacting_gc() {
        assert_eq!(
            eval("ValueVariadic.sumXAfterGc({x: 1}, {x: 2}, {x: 3})"),
            "6"
        );
        assert_eq!(eval("ValueVariadic.sumXAfterGc()"), "0");
    }

    /// The same path under GC zeal mode 14 (compact-on-every-GC), which forces
    /// relocation deterministically rather than relying on the shrinking GC to
    /// move objects on this build.
    #[cfg(feature = "debugmozjs")]
    #[test]
    fn handles_survive_under_gc_zeal() {
        use core_runtime::config::RuntimeConfig;
        use core_runtime::runtime::Runtime;
        use js::gc::SetGCZeal;

        setup();
        let rt = Runtime::init(&RuntimeConfig::default());
        let scope = rt.default_global();

        // Mode 14 (Compact): every GC compacts, moving heap objects.
        unsafe { SetGCZeal(scope.raw_cx_no_gc(), 14, 1) };

        let val = js::compile::evaluate(
            &scope,
            "ValueVariadic.sumXAfterGc({x: 10}, {x: 20}, {x: 30})",
        )
        .expect("evaluation threw");
        assert_eq!(val.to_int32(), 60);

        unsafe { SetGCZeal(scope.raw_cx_no_gc(), 0, 0) };
    }
}
