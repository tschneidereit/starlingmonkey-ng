// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! A safe, idiomatic Rust API for SpiderMonkey.
//!
//! This module provides a coherent, well-structured API that wraps SpiderMonkey's
//! JSAPI surface. It is designed to be safe wherever the type system can enforce
//! the required invariants, ergonomic to use, and complete enough to cover the
//! full SpiderMonkey embedding surface.
//!
//! # Safety Model
//!
//! A function in this API is marked safe if **all** of the following hold:
//!
//! 1. All GC-heap parameters are properly rooted (enforced via [`Handle`] /
//!    [`MutableHandle`] types).
//! 2. A realm has been entered when required (enforced via the
//!    [`Scope`](crate::gc::scope::Scope) type).
//! 3. There is no other invariant that the type system cannot check.
//!
//! Notably, **a function that may trigger garbage collection is still safe** as
//! long as all its parameters are properly rooted. GC-triggering is modeled
//! through `&mut JSContext` (which invalidates any outstanding `&NoGC` borrows),
//! not through `unsafe`.
//!
//! # Module Organization
//!
//! The API is organized into flat, domain-specific modules:
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`error`] | Error types and throw helpers |
//! | [`value`] | JS value creation and inspection |
//! | [`object`] | Object creation, properties, prototypes |
//! | [`string`] | String creation, encoding, comparison |
//! | [`function`] | Function creation, calling, safe callbacks |
//! | [`array`] | Array creation and element access |
//! | [`promise`] | Promise creation, resolution, reactions |
//! | [`symbol`] | Symbol creation and well-known symbols |
//! | [`bigint`] | BigInt creation and conversion |
//! | [`typedarray`] | Typed array creation and access |
//! | [`collections`] | Map, Set, and WeakMap operations |
//! | [`comparison`] | Value comparison (===, ==, Object.is) |
//! | [`conversion`] | ECMAScript abstract type coercion and introspection |
//! | [`json`] | JSON parse and stringify |
//! | [`regexp`] | RegExp creation and execution |
//! | [`date`] | Date object creation and queries |
//! | [`compile`] | Script compilation and evaluation |
//! | [`module`] | ES module compilation, linking, evaluation |
//! | [`realm`] | Realm context trait, utilities, and built-in prototypes |
//! | [`gc`] | Garbage collection control |
//! | [`exception`] | Pending exception management |
//! | [`class`] | JSClass definition and standard classes |
//! | [`stack`] | Stack capture and saved frame inspection |
//! | [`context`] | Context options, callbacks, memory |
//! | [`compartment`] | Cross-compartment wrappers |
//! | [`id`] | Property key (jsid) operations |
//! | [`jobs`] | Job queue (microtask) management |
//! | [`structured_clone`] | Structured clone read/write |
//! | [`try_catch`] | Scoped exception handler |
//! | [`debug`] | Debugger, profiling, testing |
//! | [`ionmonkey`] | JIT compiler options |
//! | [`builtins`] | Newtype wrappers and Is/As traits for built-in types |
//! | [`prelude`] | Convenient re-exports |
//!
//! # Quick Start
//!
//! ```ignore
//! use mozjs::rust::{JSEngine, Runtime};
//! use core_runtime::js::prelude::*;
//!
//! let engine = JSEngine::init().unwrap();
//! let runtime = Runtime::new(engine.handle());
//! // ... enter a realm, evaluate scripts, etc.
//! ```
//!
//! # Unsafe Blocks
//!
//! Most functions in this module wrap SpiderMonkey C++ functions via the
//! `wrappers2` FFI layer. The `unsafe` blocks in these wrappers share a
//! common safety justification:
//!
//! - The [`Scope`](crate::gc::scope::Scope) parameter ensures a valid
//!   `JSContext` with an entered realm.
//! - [`Handle`] / [`MutableHandle`] types ensure GC-heap values are rooted.
//! - The `wrappers2` functions are mechanically generated wrappers around
//!   `JSAPI` functions that match the original C++ signatures.
//!
//! Non-trivial unsafe blocks (pointer arithmetic, transmutes, raw pointer
//! dereferences) are documented with per-block `// SAFETY:` comments.

pub mod array;
pub mod bigint;
pub mod builtins;
pub mod class;
pub mod collections;
pub mod comparison;
pub mod compartment;
pub mod compile;
pub mod context;
// pub mod conversion;
pub mod date;
pub mod debug;
pub mod error;
pub mod exception;
pub mod function;
pub mod gc;
pub mod id;
pub mod ionmonkey;
pub mod jobs;
pub mod json;
pub mod macros {
    pub use starling_macro::*;
}
pub mod module;
pub mod object;
pub mod prelude;
pub mod promise;
pub mod regexp;
pub mod stack;
pub mod string;
pub mod structured_clone;
pub mod symbol;
pub mod try_catch;
pub mod typedarray;
pub mod value;

// ---------------------------------------------------------------------------
// Re-exports of SpiderMonkey types that have no safe wrapper
// ---------------------------------------------------------------------------
//
// These are types from the `mozjs` crate that are fundamental to the
// SpiderMonkey embedding API and have no meaningful safe abstraction.
// They are re-exported here so that downstream crates can depend solely
// on `js` without a direct `mozjs` dependency.
//
// Organized by domain rather than mirroring `mozjs`'s module structure.

/// SpiderMonkey engine and runtime lifecycle types.
///
/// These are needed to initialize SpiderMonkey and create execution contexts.
pub mod engine {
    pub use mozjs::rust::{JSEngine, JSEngineHandle, RealmOptions, Runtime as MozJSRuntime};
}

/// Context and native callback types.
///
/// Used by code that implements `JSNative` callbacks, GC trace hooks,
/// or class finalize hooks.
pub mod native {
    pub use mozjs::context::{JSContext, RawJSContext};
    pub use mozjs::jsapi::{
        CallArgs, ExceptionStackBehavior, GCContext, HandleValueArray, JSNative, JSObject,
        JSRuntime, JSString, JSTracer, PropertyDescriptor, SymbolCode, Value,
    };
    pub use mozjs::rust::wrappers2::JS_GetRuntime;
    pub use mozjs::rust::{Handle, HandleId, HandleObject, MutableHandle, MutableHandleValue};

    /// Re-export of `mozjs::gc::Handle` for use in trait signatures that need
    /// the lifetime-parameterized version.
    pub use mozjs::gc::Handle as GCHandle;

    /// The raw `JS::Handle<T>` type from SpiderMonkey's FFI layer.
    ///
    /// Used in resolve hook signatures and other callbacks that receive
    /// handles directly from the engine.
    pub use mozjs::jsapi::Handle as RawHandle;
}

/// Raw class definition types and constants from SpiderMonkey.
///
/// These are primarily used by generated proc macro code (`#[jsclass]`,
/// `#[jsmethods]`, etc.) and should rarely be referenced directly.
/// Prefer the higher-level abstractions in [`class`] for hand-written code.
pub mod class_spec {
    pub use mozjs::jsapi::{
        JSClass, JSClassOps, JSFunctionSpec, JSNativeWrapper, JSPropertySpec,
        JSPropertySpec_Accessor, JSPropertySpec_AccessorsOrValue,
        JSPropertySpec_AccessorsOrValue_Accessors, JSPropertySpec_Kind, JSPropertySpec_Name,
        JSProtoKey, JS_EnumerateStandardClasses, JS_GlobalObjectTraceHook,
        JS_MayResolveStandardClass, JS_NewObjectForConstructor, JS_ResolveStandardClass,
        JSCLASS_FOREGROUND_FINALIZE, JSCLASS_IS_GLOBAL, JSCLASS_RESERVED_SLOTS_SHIFT,
        JSPROP_ENUMERATE, JSPROP_PERMANENT, JSPROP_READONLY,
    };
    pub use mozjs::{JSCLASS_GLOBAL_SLOT_COUNT, JSCLASS_RESERVED_SLOTS_MASK};
}

/// GC heap storage types and tracing.
///
/// `Heap<T>` stores GC pointers outside the stack. `RootedTraceableBox<T>`
/// provides self-rooting heap storage. Both are fundamental to storing
/// JS objects in Rust data structures that outlive a single scope.
pub mod heap {
    pub use mozjs::gc::RootedTraceableBox;
    pub use mozjs::jsapi::Heap;
    pub use mozjs::rust::Trace;
}

/// Value conversion traits from `mozjs::conversions`.
///
/// These traits enable automatic conversion between Rust types and
/// JS values in generated `JSNative` callback code.
pub mod conversions {
    pub use mozjs::conversions::{
        jsstr_to_string, ConversionBehavior, ConversionResult, FromJSValConvertible,
        ToJSValConvertible,
    };
}

/// Module compilation and source text utilities.
///
/// Re-exports of low-level module compilation functions that are not
/// (yet) wrapped by the higher-level `js::module` API.
pub mod module_raw {
    pub use mozjs::jsapi::{
        CompileModule1, GetModuleRequestSpecifier, JSRuntime, SetModulePrivate,
    };
    pub use mozjs::rust::{
        transform_str_to_source_text, wrappers2::GetModuleEnvironment, CompileOptionsWrapper,
    };
}

/// The `rooted!` macro for stack-allocating GC roots.
///
/// This is sometimes necessary in low-level code that interacts directly
/// with SpiderMonkey. Prefer `Scope::root_*` methods when possible.
pub use mozjs::rooted;

pub use macros::{allow_unrooted, allow_unrooted_interior, must_root};

use crate::gc::handle::Stack;

// ---------------------------------------------------------------------------
// Type aliases — scope-rooted handles for builtin JS types
// ---------------------------------------------------------------------------

/// A scope-rooted handle to a JavaScript object.
///
/// This is the primary type for interacting with JS objects.
/// All builtin handle types (`Array<'s>`, `Promise<'s>`, etc.) deref to this.
pub type Object<'s> = Stack<'s, object::Object>;

/// A scope-rooted handle to a JavaScript `Array` object.
pub type Array<'s> = Stack<'s, array::Array>;

/// A scope-rooted handle to a JavaScript `Promise` object.
pub type Promise<'s> = Stack<'s, promise::Promise>;

/// A scope-rooted handle to a JavaScript `Date` object.
pub type Date<'s> = Stack<'s, date::Date>;

/// A scope-rooted handle to a JavaScript `RegExp` object.
pub type RegExp<'s> = Stack<'s, regexp::RegExp>;

/// A scope-rooted handle to a JavaScript `Map` object.
pub type Map<'s> = Stack<'s, collections::map::Map>;

/// A scope-rooted handle to a JavaScript `Set` object.
pub type Set<'s> = Stack<'s, collections::set::Set>;

/// A scope-rooted handle to a JavaScript `WeakMap` object.
pub type WeakMap<'s> = Stack<'s, collections::weak_map::WeakMap>;
