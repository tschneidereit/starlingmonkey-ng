// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Regression test for `#[jsglobals]` class registration via `pub use`.
//!
//! Only `pub use Foo;` (a plain name) was registered; a grouped
//! `pub use {A, B};` or a renamed `pub use Foo as Bar;` was silently skipped,
//! so the class was never installed on the global. All forms are now handled.

use core_runtime::test_util::eval_with_setup;
use core_runtime::{jsclass, jsglobals, jsmethods};

#[jsclass]
struct Alpha {}

#[jsmethods]
impl Alpha {
    #[constructor]
    fn construct() -> Self {
        Self {}
    }

    #[getter]
    fn tag(&self) -> String {
        "alpha".to_string()
    }
}

#[jsclass]
struct Beta {}

#[jsmethods]
impl Beta {
    #[constructor]
    fn construct() -> Self {
        Self {}
    }

    #[getter]
    fn tag(&self) -> String {
        "beta".to_string()
    }
}

#[jsglobals]
mod grouped_globals {
    pub use super::{Alpha, Beta};
}

fn setup() {
    core_runtime::runtime::register_global_initializer(|scope, global| {
        grouped_globals::add_to_global(scope, global);
    });
}

#[test]
fn grouped_use_registers_all_classes() {
    assert_eq!(eval_with_setup(setup, "typeof Alpha"), "function");
    assert_eq!(eval_with_setup(setup, "typeof Beta"), "function");
    assert_eq!(eval_with_setup(setup, "new Alpha().tag"), "alpha");
    assert_eq!(eval_with_setup(setup, "new Beta().tag"), "beta");
}
