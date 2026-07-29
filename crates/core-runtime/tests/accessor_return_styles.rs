// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Accessor return-style codegen regression test.
//!
//! A fallible setter whose body returns `Result<T, E>` with a non-unit `T`
//! (classified `ResultValue`) used to fall into a catch-all arm that ran the
//! call and discarded the `Result`, silently swallowing the `Err`. The setter
//! must instead throw the error; its `Ok` value is ignored (a JS setter yields
//! undefined).

// This file contains nothing platform-specific, so skip it on wasm32.
#![cfg(not(target_arch = "wasm32"))]

use core_runtime::jsclass;
use core_runtime::jsmethods;
use core_runtime::test_util::{eval_with_setup, throws_with_setup};
use js::error::TypeError;

#[jsclass]
struct Cell {
    value: i32,
}

#[jsmethods]
impl Cell {
    #[constructor]
    fn construct() -> Self {
        Self { value: 0 }
    }

    #[getter]
    fn value(&self) -> i32 {
        self.data().value
    }

    #[setter]
    fn set_value(&mut self, v: i32) -> Result<i32, TypeError> {
        if v < 0 {
            return Err(TypeError("value must be non-negative".into()));
        }
        self.data_mut().value = v;
        Ok(v)
    }
}

fn setup() {
    core_runtime::runtime::register_global_initializer(|scope, global| {
        Cell::add_to_global(scope, global);
    });
}

#[test]
fn setter_ok_assigns() {
    assert_eq!(
        eval_with_setup(setup, "const c = new Cell(); c.value = 5; c.value"),
        "5"
    );
}

#[test]
fn setter_err_throws_not_swallowed() {
    assert!(throws_with_setup(
        setup,
        "const c = new Cell(); c.value = -1;"
    ));
}

#[test]
fn setter_err_leaves_value_unchanged() {
    assert_eq!(
        eval_with_setup(
            setup,
            "const c = new Cell(); c.value = 9; try { c.value = -1; } catch (e) {} c.value"
        ),
        "9"
    );
}
