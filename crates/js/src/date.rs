// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Date object creation and queries.
//!
//! The [`Date`] marker type implements [`JsType`](crate::gc::handle::JsType),
//! enabling [`Stack<'s, Date>`](crate::gc::handle::Stack) as the scope-rooted
//! handle type. It provides methods for creating Date objects and extracting
//! their values.

use std::ptr::NonNull;

use crate::gc::handle::{JsType, Stack};
use crate::gc::scope::Scope;
use mozjs::jsapi::ClippedTime;
use mozjs::rust::wrappers2;
use mozjs::rust::HandleObject;

use super::builtins::{Is, To};
use super::error::JSError;
use crate::Object;

/// Marker type for JavaScript `Date` objects.
///
/// Use [`Stack<'s, Date>`](crate::gc::handle::Stack) as the scope-rooted
/// handle type:
///
/// ```ignore
/// let date = Stack::<Date>::new(&scope, time)?;
/// let msec = date.msec_since_epoch(&scope)?;
/// ```
pub struct Date;

impl JsType for Date {
    const JS_NAME: &'static str = "Date";
}

impl<'s> Stack<'s, Date> {
    /// Create a new `Date` object from a `ClippedTime` value.
    pub fn new(scope: &'s Scope<'_>, time: ClippedTime) -> Result<Self, JSError> {
        let obj = unsafe { wrappers2::NewDateObject(scope.cx_mut(), time) };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(JSError)
    }

    /// Create a new `Date` object from individual components.
    ///
    /// All values follow the `Date` constructor semantics (month is 0-based, etc).
    pub fn from_components(
        scope: &'s Scope<'_>,
        year: i32,
        month: i32,
        day: i32,
        hour: i32,
        minute: i32,
        second: i32,
    ) -> Result<Self, JSError> {
        let obj = unsafe {
            wrappers2::NewDateObject1(scope.cx_mut(), year, month, day, hour, minute, second)
        };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(JSError)
    }

    /// Check whether an object is a `Date`.
    pub fn is_date(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        let mut result = false;
        // SAFETY: cx and obj are valid; ObjectIsDate writes to result.
        let ok = unsafe { wrappers2::ObjectIsDate(scope.cx_mut(), obj, &mut result) };
        JSError::check(ok)?;
        Ok(result)
    }

    /// Check whether this date is valid (not NaN).
    pub fn is_valid(&self, scope: &Scope<'_>) -> Result<bool, JSError> {
        let mut valid = false;
        // SAFETY: cx is valid; self is a rooted handle to a valid Date object.
        let ok = unsafe { wrappers2::DateIsValid(scope.cx_mut(), self.handle(), &mut valid) };
        JSError::check(ok)?;
        Ok(valid)
    }

    /// Get the milliseconds since the Unix epoch for this `Date` object.
    ///
    /// Returns `NaN` if the date is invalid.
    pub fn msec_since_epoch(&self, scope: &Scope<'_>) -> Result<f64, JSError> {
        let mut msec: f64 = 0.0;
        let ok =
            unsafe { wrappers2::DateGetMsecSinceEpoch(scope.cx_mut(), self.handle(), &mut msec) };
        JSError::check(ok)?;
        Ok(msec)
    }
}

impl Is for Date {
    fn is(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        Stack::<Date>::is_date(scope, obj)
    }
}

impl<'s> To<Stack<'s, Date>> for Object<'s> {
    fn to(&self, scope: &Scope<'_>) -> Result<Stack<'s, Date>, JSError> {
        if Date::is(scope, self.handle())? {
            // SAFETY: We just verified the object is a Date.
            Ok(unsafe { Stack::from_handle_unchecked(self.handle()) })
        } else {
            Err(JSError)
        }
    }
}

impl<'s> std::ops::Deref for Stack<'s, Date> {
    type Target = Object<'s>;

    fn deref(&self) -> &Object<'s> {
        // SAFETY: Stack<Date> and Stack<Object> are both repr(transparent)
        // over Handle<'s, *mut JSObject>.
        unsafe { &*(self as *const Stack<'s, Date> as *const Object<'s>) }
    }
}
