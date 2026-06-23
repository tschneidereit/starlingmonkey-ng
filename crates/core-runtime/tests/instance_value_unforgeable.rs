// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Regression test: a method returning `Self` (the `InstanceValue` codegen
//! path) must mint instances that carry `[LegacyUnforgeable]` own accessors,
//! exactly like the constructor and Rust-side factory paths. The JS-native
//! trampoline used to skip `install_unforgeable`, so an instance returned from
//! a JS method call lacked the own accessor entirely.

use core_runtime::jsclass;
use core_runtime::jsmethods;
use core_runtime::test_util::eval_with_setup;

#[jsclass]
struct Widget {
    id: i32,
}

#[jsmethods]
impl Widget {
    #[constructor]
    fn construct() -> Self {
        Self { id: 7 }
    }

    /// `[LegacyUnforgeable]`: an own accessor on each instance, not the prototype.
    #[getter(unforgeable)]
    fn kind(&self) -> i32 {
        42
    }

    /// Returns a fresh instance (the `InstanceValue` path).
    #[method]
    fn dup(&self) -> Self {
        Self { id: self.id }
    }
}

fn setup() {
    core_runtime::runtime::register_global_initializer(|scope, global| {
        Widget::add_to_global(scope, global);
    });
}

#[test]
fn dup_instance_has_unforgeable_own_accessor() {
    assert_eq!(
        eval_with_setup(
            setup,
            "const d = new Widget().dup(); \
             typeof Object.getOwnPropertyDescriptor(d, 'kind') === 'object'"
        ),
        "true"
    );
    assert_eq!(eval_with_setup(setup, "new Widget().dup().kind"), "42");
}

#[test]
fn unforgeable_accessor_is_not_on_prototype() {
    assert_eq!(
        eval_with_setup(
            setup,
            "Object.getOwnPropertyDescriptor(Widget.prototype, 'kind') === undefined"
        ),
        "true"
    );
}
