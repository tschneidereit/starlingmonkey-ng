// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Implicit non-constructibility for classes without `#[constructor]`.
//!
//! A `#[jsmethods]` block that declares no `#[constructor]` yields a class
//! that is not constructible from JS: `new Foo()` throws a `TypeError`
//! ("Illegal constructor"). An unannotated constructor-shaped fn — either
//! `fn any_name() -> Self` or the setup-style `fn new(&self, ...)` — is still
//! treated as a Rust-side constructor, so `Foo::new(scope, ...)` keeps
//! working.
// This file contains nothing platform-specific, so skip it on wasm32.
#![cfg(not(target_arch = "wasm32"))]

use core_runtime::config::RuntimeConfig;
use core_runtime::runtime::Runtime;
use core_runtime::test_util::eval_with_setup;
use core_runtime::{jsclass, jsmethods};

/// Old-style Rust constructor (`fn new() -> Self`), no `#[constructor]`.
#[jsclass]
struct RustOnly {
    n: i32,
}

#[jsmethods]
impl RustOnly {
    fn new() -> Self {
        Self { n: 7 }
    }

    #[method]
    fn n(&self) -> i32 {
        self.data().n
    }
}

/// Factory detection is by shape, not by name: a no-receiver fn returning
/// `Self` is a Rust-side factory whatever it's called, and the generated
/// factory carries the same name.
#[jsclass]
struct RenamedCtor {
    n: i32,
}

#[jsmethods]
impl RenamedCtor {
    fn from_parts(n: i32) -> Self {
        Self { n }
    }
}

/// Several Rust-side factories, each named after its own fn.
#[jsclass]
struct ManyFactories {
    n: i32,
}

#[jsmethods]
impl ManyFactories {
    fn new() -> Self {
        Self { n: 0 }
    }

    fn from_n(n: i32) -> Self {
        Self { n }
    }

    fn doubled(n: i32) -> Self {
        Self { n: n * 2 }
    }
}

/// An extra factory alongside a JS constructor: `new` belongs to the
/// `#[constructor]`, the extra keeps its own name.
#[jsclass]
struct CtorPlusFactory {
    n: i32,
}

#[jsmethods]
impl CtorPlusFactory {
    #[constructor]
    fn new(n: i32) -> Self {
        Self { n }
    }

    fn doubled(n: i32) -> Self {
        Self { n: n * 2 }
    }

    #[getter]
    fn n(&self) -> i32 {
        self.data().n
    }
}

/// Setup-style Rust constructor (`fn new(&self, ...)`), no `#[constructor]`.
#[jsclass]
struct RustOnlySetup {
    n: i32,
}

#[jsmethods]
impl RustOnlySetup {
    fn new(&self, n: i32) -> Result<(), js::error::ExnThrown> {
        self.data_mut().n = n;
        Ok(())
    }
}

/// No constructor of any kind: `add_to_global` must still be generated.
#[jsclass]
struct NoCtorAtAll {
    n: i32,
}

#[jsmethods]
impl NoCtorAtAll {
    #[static_method]
    fn answer() -> i32 {
        42
    }
}

/// Positive control: an explicit `#[constructor]` stays JS-constructible.
#[jsclass]
struct Constructible {
    n: i32,
}

#[jsmethods]
impl Constructible {
    #[constructor]
    fn new(n: i32) -> Self {
        Self { n }
    }

    #[getter]
    fn n(&self) -> i32 {
        self.data().n
    }
}

fn setup() {
    core_runtime::runtime::register_global_initializer(|scope, global| {
        RustOnly::add_to_global(scope, global);
        RenamedCtor::add_to_global(scope, global);
        ManyFactories::add_to_global(scope, global);
        CtorPlusFactory::add_to_global(scope, global);
        RustOnlySetup::add_to_global(scope, global);
        NoCtorAtAll::add_to_global(scope, global);
        Constructible::add_to_global(scope, global);
    });
}

/// `new Foo()` must throw a `TypeError` when no `#[constructor]` is present.
#[test]
fn js_construction_throws() {
    for class in [
        "RustOnly",
        "RenamedCtor",
        "ManyFactories",
        "RustOnlySetup",
        "NoCtorAtAll",
    ] {
        let result = eval_with_setup(
            setup,
            &format!(
                "try {{ new {class}(); 'no-throw' }} \
                 catch (e) {{ (e instanceof TypeError) + ':' + e.message }}"
            ),
        );
        assert_eq!(result, "true:Illegal constructor", "for class {class}");
    }
}

/// The interface object is still installed and usable (statics, prototype).
#[test]
fn interface_object_still_installed() {
    let result = eval_with_setup(setup, "NoCtorAtAll.answer()");
    assert_eq!(result, "42");
}

/// A class that does declare a `#[constructor]` stays constructible from JS.
#[test]
fn explicit_constructor_still_works() {
    let result = eval_with_setup(setup, "new Constructible(5).n");
    assert_eq!(result, "5");
}

/// `Foo::new(scope)` still works from Rust for the old-style shape.
#[test]
fn rust_side_new_works() {
    setup();
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();
    let obj = RustOnly::new(&scope).unwrap();
    assert_eq!(obj.data().n, 7);
    assert_eq!(obj.n(), 7);
}

/// The factory keeps the source fn's name: `fn from_parts` becomes
/// `Foo::from_parts(scope, …)`, and no `Foo::new` is conjured up.
#[test]
fn rust_side_renamed_ctor_works() {
    setup();
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();
    let obj = RenamedCtor::from_parts(&scope, 3).unwrap();
    assert_eq!(obj.data().n, 3);
}

/// Every constructor-shaped fn gets a factory under its own name.
#[test]
fn rust_side_multiple_factories() {
    setup();
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();
    assert_eq!(ManyFactories::new(&scope).unwrap().data().n, 0);
    assert_eq!(ManyFactories::from_n(&scope, 6).unwrap().data().n, 6);
    assert_eq!(ManyFactories::doubled(&scope, 6).unwrap().data().n, 12);
}

/// An extra factory coexists with a JS constructor: `new` is the
/// `#[constructor]`'s, and the extra is reachable only from Rust.
#[test]
fn rust_side_factory_alongside_js_constructor() {
    let js = eval_with_setup(
        setup,
        "new CtorPlusFactory(5).n + ':' + typeof CtorPlusFactory.doubled",
    );
    assert_eq!(js, "5:undefined");

    setup();
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();
    assert_eq!(CtorPlusFactory::new(&scope, 5).unwrap().data().n, 5);
    assert_eq!(CtorPlusFactory::doubled(&scope, 5).unwrap().data().n, 10);
}

/// `Foo::new(scope, args)` still works from Rust for the setup-style shape.
#[test]
fn rust_side_setup_new_works() {
    setup();
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();
    let obj = RustOnlySetup::new(&scope, 11).unwrap();
    assert_eq!(obj.data().n, 11);
}
