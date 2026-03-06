// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Value comparison operations.
//!
//! Provides safe wrappers for JavaScript's three comparison algorithms:
//!
//! - **Strict equality** (`===`): [`strictly_equal`]
//! - **Loose equality** (`==`): [`loosely_equal`]
//! - **SameValue** (`Object.is`): [`same_value`]

use crate::gc::scope::Scope;
use mozjs::gc::HandleValue;
use mozjs::rust::wrappers2;

use super::error::JSError;

/// Strict equality comparison (`===`).
///
/// Returns `true` if the two values are strictly equal according to the
/// ECMAScript `===` operator.
pub fn strictly_equal(
    scope: &Scope<'_>,
    v1: HandleValue,
    v2: HandleValue,
) -> Result<bool, JSError> {
    let mut equal = false;
    let ok = unsafe { wrappers2::StrictlyEqual(scope.cx(), v1, v2, &mut equal) };
    JSError::check(ok)?;
    Ok(equal)
}

/// Loose equality comparison (`==`).
///
/// Returns `true` if the two values are loosely equal according to the
/// ECMAScript `==` operator. This may trigger type coercion.
pub fn loosely_equal(scope: &Scope<'_>, v1: HandleValue, v2: HandleValue) -> Result<bool, JSError> {
    let mut equal = false;
    let ok = unsafe { wrappers2::LooselyEqual(scope.cx_mut(), v1, v2, &mut equal) };
    JSError::check(ok)?;
    Ok(equal)
}

/// SameValue comparison (`Object.is`).
///
/// Differs from strict equality in that `NaN === NaN` is `true` and
/// `+0` is not the same as `-0`.
pub fn same_value(scope: &Scope<'_>, v1: HandleValue, v2: HandleValue) -> Result<bool, JSError> {
    let mut same = false;
    let ok = unsafe { wrappers2::SameValue(scope.cx(), v1, v2, &mut same) };
    JSError::check(ok)?;
    Ok(same)
}
