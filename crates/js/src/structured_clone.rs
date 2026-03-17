// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Structured clone read and write.
//!
//! Structured cloning serializes and deserializes JavaScript values in a
//! format that preserves object graphs, typed arrays, and other complex types.
//! This is the mechanism behind `postMessage` and `IndexedDB` serialization.

use crate::gc::scope::Scope;
use mozjs::jsapi::{
    CloneDataPolicy, JSStructuredCloneCallbacks, JSStructuredCloneData, StructuredCloneScope,
};
use mozjs::jsval::UndefinedValue;
use mozjs::rust::wrappers2;
use mozjs::rust::HandleValue;

use super::error::ExnThrown;

/// Perform a structured clone of a value (serialize + deserialize in one step).
///
/// # Safety
///
/// `callbacks` must be a valid pointer (or null). `closure` is passed through.
pub unsafe fn clone<'r>(
    scope: &'r Scope<'_>,
    value: HandleValue,
    callbacks: *const JSStructuredCloneCallbacks,
    closure: *mut std::os::raw::c_void,
) -> Result<HandleValue<'r>, ExnThrown> {
    let mut rval = scope.root_value_mut(UndefinedValue());
    let ok =
        wrappers2::JS_StructuredClone(scope.cx_mut(), value, rval.reborrow(), callbacks, closure);
    ExnThrown::check(ok)?;
    Ok(rval.handle())
}

/// Write a value into structured clone data.
///
/// # Safety
///
/// `data`, `policy`, `callbacks` must be valid pointers (or null for callbacks).
/// `closure` is passed through to callbacks.
pub unsafe fn write(
    scope: &Scope<'_>,
    value: HandleValue,
    data: *mut JSStructuredCloneData,
    clone_scope: StructuredCloneScope,
    policy: *const CloneDataPolicy,
    callbacks: *const JSStructuredCloneCallbacks,
    closure: *mut std::os::raw::c_void,
    transferable: HandleValue,
) -> Result<(), ExnThrown> {
    let ok = wrappers2::JS_WriteStructuredClone(
        scope.cx_mut(),
        value,
        data,
        clone_scope,
        policy,
        callbacks,
        closure,
        transferable,
    );
    ExnThrown::check(ok)
}

/// Read a value from structured clone data.
///
/// # Safety
///
/// `data` and `policy` must be valid pointers. `callbacks` may be null.
/// `closure` is passed through to callbacks.
pub unsafe fn read<'r>(
    scope: &'r Scope<'_>,
    data: *const JSStructuredCloneData,
    version: u32,
    clone_scope: StructuredCloneScope,
    policy: *const CloneDataPolicy,
    callbacks: *const JSStructuredCloneCallbacks,
    closure: *mut std::os::raw::c_void,
) -> Result<HandleValue<'r>, ExnThrown> {
    let mut rval = scope.root_value_mut(UndefinedValue());
    let ok = wrappers2::JS_ReadStructuredClone(
        scope.cx_mut(),
        data,
        version,
        clone_scope,
        rval.reborrow(),
        policy,
        callbacks,
        closure,
    );
    ExnThrown::check(ok)?;
    Ok(rval.handle())
}
