// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Typed array creation and access.
//!
//! This module re-exports and wraps the typed array API from [`crate::typedarray`],
//! adding safe wrappers for creating typed arrays with the `Scope` constraint.
//!
//! For the core typed array types (`TypedArray`, `TypedArrayElement`,
//! `TypedArrayElementCreator`) and element type tags (`Uint8`, `Int32`,
//! `Float64`, etc.), see the re-exports below.
//!
//! # Creating typed arrays
//!
//! Use [`new_typed_array`] to create a typed array of a given element type and
//! length in the current realm.
//!
//! ```ignore
//! use crate::typedarray;
//! use mozjs::typedarray::Uint8;
//!
//! let obj = typedarray::new_typed_array::<Uint8>(&mut realm, 1024)?;
//! ```

use std::ptr::NonNull;

use crate::gc::scope::Scope;
use mozjs::gc::Handle;
use mozjs::jsapi::JSObject;
use mozjs::rust::wrappers2;
use mozjs::typedarray::TypedArrayElementCreator;

use super::error::JSError;

// Re-export the core typed array types so users can `use crate::typedarray::*`.
pub use mozjs::typedarray::{
    ClampedU8, Float32, Float64, Int16, Int32, Int8, TypedArray, TypedArrayElement as Element,
    Uint16, Uint32, Uint8,
};

/// Create a new typed array of the given element type with the specified length.
///
/// # Example
///
/// ```ignore
/// let arr = typedarray::new_typed_array::<Uint8>(scope, 256)?;
/// ```
pub fn new_typed_array<'s, T: TypedArrayElementCreator>(
    scope: &'s Scope<'_>,
    length: usize,
) -> Result<Handle<'s, *mut JSObject>, JSError> {
    let obj = unsafe { T::create_new(scope.cx_mut().raw_cx(), length) };
    NonNull::new(obj)
        .map(|p| scope.root_object(p))
        .ok_or(JSError)
}

/// Create a new typed array of the given element type pre-populated with data.
pub fn new_typed_array_with_data<'s, T: TypedArrayElementCreator>(
    scope: &'s Scope<'_>,
    data: &[T::Element],
) -> Result<Handle<'s, *mut JSObject>, JSError> {
    use std::ptr;
    let obj = unsafe { T::create_new(scope.cx_mut().raw_cx(), data.len()) };
    let nn = NonNull::new(obj).ok_or(JSError)?;
    // Copy data into the newly created typed array buffer.
    unsafe {
        let (buf, _len) = T::length_and_data(obj);
        ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len());
    }
    Ok(scope.root_object(nn))
}

/// Create a new `ArrayBuffer` with the given byte length.
pub fn new_array_buffer<'s>(
    scope: &'s Scope<'_>,
    nbytes: usize,
) -> Result<Handle<'s, *mut JSObject>, JSError> {
    let obj = unsafe { wrappers2::NewArrayBuffer(scope.cx_mut(), nbytes) };
    NonNull::new(obj)
        .map(|p| scope.root_object(p))
        .ok_or(JSError)
}

/// Copy an `ArrayBuffer`.
pub fn copy_array_buffer<'s>(
    scope: &'s Scope<'_>,
    buffer: mozjs::gc::HandleObject,
) -> Result<Handle<'s, *mut JSObject>, JSError> {
    let obj = unsafe { wrappers2::CopyArrayBuffer(scope.cx_mut(), buffer) };
    NonNull::new(obj)
        .map(|p| scope.root_object(p))
        .ok_or(JSError)
}

/// Detach an `ArrayBuffer`, making it zero-length.
pub fn detach_array_buffer(
    scope: &Scope<'_>,
    buffer: mozjs::gc::HandleObject,
) -> Result<(), JSError> {
    let ok = unsafe { wrappers2::DetachArrayBuffer(scope.cx_mut(), buffer) };
    JSError::check(ok)
}

/// Create a new `SharedArrayBuffer` with the given byte length.
pub fn new_shared_array_buffer<'s>(
    scope: &'s Scope<'_>,
    nbytes: usize,
) -> Result<Handle<'s, *mut JSObject>, JSError> {
    let obj = unsafe { wrappers2::NewSharedArrayBuffer(scope.cx_mut(), nbytes) };
    NonNull::new(obj)
        .map(|p| scope.root_object(p))
        .ok_or(JSError)
}

/// Create a new `ArrayBuffer` whose contents are borrowed from the caller.
///
/// The returned `ArrayBuffer` references the provided `data` without copying.
/// The caller **must** ensure `data` outlives the `ArrayBuffer` and that the
/// buffer is not detached while `data` is in use.
///
/// This is useful for passing pre-existing byte slices (e.g. Wasm modules)
/// to JS without copying.
///
/// # Safety
///
/// The caller must guarantee that `data` remains valid and is not mutated
/// for the lifetime of the returned `ArrayBuffer`.
pub unsafe fn new_array_buffer_with_user_owned_contents<'s>(
    scope: &'s Scope<'_>,
    data: &[u8],
) -> Result<Handle<'s, *mut JSObject>, JSError> {
    let obj = wrappers2::NewArrayBufferWithUserOwnedContents(
        scope.cx_mut(),
        data.len(),
        data.as_ptr() as *mut std::os::raw::c_void,
    );
    NonNull::new(obj)
        .map(|p| scope.root_object(p))
        .ok_or(JSError)
}

// TODO: add the full typed array API, potentially as a generic builtin, taking the element type as a type parameter. This would include functions for getting/setting elements, getting the length, etc. Use JS_IsTypedArrayObject, JS_IsArrayBufferViewObject, and various other functions available on mozjs_sys::jsapi. DO NOT REMOVE THIS TODO WITHOUT ADDRESSING OR BY JUST CHANGING THIS COMMENT.
