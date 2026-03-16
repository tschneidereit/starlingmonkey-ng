// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Traits for built-in type checking and checked conversion, plus primitive
//! type markers.
//!
//! Object newtypes (Array, Map, Set, Promise, Date, RegExp, WeakMap) live in
//! their own modules. This module provides the shared [`IsPrimitive`] trait, plus
//! primitive marker types for value-level type tests.

use mozjs::{
    gc::Handle,
    jsapi::{JSObject, Value},
};

use crate::gc::handle::Stack;

/// Marker trait for types representable as JavaScript objects.
///
/// Implemented by:
/// - Builtin marker types (`object::Object`, `array::Array`, `promise::Promise`, etc.)
/// - `ClassDef` types (user-defined classes with Rust data in private slots)
///
/// `JSType` is the bound for [`Stack`] and [`Heap`], the universal wrappers
/// for scope-rooted and heap-traced JS object handles.
///
/// Every `JSType` carries a stable `JSClass` pointer that serves as the
/// type's identity tag. For builtin types this comes from SpiderMonkey's
/// `ProtoKeyToClass`; for user-defined classes from the generated
/// `static JSClass`.
pub trait JSType: 'static {
    /// The JavaScript-visible name of this type (e.g. `"Object"`, `"Array"`, `"Counter"`).
    const JS_NAME: &'static str;

    /// The `JSClass` pointer that identifies objects of this type.
    ///
    /// Used by [`Stack::is`](Stack::is) and
    /// [`Stack::from_object`](Stack::from_object) for type-checked
    /// conversions.
    fn js_class() -> *const crate::class_spec::JSClass;
}

/// Target type for [`Stack::cast`] and [`StackType::cast`].
///
/// Implemented for:
/// - All [`JSType`] markers (builtins like `Date`, `Array`, `Promise`, etc.)
///   — cast returns `Stack<'s, T>`.
/// - Proc-macro newtypes (e.g. `Dog<'s>`) — cast returns the newtype itself.
pub trait CastTarget<'s> {
    /// The result type of a successful cast.
    type Output;

    /// JS-visible name of the target type (for error messages).
    const TARGET_NAME: &'static str;

    /// The `JSClass` pointer tag identifying this type.
    fn target_class_tag() -> usize;

    /// Construct the output from a rooted handle without type checking.
    ///
    /// # Safety
    ///
    /// The handle must point to a JS object of the target type (or subclass).
    unsafe fn construct_unchecked(h: Handle<'s, *mut JSObject>) -> Self::Output;
}

/// Blanket impl: `Stack<'s, T>` is a valid cast target for any `T: JSType`,
/// including the public type aliases (`Date<'s>`, `Promise<'s>`, etc.).
impl<'s, T: JSType> CastTarget<'s> for Stack<'s, T> {
    type Output = Stack<'s, T>;

    const TARGET_NAME: &'static str = T::JS_NAME;

    fn target_class_tag() -> usize {
        class_tag::<T>()
    }

    unsafe fn construct_unchecked(h: Handle<'s, *mut JSObject>) -> Stack<'s, T> {
        unsafe { Stack::from_handle_unchecked(h) }
    }
}

/// Get the `JSClass` pointer for any `JSType`, cast to `usize`.
///
/// Works for both user-defined classes (via `ClassDef`) and builtin types
/// (Object, Array, Date, Promise, etc.).
#[inline]
pub(crate) fn class_tag<T: JSType>() -> usize {
    T::js_class() as usize
}

/// Read the type tag from a JS object by inspecting its `JSClass` pointer.
///
/// # Safety
///
/// - `obj` must be a valid, non-null JS object pointer.
pub unsafe fn get_class_tag(obj: *mut JSObject) -> usize {
    crate::object::get_object_class(obj) as usize
}

/// Check if a concrete type (by tag) derives from a target type (by tag).
///
/// Returns `true` if `concrete_tag == target_tag`, or if the concrete type
/// has the target in its ancestor set. Also returns `true` if `target_tag`
/// is Object's JSClass tag, since every JS object is-an Object.
pub(crate) fn is_derived_from_type(concrete_tag: usize, target_tag: usize) -> bool {
    if concrete_tag == target_tag {
        return true;
    }
    if target_tag == class_tag::<crate::object::Object>() {
        return true;
    }

    crate::class::inherits_from(concrete_tag, target_tag)
}

/// Error returned when a type-checked [`cast`](StackType::cast) fails.
#[derive(Debug)]
pub struct CastError {
    /// Name of the source class.
    pub from: &'static str,
    /// Name of the target class.
    pub to: &'static str,
}

impl std::fmt::Display for CastError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cannot cast {} to {}", self.from, self.to)
    }
}

impl std::error::Error for CastError {}

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
