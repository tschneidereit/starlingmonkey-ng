// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Regression test for the class-typed argument private-data guard.
//!
//! A class's prototype object shares the instance `JSClass` but carries no
//! private data (`set_private` only runs during construction). When such an
//! object is passed where a class instance is expected, the cast must fail:
//! without the guard it would succeed and a later `data()`/`data_mut()` would
//! `unwrap_unchecked` a `None`, dereferencing a null private pointer (type
//! confusion reachable from script).

use core_runtime::jsclass;
use core_runtime::jsmethods;
use core_runtime::test_util::{eval_with_setup, throws_with_setup};

#[jsclass]
struct Holder {
    value: i32,
}

#[jsmethods]
impl Holder {
    #[constructor]
    fn construct() -> Self {
        Self { value: 7 }
    }

    /// Reads the argument's private data, so an unguarded cast would crash here.
    #[method]
    fn other_value(&self, other: Holder<'_>) -> i32 {
        other.data().value
    }
}

fn setup() {
    core_runtime::runtime::register_global_initializer(|scope, global| {
        Holder::add_to_global(scope, global);
    });
}

#[test]
fn accepts_real_instance() {
    assert_eq!(
        eval_with_setup(setup, "new Holder().otherValue(new Holder())"),
        "7"
    );
}

#[test]
fn rejects_prototype_argument() {
    assert!(throws_with_setup(
        setup,
        "new Holder().otherValue(Holder.prototype)"
    ));
}

#[test]
fn rejects_plain_object_argument() {
    assert!(throws_with_setup(setup, "new Holder().otherValue({})"));
}
