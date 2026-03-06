# StarlingMonkey

A JavaScript runtime for WASI and native platforms, built on
[SpiderMonkey](https://spidermonkey.dev/).

StarlingMonkey is designed to be extensible and provides safe high-level
abstractions for defining additional builtins as JS classes, WebIDL interfaces,
JS modules, and functions and properties on the global object.

---

## Contents

- [Running JavaScript](#running-javascript)
- [Built-in APIs](#built-in-apis)
- [CLI Reference](#cli-reference)
- [Extending with Custom Builtins](#extending-with-custom-builtins)
  - [`#[jsclass]` / `#[jsmethods]`](#jsclass--jsmethods)
  - [`#[jsmodule]`](#jsmodule)
  - [`#[jsglobals]`](#jsglobals)
  - [`#[jsnamespace]` / `#[webidl_namespace]`](#jsnamespace--webidl_namespace)
  - [`#[webidl_interface]`](#webidl_interface)
  - [`#[derive(Traceable)]`](#derivetraceable)
- [Error Handling](#error-handling)
- [Inheritance](#inheritance)
- [Promise / Async](#promise--async)
- [Building](#building)
- [Running Tests](#running-tests)
- [Web Platform Tests (WPT)](#web-platform-tests-wpt)
- [GC Rooting Linter (Crown)](#gc-rooting-linter-crown)
- [Architecture](#architecture)

---

## Running JavaScript

StarlingMonkey runs `.js` and `.mjs` files as ES modules by default:

```bash
starling script.js
```

ES module features work out of the box — `import`/`export`, strict mode, and
multi-file projects:

```bash
# main.js
import { greet } from "./greet.js";
console.log(greet("world"));
```

```bash
starling main.js
```

For quick one-liners, use `-e`:

```bash
starling -e 'console.log("hello")'
```

For legacy scripts that rely on sloppy mode or a global `this`:

```bash
starling --legacy-script old-code.js
```

---

## Built-in APIs

StarlingMonkey's suite of builtins is currently in its infancy, only providing
a small set of builtins that were written to test out the API:
spec-compliant Rust implementations of `DOMException` and `atob`/`btoa` exist, as well as basic versions `setTimeout`/`clearTimeout`,
`setInterval`/ `clearInterval`.

### C++ built-ins

The previous incarnation of StarlingMonkey was written in C++. This one has
support for running built-ins from that version, in the [crates/builtins/cpp-builtins]()
crate. Only the old `console` builtin is added right now.

Builtins are tested against the
[Web Platform Tests](https://github.com/web-platform-tests/wpt) suite running
on both native and `wasm32-wasip2` targets.

---

## CLI Reference

```
starling [OPTIONS] [PATH]

Arguments:
  [PATH]   Path to the entry JS/MJS file (default: ./index.js)

Options:
  -e, --eval <SCRIPT>                Evaluate inline script instead of a file
  -i, --initializer-script <PATH>    Run an init script in a separate global first
      --legacy-script                Run as a classic script instead of an ES module
  -v, --verbose                      Enable verbose logging
  -d, --debug                        Enable script debugging via socket
      --wpt-mode                     Enable WPT (Web Platform Tests) mode
      --init-location <URL>          Override the location URL for initialization
      --strip-path-prefix <PREFIX>   Strip this prefix from script paths
  -h, --help                         Print help
```

**Module mode** (default) — strict mode, `import`/`export` supported, `this`
is `undefined` at the top level.

**Legacy script mode** (`--legacy-script`) — sloppy mode, no
`import`/`export`, `this` is the global object.

---

## Extending with Custom Builtins

StarlingMonkey provides proc macros for exposing Rust code to JavaScript. All
builtins in the `web-globals` crate are implemented using these macros.

### `#[jsclass]` / `#[jsmethods]`

Expose a Rust struct as a JS constructor with methods, getters, setters, and
static methods:

```rust
use libstarling::{jsclass, jsmethods};

#[jsclass]
struct Counter {
    value: i32,
}

#[jsmethods]
impl Counter {
    #[constructor]
    fn new(initial: i32) -> Self { Self { value: initial } }

    #[method]
    fn increment(&mut self) { self.value += 1; }

    #[getter]
    fn value(&self) -> i32 { self.value }

    #[static_method]
    fn zero() -> Self { Self { value: 0 } }
}

// Register on the JS global and create an instance from Rust:
Counter::add_to_global(&scope, global);
let c = Counter::new(&scope, 0);   // Counter<'s> stack newtype
```

The `#[jsclass]` macro generates three types from the annotated struct, allowing
the type to be used from JS and Rust while ensuring proper GC rooting:

| Generated type | Purpose |
|----------------|---------|
| `__CounterInner` | Inner data struct implementing `ClassDef`. |
| `Counter<'s>` | Stack newtype — use within a GC scope. |
| `CounterRef` | Heap ref — store inside `#[derive(Traceable)]` structs. |

**`#[jsclass]` options:**

```rust
#[jsclass(name = "MyCounter")]         // override the JS class name
#[jsclass(extends = Parent)]           // set up a prototype chain
#[jsclass(js_proto = "Error")]         // inherit from a built-in JS prototype
#[jsclass(to_string_tag = "MyClass")]  // set Symbol.toStringTag
```

**`#[jsmethods]` attributes:**

| Attribute | Role |
|-----------|------|
| `#[constructor]` | Called when JS code runs `new Counter(...)`. |
| `#[method]` / `#[method(name = "jsName")]` | Instance method on the prototype. |
| `#[getter]` | Read-only JS property accessor (`obj.x`). |
| `#[setter]` | Write accessor; paired with matching getter by name. |
| `#[property]` | Convenience: generates both getter and looks for a `set_<name>` setter. |
| `#[static_method]` | Method on the constructor (`Counter.zero()`). |
| `#[destructor]` | Called by SpiderMonkey just before the object is freed. |

**Return types:**

| Rust return type | JS behaviour |
|------------------|-------------|
| `()` | `undefined` |
| `T: ToJSValConvertible` | Value returned to JS. |
| `Result<T, E>` where `E: ThrowException` | `Ok` → value; `Err` → typed JS exception. |
| `Self` (from `#[static_method]` / `#[method]`) | New JS instance of the same class. |
| `JSPromise` | JS `Promise`; the future is driven by the event loop. |

**Constants:**

`pub const` items in `#[jsmethods]` blocks become read-only properties on
the constructor:

```rust
#[jsmethods]
impl DOMException {
    pub const INDEX_SIZE_ERR: u16 = 1;
    pub const NOT_FOUND_ERR: u16 = 8;
    // ...
}
```

**Variadic arguments:**

Use `RestArgs<T>` as the last parameter to collect extra typed arguments:

```rust
#[static_method]
fn sum(a: f64, rest: RestArgs<f64>) -> f64 {
    a + rest.iter().sum::<f64>()
}
```

There's also support for untyped access to arguments via `&CallArgs`.

### `#[jsmodule]`

Turn a Rust `mod` block into an importable ES module:

```rust
#[jsmodule]
mod math {
    pub const PI: f64 = std::f64::consts::PI;

    pub fn add(a: f64, b: f64) -> f64 { a + b }

    pub fn safe_divide(a: f64, b: f64) -> Result<f64, String> {
        if b == 0.0 { Err("division by zero".into()) } else { Ok(a / b) }
    }
}

// Register before evaluating any JS that imports it:
unsafe { math::register(&scope); }
```

```js
import { PI, add, safeDivide } from "math";
```

Override the import specifier: `#[jsmodule(name = "my-math")]`

### `#[jsglobals]`

Install functions, constants, and class constructors directly on the global
object:

```rust
#[jsglobals]
mod app_globals {
    pub use super::Circle;   // `pub use` items register #[jsclass] classes;
    pub use super::Shape;    // any order works — parents are auto-registered first
    pub const APP_NAME: &str = "My App";

    pub fn greet(name: String) -> String {
        format!("Hello, {name}!")
    }
}

// Install on a global object:
unsafe { app_globals::add_to_global(&scope, global); }
```

### `#[jsnamespace]` / `#[webidl_namespace]`

Create a plain singleton object (like `console`):

```rust
#[jsnamespace(name = "console")]
mod console_ns {
    pub fn log(scope: &Scope<'_>, rest: RestArgs<Value>) { /* ... */ }
    pub fn warn(scope: &Scope<'_>, rest: RestArgs<Value>) { /* ... */ }
}

unsafe { console_ns::add_to_global(&scope, global); }
```

`#[webidl_namespace]` is the same but auto-sets `Symbol.toStringTag` per
[WebIDL §3.13](https://webidl.spec.whatwg.org/#es-namespaces).

### `#[webidl_interface]`

Like `#[jsclass]` but with [WebIDL §3.7](https://webidl.spec.whatwg.org/#es-interfaces) semantics:
- `Symbol.toStringTag` auto-set to the class name (overridable with `to_string_tag`)
- `pub const` items installed on **both** constructor and prototype

```rust
#[webidl_interface(js_proto = "Error")]
struct DOMException {
    name: String,
    message: String,
}
```

Same options as `#[jsclass]`: `name`, `extends`, `js_proto`, `to_string_tag`.

### `#[derive(Traceable)]`

Generate `unsafe impl Trace` so SpiderMonkey's GC can find JS references
stored in your Rust structs:

```rust
#[derive(Traceable)]
struct AppState {
    node: MyClassRef,           // traced automatically
    #[no_trace]
    counter: u32,               // excluded from tracing
}
```

Use `MyClassRef` (the heap-ref type from `#[jsclass]`) whenever storing a JS
object reference in a long-lived struct.

---

## Error Handling

Methods returning `Result<T, E>` where `E: ThrowException` throw typed JS
exceptions on `Err`:

```rust
use core_runtime::class::{TypeError, RangeError, SyntaxError};

#[jsmethods]
impl MyClass {
    #[method]
    fn parse(&self, input: String) -> Result<String, SyntaxError> {
        if input.is_empty() {
            return Err(SyntaxError("input must not be empty".into()));
        }
        Ok(input)
    }
}
```

**Built-in error types:**

| Type | JS Exception |
|------|-------------|
| `TypeError(String)` | `TypeError` |
| `RangeError(String)` | `RangeError` |
| `SyntaxError(String)` | `SyntaxError` |
| `String` | automatically converted to `TypeError` |
| `DOMExceptionError { name, message }` | `DOMException` |

Implement `ThrowException` for custom error types:

```rust
use core_runtime::class::ThrowException;
use js::gc::scope::Scope;

struct MyError(String);

impl ThrowException for MyError {
    unsafe fn throw(self, scope: &Scope<'_>) {
        TypeError(self.0).throw(scope);
    }
}
```

---

## Inheritance

```rust
#[jsclass]
struct Shape { color: String }

#[jsclass(extends = Shape)]
struct Circle {
    parent: __ShapeInner,      // first field must be the parent inner type
    radius: f64,
}

#[jsmethods]
impl Circle {
    #[constructor]
    fn new(color: String, radius: f64) -> Self {
        Self { parent: __ShapeInner::new(color), radius }
    }

    #[method]
    fn area(&self) -> f64 { std::f64::consts::PI * self.radius * self.radius }
}
```

Upcast and downcast from Rust:

```rust
let circle: Circle<'s> = /* ... */;
let shape: Shape<'s> = circle.upcast();             // always succeeds
let back: Option<Circle<'s>> = shape.cast::<Circle<'_>>();  // type-checked
```

---

## Promise / Async

Return `JSPromise` from any method to create a JS `Promise`:

```rust
use libstarling::class::JSPromise;

#[jsmethods]
impl Fetcher {
    #[method]
    fn fetch(&self, url: String) -> JSPromise {
        JSPromise::new(async move {
            // ... async work ...
            Ok("response body".to_string())
        })
    }
}
```

The method returns the `Promise` to JS immediately. The runtime drives all
pending futures to completion via `drain_promises(&scope)`.

---

## Building

**Prerequisites:**

- Rust toolchain (edition 2021)

Note: Builds sometimes have to compile SpiderMonkey from source, which can take
several minutes.

```bash
cargo build
```

**For WebAssembly (WASIp2):**

```bash
WASI_SDK_PATH=/opt/wasi-sdk-25 cargo build --target wasm32-wasip2
```

---

## Running Tests

```bash
cargo test --workspace

# With SpiderMonkey debug assertions (recommended — catches GC issues):
cargo test --features debugmozjs --workspace
```

A [`justfile`](justfile) provides shortcuts (requires
[just](https://github.com/casey/just)):

```bash
just test              # all Rust tests
just test-debug        # with debugmozjs assertions
just build             # debug build
just fmt               # format code
just clippy            # run clippy
just check             # fmt-check + clippy + tests
```

To stress-test under all GC zeal modes:

```bash
bash scripts/test-gc-zeal.sh        # quick mode
bash scripts/test-gc-zeal.sh full   # exhaustive
```

---

## Web Platform Tests (WPT)

The project includes a [WPT](https://web-platform-tests.org/) harness that
validates web API conformance against the official test suite.

**Current coverage:**
- `DOMException`: 115/115 tests passing
- `btoa`/`atob`: 285/286 tests passing

**Setup:**

```bash
just wpt-setup
```

**Run:**

```bash
just wpt-test              # all configured WPT tests
just wpt-test base64       # only base64 tests
just wpt-test DOMException # only DOMException tests
just wpt-update            # run and update expectation files
```

Test results are compared against expectation files in
`tests/wpt-harness/expectations/`. When adding new web APIs, add corresponding
WPT test paths to `tests/wpt-harness/tests.json` and run `just wpt-update`.

---

## GC Rooting Linter (Crown)

The workspace includes a [modified version](crown/) of Servo's
[Crown](https://github.com/servo/servo/tree/main/support/crown) `rustc` plugin
that statically verifies GC rooting safety — catching cases where raw GC
pointers are used without proper rooting.

```bash
bash scripts/check-crown.sh                  # default package
bash scripts/check-crown.sh -p core-runtime  # specific crate
bash scripts/check-crown.sh --workspace      # entire workspace
```

The `crown` script enables the `js/crown` Cargo feature, activating

`#[js::must_root]` and `#[js::allow_unrooted_interior]` annotations. Two lints
are enforced:

- **`js::must_root`** — values of types marked `#[js::must_root]` (e.g.
  `Heap<T>`) must be stored inside `Trace`-implementing or `must_root` types.
- **`crown::trace_in_no_trace`** — `#[no_trace]` fields must not contain
  traceable types.

Types generated by `#[jsclass]`, `#[jsmodule]`, etc. are annotated
automatically. For manual types, use `#[js::must_root]` or
`#[js::allow_unrooted_interior]` as appropriate.

---

## Architecture

```
starling/               # Binary crate — `starling` executable
  src/main.rs
crates/
  libstarling/          # Public-facing re-export crate
  core-runtime/         # Core: class system, module loader, event loop
    src/
      lib.rs            # run() entry point
      runtime.rs        # Engine singleton, Runtime struct, GC lifecycle
      class.rs          # ClassDef trait, HeapRef, StackNewtype, ClassRegistry
      module.rs         # NativeModule trait, file resolver, resolve hook
      config.rs         # RuntimeConfig (clap CLI parsing)
      event_loop.rs     # Task-based event loop (timers, promises)
  builtins/
    web-globals/        # btoa, atob, console, DOMException
  starling-macro/       # Proc macros
  js/                   # Safe SpiderMonkey wrapper (sole mozjs dependency)
crown/                  # GC rooting linter (rustc plugin)
scripts/                # check-crown.sh, test-gc-zeal.sh, clone-wpt.sh
tests/wpt-harness/      # WPT runner and expectation files
```

### Key Design Points

**GC-safe value ownership**

StarlingMonkey provides safe abstractions that ensure GC references are
properly rooted on both the stack and the heap:

- *Stack* — `Foo<'s>` (a `StackNewtype`) with lifetime tied to the GC scope.
- *Heap* — `FooRef` (a `HeapRef<__FooInner>`) inside a `Trace`-implementing struct.

These make proper rooting the default and much easier to get right. The
GC rooting linter will additionally catch almost all violations of GC rooting.

**Safe, high-level JS API**

Built on top of `mozjs`, the `js` crate provides a higher-level API designed to
make use of SpiderMonkey easier and safer.

While there are some cases where lower-level constructs leak through for now,
the goal is to eventually make `js` a full abstraction layer.

**Proc-macro code generation**

As described above, StarlingMonkey has an extensive suite of proc macros to
make implementation and use of additional builtins as easy and safe as
possible.
