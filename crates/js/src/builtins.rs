// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Traits for built-in type checking and checked conversion, plus primitive
//! type markers.
//!
//! Object newtypes (Array, Map, Set, Promise, Date, RegExp, WeakMap) live in
//! their own modules. This module provides the shared [`Is`],
//! [`IsValue`], [`To`], and [`IsPrimitive`] traits, plus
//! primitive marker types for value-level type tests.
//!
//! # Type Checking and Conversion
//!
//! Use [`Is::is`] to test whether a JS object matches a type, and
//! [`To::to`] for a checked conversion:
//!
//! ```ignore
//! use crate::array::Array;
//! use crate::builtins::{Is, To};
//!
//! if Array::is(scope, obj.handle())? {
//!     let arr: &Array = obj.to(&scope)?;
//!     let len = arr.length(&scope)?;
//! }
//! ```

use crate::gc::handle::{JsType, Stack};
use crate::gc::scope::Scope;
use mozjs::gc::{HandleObject, HandleValue};
use mozjs::jsapi::Value;

use super::error::JSError;

// ---------------------------------------------------------------------------
// Type-checking trait
// ---------------------------------------------------------------------------

/// Trait for checking whether a JS object is an instance of a specific
/// built-in type.
pub trait Is {
    /// Check whether `obj` is an instance of this built-in type.
    ///
    /// Some checks (like `IsPromiseObject`) don't need `cx`; for uniformity,
    /// the trait always takes `scope`.
    fn is(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError>;
}

/// Blanket impl: `Stack<'s, T>` inherits `Is` from the inner marker `T`.
impl<T: JsType + Is> Is for Stack<'_, T> {
    fn is(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        T::is(scope, obj)
    }
}

/// Trait for checking whether a [`Value`] is an instance of a specific
/// built-in type.
pub trait IsValue {
    /// Check whether a value wraps an instance of this built-in type.
    fn is_value(scope: &Scope<'_>, val: HandleValue) -> Result<bool, JSError>;
}

// ---------------------------------------------------------------------------
// Checked conversion trait
// ---------------------------------------------------------------------------

/// Checked conversion from one built-in type to another.
///
/// Returns `T` (by value) if the underlying JS object passes [`Is::is`],
/// or [`JSError`] otherwise. Since newtypes like `Object<'s>` and `Array<'s>`
/// are `Copy` (they wrap a `Handle`), returning by value is zero-cost.
///
/// # Example
///
/// ```ignore
/// use crate::builtins::To;
/// use crate::array::Array;
///
/// let arr: Array<'_> = obj.to(&scope)?;
/// let len = arr.length(&scope)?;
/// ```
pub trait To<T> {
    /// Perform a checked conversion, returning `T` if the object is of the
    /// expected type, or [`JSError`] if not.
    fn to(&self, scope: &Scope<'_>) -> Result<T, JSError>;
}

// ---------------------------------------------------------------------------
// Primitive type markers for JSVal
// ---------------------------------------------------------------------------

/// Marker type for checking whether a `JSVal` is `undefined`.
pub struct Undefined;

/// Marker type for checking whether a `JSVal` is `null`.
pub struct Null;

/// Marker type for checking whether a `JSVal` is a boolean.
pub struct Boolean;

/// Marker type for checking whether a `JSVal` is an `int32`.
pub struct Int32;

/// Marker type for checking whether a `JSVal` is a `double`.
pub struct Double;

/// Marker type for checking whether a `JSVal` is a string.
pub struct StringPrimitive;

/// Marker type for checking whether a `JSVal` is a symbol.
pub struct SymbolPrimitive;

/// Marker type for checking whether a `JSVal` is a `BigInt`.
pub struct BigIntPrimitive;

/// Trait for checking whether a [`Value`] is of a specific primitive type.
pub trait IsPrimitive {
    /// Check whether the value is of this primitive type.
    fn is_value(val: Value) -> bool;
}

impl IsPrimitive for Undefined {
    fn is_value(val: Value) -> bool {
        val.is_undefined()
    }
}

impl IsPrimitive for Null {
    fn is_value(val: Value) -> bool {
        val.is_null()
    }
}

impl IsPrimitive for Boolean {
    fn is_value(val: Value) -> bool {
        val.is_boolean()
    }
}

impl IsPrimitive for Int32 {
    fn is_value(val: Value) -> bool {
        val.is_int32()
    }
}

impl IsPrimitive for Double {
    fn is_value(val: Value) -> bool {
        val.is_double()
    }
}

impl IsPrimitive for StringPrimitive {
    fn is_value(val: Value) -> bool {
        val.is_string()
    }
}

impl IsPrimitive for SymbolPrimitive {
    fn is_value(val: Value) -> bool {
        val.is_symbol()
    }
}

impl IsPrimitive for BigIntPrimitive {
    fn is_value(val: Value) -> bool {
        val.is_bigint()
    }
}
