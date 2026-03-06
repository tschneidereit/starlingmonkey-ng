// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! **Comprehensive showcase** of every `#[js*]` proc macro and the public API.
//!
//! This example is the "kitchen sink" — it demonstrates:
//!
//! - `#[jsclass]` with `extends =` (inheritance)
//! - `#[jsmethods]` with every annotation:
//!   `#[constructor]`, `#[method]`, `#[getter]`, `#[setter]`,
//!   `#[static_method]`, `#[destructor]`, `#[method(name = "...")]`
//! - `#[jsmodule]` — a native ES module with constants and functions
//! - `#[jsglobals]` — functions, constants, and classes (`pub use`) on the global
//! - `RestArgs<T>` (typed variadic arguments)
//! - `Result<T, String>` error-throwing methods
//! - `-> Self` return from methods/static methods (creates new JS objects)
//! - `StackNewtype::cast` / `upcast()` for type-checked casts
//! - Constructing objects from Rust via the stack newtype's `new()`
//! - Calling methods from Rust via forwarded stack newtype methods
//! - Loading an external `.js` ES module file that exercises everything

use std::ptr;

use js::compile::evaluate_with_filename;
use js::native::Value;
use js::string as jsstring;
use libstarling::class::StackNewtype;
use libstarling::config::RuntimeConfig;
use libstarling::module::evaluate_module;
use libstarling::runtime::Runtime;
use libstarling::{jsclass, jsglobals, jsmethods, jsmodule};

// ============================================================================
// #[jsclass]: a simple value class — Vec2
// ============================================================================

/// A 2D vector — demonstrates a simple class with constructor, getters,
/// methods, static methods, and name-overridden methods.
#[jsclass]
struct Vec2 {
    x: f64,
    y: f64,
}

#[jsmethods]
impl Vec2 {
    #[constructor]
    fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    #[getter]
    fn x(&self) -> f64 {
        self.x
    }

    #[getter]
    fn y(&self) -> f64 {
        self.y
    }

    /// Returns the Euclidean length.
    #[method]
    fn length(&self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    /// Returns a new Vec2 scaled by `factor`.
    #[method]
    fn scale(&self, factor: f64) -> Self {
        Self {
            x: self.x * factor,
            y: self.y * factor,
        }
    }

    /// A method whose JS name differs from its Rust name.
    #[method(name = "toString")]
    fn to_display(&self) -> String {
        format!("Vec2({}, {})", self.x, self.y)
    }

    /// Static factory method — available as `Vec2.origin()` in JS.
    #[static_method]
    fn origin() -> Self {
        Self { x: 0.0, y: 0.0 }
    }

    /// Sum arbitrary numbers — demonstrates typed `RestArgs<f64>`.
    ///
    /// Each variadic argument is automatically converted from a JS value
    /// to `f64` using the `FromJSValue` trait.
    #[static_method]
    fn sum(rest: RestArgs<f64>) -> f64 {
        rest.iter().sum()
    }

    #[destructor]
    fn destructor(&mut self) {
        // Destructor runs when GC collects the JS object.
        // Useful for releasing non-GC resources (file handles, etc.).
    }
}

// ============================================================================
// #[jsclass] with `extends =`: Shape → Circle / Rect hierarchy
// ============================================================================

/// Base class — every shape has a color.
#[jsclass]
struct Shape {
    color: String,
}

#[jsmethods]
impl Shape {
    #[constructor]
    fn new(color: String) -> Self {
        Self { color }
    }

    #[getter]
    fn color(&self) -> String {
        self.color.clone()
    }

    #[setter]
    fn set_color(&mut self, color: String) {
        self.color = color;
    }

    #[method]
    fn describe(&self) -> String {
        format!("Shape(color={})", self.color)
    }
}

/// Circle — extends Shape, adds a radius.
#[jsclass(extends = Shape)]
struct Circle {
    parent: Shape,
    radius: f64,
}

#[jsmethods]
impl Circle {
    #[constructor]
    fn new(color: String, radius: f64) -> Self {
        Self {
            parent: __ShapeInner::new(color),
            radius,
        }
    }

    #[getter]
    fn radius(&self) -> f64 {
        self.radius
    }

    #[method]
    fn area(&self) -> f64 {
        std::f64::consts::PI * self.radius * self.radius
    }

    #[method]
    fn describe(&self) -> String {
        format!(
            "Circle(color={}, radius={}, area={:.2})",
            self.parent.color,
            self.radius,
            std::f64::consts::PI * self.radius * self.radius,
        )
    }
}

/// Rect — extends Shape, adds width and height.
#[jsclass(extends = Shape)]
struct Rect {
    parent: Shape,
    width: f64,
    height: f64,
}

#[jsmethods]
impl Rect {
    #[constructor]
    fn new(color: String, width: f64, height: f64) -> Self {
        Self {
            parent: __ShapeInner::new(color),
            width,
            height,
        }
    }

    #[method]
    fn area(&self) -> f64 {
        self.width * self.height
    }

    #[method]
    fn describe(&self) -> String {
        format!(
            "Rect(color={}, {}x{}, area={})",
            self.parent.color,
            self.width,
            self.height,
            self.width * self.height,
        )
    }
}

// ============================================================================
// #[jsmodule]: native ES module
// ============================================================================

/// A native module importable as `import { pi, add, ... } from "math"`.
#[jsmodule]
mod math {
    pub const PI: f64 = std::f64::consts::PI;
    pub const E: f64 = std::f64::consts::E;

    pub fn add(a: f64, b: f64) -> f64 {
        a + b
    }

    pub fn multiply(a: f64, b: f64) -> f64 {
        a * b
    }

    pub fn greet(name: String) -> String {
        format!("Hello, {}!", name)
    }

    /// Demonstrates a fallible function — throws in JS on error.
    pub fn safe_divide(a: f64, b: f64) -> Result<f64, String> {
        if b == 0.0 {
            Err("Division by zero".to_string())
        } else {
            Ok(a / b)
        }
    }

    /// Clamps `value` into `[min, max]`.
    pub fn clamp(value: f64, min: f64, max: f64) -> f64 {
        if value < min {
            min
        } else if value > max {
            max
        } else {
            value
        }
    }
}

// ============================================================================
// #[jsglobals]: install functions/constants on the global
// ============================================================================

/// Functions, constants, and classes available without an import.
#[jsglobals]
mod app_globals {
    pub use super::Circle;
    pub use super::Rect;
    pub use super::Shape;
    pub use super::Vec2;

    pub const APP_NAME: &str = "StarlingMonkey Showcase";
    pub const APP_VERSION: &str = "0.1.0";

    /// Format a Unix timestamp into an ISO-8601-ish date string.
    pub fn format_timestamp(ts: f64) -> String {
        // Simplified — real code would use chrono or similar.
        format!(
            "1970-01-01T{:02}:{:02}:{:02}Z",
            (ts / 3600.0) as u64 % 24,
            (ts / 60.0) as u64 % 60,
            ts as u64 % 60
        )
    }

    /// Returns a "random" number between min and max.
    /// (Deterministic for test reproducibility.)
    pub fn random_between(min: f64, max: f64) -> f64 {
        // For a real implementation this would use rand.
        // Here we just return `min` for determinism.
        if min == max {
            min
        } else {
            (min + max) / 2.0
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Extract a Rust String from a JS string value.
fn val_to_string(scope: &js::gc::scope::Scope<'_>, val: &Value) -> String {
    assert!(val.is_string(), "Expected string value");
    let str_handle = scope.root_string(ptr::NonNull::new(val.to_string()).expect("null string"));
    jsstring::to_utf8(scope, str_handle).expect("utf8 conversion failed")
}

/// Check a JS boolean value stored on the global.
fn read_global_bool(scope: &js::gc::scope::Scope<'_>, name: &str) -> bool {
    let rval = evaluate_with_filename(scope, name, "check.js", 1).expect("Failed to read global");
    rval.to_boolean()
}

// ============================================================================
// Main — Rust-side API usage, then load the external JS file
// ============================================================================

fn main() {
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();
    let global = scope.global();

    // Register classes and install globals — classes are registered via
    // `pub use ClassName;` inside the #[jsglobals] block.
    unsafe { app_globals::add_to_global(&scope, global) };

    // Register native module.
    assert!(
        unsafe { math::register(&scope) },
        "Failed to register math module"
    );

    // ====================================================================
    // Part A: Rust-side usage of every feature
    // ====================================================================

    println!("=== Part A: Rust-side API usage ===\n");

    // --- #[jsmodule] functions from Rust ---
    println!("Module functions (Rust-side):");
    assert_eq!(math::add(2.0, 3.0), 5.0);
    assert_eq!(math::multiply(4.0, 5.0), 20.0);
    assert_eq!(math::greet("Rust".to_string()), "Hello, Rust!");
    assert_eq!(math::safe_divide(10.0, 4.0), Ok(2.5));
    assert!(math::safe_divide(1.0, 0.0).is_err());
    assert_eq!(math::clamp(15.0, 0.0, 10.0), 10.0);
    println!("  add(2, 3) = {}", math::add(2.0, 3.0));
    println!("  multiply(4, 5) = {}", math::multiply(4.0, 5.0));
    println!("  greet(\"Rust\") = {}", math::greet("Rust".to_string()));
    println!("  clamp(15, 0, 10) = {}", math::clamp(15.0, 0.0, 10.0));
    println!("  PASSED\n");

    // --- #[jsglobals] functions from Rust ---
    println!("Global functions (Rust-side):");
    assert_eq!(
        app_globals::format_timestamp(3661.0),
        "1970-01-01T01:01:01Z"
    );
    assert_eq!(app_globals::random_between(5.0, 5.0), 5.0);
    println!(
        "  format_timestamp(3661) = {}",
        app_globals::format_timestamp(3661.0)
    );
    println!(
        "  random_between(5, 5) = {}",
        app_globals::random_between(5.0, 5.0)
    );
    println!("  PASSED\n");

    // --- #[jsclass] construction from Rust via stack newtype ---
    println!("Class construction (Rust-side):");
    let v = Vec2::new(&scope, 3.0, 4.0);
    // Forwarded getters work from Rust via the stack newtype:
    assert_eq!(v.x(), 3.0);
    assert_eq!(v.y(), 4.0);
    println!("  Vec2::new(3, 4) => x={}, y={}", v.x(), v.y());
    // Forwarded methods:
    let len = v.length();
    assert!((len - 5.0).abs() < 1e-10);
    println!("  length() = {}", len);

    // Self-returning method — creates a new JS-backed Vec2 object:
    let scaled = v.scale(&scope, 2.0);
    assert_eq!(scaled.x(), 6.0);
    assert_eq!(scaled.y(), 8.0);
    println!("  scale(2) => x={}, y={}", scaled.x(), scaled.y());
    println!("  PASSED\n");

    // --- Inheritance: construction and upcast ---
    println!("Inheritance & upcast (Rust-side):");
    let circle = Circle::new(&scope, "blue".to_string(), 5.0);
    assert_eq!(circle.radius(), 5.0);
    println!("  Circle::new(\"blue\", 5) => radius={}", circle.radius());

    // Upcast Circle -> Shape
    let as_shape: Shape = circle.upcast();
    assert_eq!(as_shape.color(), "blue");
    println!("  circle.upcast::<Shape>().color() = {}", as_shape.color());

    // Setter: mutate color through the forwarded setter
    as_shape.set_color("green".to_string());
    assert_eq!(as_shape.color(), "green");
    println!("  shape.set_color(\"green\") => color={}", as_shape.color());
    println!("  PASSED\n");

    // --- cast() for type-checked downcasting ---
    println!("cast() downcast (Rust-side):");
    // Downcast back from Shape that's actually a Circle:
    let circle_back: Circle = as_shape
        .cast::<Circle>()
        .expect("Expected successful downcast to Circle");
    assert_eq!(circle_back.radius(), 5.0);
    println!(
        "  shape.cast::<Circle>().radius() = {}",
        circle_back.radius()
    );

    // Downcast to wrong type fails:
    let bad: Option<Rect> = as_shape.cast::<Rect>();
    assert!(bad.is_none());
    println!("  shape.cast::<Rect>() = None (correct)");
    println!("  PASSED\n");

    // --- Verify instanceof from JS ---
    println!("instanceof via JS:");
    let rval = evaluate_with_filename(
        &scope,
        r#"
const c = new Circle("red", 10);
const checks = [];
checks.push("Circle instanceof Shape: " + (c instanceof Shape));
checks.push("Circle instanceof Circle: " + (c instanceof Circle));
checks.push("Circle instanceof Rect: " + (c instanceof Rect));
checks.join(", ")
"#,
        "instanceof.js",
        1,
    )
    .expect("instanceof script failed");
    let s = val_to_string(&scope, &rval);
    println!("  {}", s);
    assert!(s.contains("Circle instanceof Shape: true"));
    assert!(s.contains("Circle instanceof Circle: true"));
    assert!(s.contains("Circle instanceof Rect: false"));
    println!("  PASSED\n");

    // ====================================================================
    // Part B: Load and run the external JS module
    // ====================================================================

    println!("=== Part B: External JS module ===\n");

    // Point the module loader at the examples directory so the JS file
    // can resolve relative imports.
    let examples_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples");

    // Read the JS source at compile time so the example works regardless
    // of the working directory.
    let js_source = include_str!("showcase.js");

    // Use the examples dir as the filename base so relative imports work.
    let js_path = examples_dir.join("showcase.js");
    let js_filename = js_path.to_str().expect("non-UTF-8 path");

    let result = unsafe { evaluate_module(&scope, js_source, js_filename) };
    assert!(result.is_ok(), "showcase.js module evaluation failed");

    // Verify the JS module set `__showcaseOk` on the global.
    assert!(
        read_global_bool(&scope, "globalThis.__showcaseOk"),
        "JS showcase did not set __showcaseOk = true"
    );
    println!("  showcase.js completed successfully");

    // Print the results array the JS module collected.
    let rval = evaluate_with_filename(
        &scope,
        "globalThis.__showcaseResults.join('\\n')",
        "results.js",
        1,
    )
    .expect("Failed to read showcase results");
    let results_str = val_to_string(&scope, &rval);
    for line in results_str.lines() {
        println!("  JS: {}", line);
    }
    println!("  PASSED\n");

    // ====================================================================
    // Summary
    // ====================================================================

    println!("=== All showcase tests passed! ===");
    println!();
    println!("Features demonstrated:");
    println!("  #[jsclass]          — Vec2, Shape, Circle, Rect");
    println!("  #[jsmethods]        — constructor, method, getter,");
    println!("                        setter, static_method, destructor,");
    println!("                        method(name = \"...\")");
    println!("  -> Self returns     — scale(), origin() return new JS objects");
    println!("  extends =           — Circle/Rect extend Shape");
    println!("  #[jsmodule]         — math module (constants + functions)");
    println!("  #[jsglobals]        — app_globals (constants + functions)");
    println!("  Result<T, String>   — safe_divide throws on error");
    println!("  StackNewtype::new() — Rust-side construction");
    println!("  Forwarded getters   — .x(), .y(), .radius(), .color()");
    println!("  Forwarded setter    — .set_color()");
    println!("  Forwarded methods   — .length(), .scale()");
    println!("  upcast()            — Circle -> Shape");
    println!("  cast::<T>()         — Shape -> Circle (checked downcast)");
    println!("  External JS file    — showcase.js exercises all APIs");
}

#[test]
fn showcase_example() {
    main()
}
