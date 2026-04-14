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

use super::error::ExnThrown;

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
) -> Result<Handle<'s, *mut JSObject>, ExnThrown> {
    let obj = unsafe { T::create_new(scope.cx_mut().raw_cx(), length) };
    NonNull::new(obj)
        .map(|p| scope.root_object(p))
        .ok_or(ExnThrown)
}

/// Create a new typed array of the given element type pre-populated with data.
pub fn new_typed_array_with_data<'s, T: TypedArrayElementCreator>(
    scope: &'s Scope<'_>,
    data: &[T::Element],
) -> Result<Handle<'s, *mut JSObject>, ExnThrown> {
    use std::ptr;
    let obj = unsafe { T::create_new(scope.cx_mut().raw_cx(), data.len()) };
    let nn = NonNull::new(obj).ok_or(ExnThrown)?;
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
) -> Result<Handle<'s, *mut JSObject>, ExnThrown> {
    let obj = unsafe { wrappers2::NewArrayBuffer(scope.cx_mut(), nbytes) };
    NonNull::new(obj)
        .map(|p| scope.root_object(p))
        .ok_or(ExnThrown)
}

/// Copy an `ArrayBuffer`.
pub fn copy_array_buffer<'s>(
    scope: &'s Scope<'_>,
    buffer: mozjs::gc::HandleObject,
) -> Result<Handle<'s, *mut JSObject>, ExnThrown> {
    let obj = unsafe { wrappers2::CopyArrayBuffer(scope.cx_mut(), buffer) };
    NonNull::new(obj)
        .map(|p| scope.root_object(p))
        .ok_or(ExnThrown)
}

/// Detach an `ArrayBuffer`, making it zero-length.
pub fn detach_array_buffer(
    scope: &Scope<'_>,
    buffer: mozjs::gc::HandleObject,
) -> Result<(), ExnThrown> {
    let ok = unsafe { wrappers2::DetachArrayBuffer(scope.cx_mut(), buffer) };
    ExnThrown::check(ok)
}

/// Create a new `SharedArrayBuffer` with the given byte length.
pub fn new_shared_array_buffer<'s>(
    scope: &'s Scope<'_>,
    nbytes: usize,
) -> Result<Handle<'s, *mut JSObject>, ExnThrown> {
    let obj = unsafe { wrappers2::NewSharedArrayBuffer(scope.cx_mut(), nbytes) };
    NonNull::new(obj)
        .map(|p| scope.root_object(p))
        .ok_or(ExnThrown)
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
) -> Result<Handle<'s, *mut JSObject>, ExnThrown> {
    let obj = wrappers2::NewArrayBufferWithUserOwnedContents(
        scope.cx_mut(),
        data.len(),
        data.as_ptr() as *mut std::os::raw::c_void,
    );
    NonNull::new(obj)
        .map(|p| scope.root_object(p))
        .ok_or(ExnThrown)
}

/// Create a new `ArrayBuffer` with the given data copied into it.
pub fn new_array_buffer_with_data<'s>(
    scope: &'s Scope<'_>,
    data: &[u8],
) -> Result<Handle<'s, *mut JSObject>, ExnThrown> {
    let obj = unsafe { wrappers2::NewArrayBuffer(scope.cx_mut(), data.len()) };
    let nn = NonNull::new(obj).ok_or(ExnThrown)?;
    if !data.is_empty() {
        unsafe {
            let mut is_shared = false;
            let nogc = mozjs::jsapi::JS::AutoRequireNoGC { _address: 0 };
            let buf = mozjs::jsapi::JS::GetArrayBufferData(nn.as_ptr(), &mut is_shared, &nogc);
            std::ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len());
        }
    }
    Ok(scope.root_object(nn))
}

/// Copy the bytes held by any `BufferSource` (ArrayBuffer or ArrayBufferView)
/// into a new `Vec<u8>`.
///
/// Returns `None` if `obj` is neither an ArrayBuffer nor an ArrayBufferView.
///
/// # Safety
///
/// `obj` must be a valid, non-null JS object pointer. No GC may occur during
/// the lifetime of this call (the caller must ensure no JS operations happen
/// concurrently).
pub unsafe fn copy_buffer_source_bytes(obj: *mut JSObject) -> Option<Vec<u8>> {
    if mozjs::jsapi::JS::IsArrayBufferObject(obj) {
        let mut length = 0usize;
        let mut is_shared = false;
        let mut data: *mut u8 = std::ptr::null_mut();
        mozjs::jsapi::JS::GetArrayBufferLengthAndData(obj, &mut length, &mut is_shared, &mut data);
        if data.is_null() || length == 0 {
            return Some(Vec::new());
        }
        return Some(std::slice::from_raw_parts(data, length).to_vec());
    }

    if mozjs::jsapi::JS_IsArrayBufferViewObject(obj) {
        let length = mozjs::jsapi::JS_GetArrayBufferViewByteLength(obj);
        if length == 0 {
            return Some(Vec::new());
        }
        let mut is_shared = false;
        let nogc = mozjs::jsapi::JS::AutoRequireNoGC { _address: 0 };
        let data = mozjs::jsapi::JS_GetArrayBufferViewData(obj, &mut is_shared, &nogc);
        if data.is_null() {
            return Some(Vec::new());
        }
        return Some(std::slice::from_raw_parts(data as *const u8, length).to_vec());
    }

    None
}

/// Get a mutable slice into a typed array's data buffer.
///
/// Returns `Some(&mut [u8])` if `obj` is a `Uint8Array` (or any typed array
/// view whose element size is 1 byte), or `None` if it is not a typed array
/// view.
///
/// # Safety
///
/// `obj` must be a valid, non-null JS object pointer. The returned slice
/// borrows the typed array's inline data buffer — no GC or detach operations
/// may occur while the slice is live.
pub unsafe fn typed_array_data_mut(obj: *mut JSObject) -> Option<&'static mut [u8]> {
    if !mozjs::jsapi::JS_IsArrayBufferViewObject(obj) {
        return None;
    }
    let length = mozjs::jsapi::JS_GetArrayBufferViewByteLength(obj);
    if length == 0 {
        return Some(&mut []);
    }
    let mut is_shared = false;
    let nogc = mozjs::jsapi::JS::AutoRequireNoGC { _address: 0 };
    let data = mozjs::jsapi::JS_GetArrayBufferViewData(obj, &mut is_shared, &nogc);
    if data.is_null() {
        return None;
    }
    Some(std::slice::from_raw_parts_mut(data as *mut u8, length))
}

// TODO: add the full typed array API, potentially as a generic builtin, taking the element type as a type parameter. This would include functions for getting/setting elements, getting the length, etc. Use JS_IsTypedArrayObject, JS_IsArrayBufferViewObject, and various other functions available on mozjs_sys::jsapi. DO NOT REMOVE THIS TODO WITHOUT ADDRESSING OR BY JUST CHANGING THIS COMMENT.
