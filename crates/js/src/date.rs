// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Date object creation and queries.
//!
//! The [`Date`] newtype wraps a scope-rooted `Handle<'s, *mut JSObject>`
//! known to be a Date object. It provides methods for creating Date objects
//! and extracting their values.

use std::ptr::NonNull;

use crate::gc::scope::Scope;
use mozjs::gc::Handle;
use mozjs::jsapi::{ClippedTime, JSObject};
use mozjs::rust::wrappers2;
use mozjs::rust::HandleObject;

use super::builtins::{Is, To};
use super::error::JSError;
use super::object::Object;

/// A JavaScript `Date` object, rooted in a scope's pool.
///
/// `Date<'s>` wraps a `Handle<'s, *mut JSObject>` known to be a `Date`.
/// Construction automatically roots in the scope:
///
/// ```ignore
/// let date = Date::new(&scope, time)?;
/// let msec = date.msec_since_epoch(&scope)?;
/// ```
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
pub struct Date<'s>(pub(crate) Handle<'s, *mut JSObject>);

impl<'s> Date<'s> {
    /// Create a new `Date` object from a `ClippedTime` value.
    pub fn new(scope: &'s Scope<'_>, time: ClippedTime) -> Result<Self, JSError> {
        let obj = unsafe { wrappers2::NewDateObject(scope.cx_mut(), time) };
        NonNull::new(obj)
            .map(|nn| Date(scope.root_object(nn)))
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
            .map(|nn| Date(scope.root_object(nn)))
            .ok_or(JSError)
    }

    /// Get the rooted handle to the underlying `JSObject`.
    pub fn handle(&self) -> HandleObject<'s> {
        self.0
    }

    /// Get a raw `NonNull` pointer to the underlying `JSObject`.
    pub fn as_non_null(self) -> Option<NonNull<JSObject>> {
        NonNull::new(self.0.get())
    }

    /// Get the raw `*mut JSObject` pointer.
    pub fn as_raw(self) -> *mut JSObject {
        self.0.get()
    }

    /// Wrap an existing rooted handle in a `Date`.
    pub fn from_handle(handle: Handle<'s, *mut JSObject>) -> Self {
        Date(handle)
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
        // SAFETY: cx is valid; self.0 is a rooted handle to a valid Date object.
        let ok = unsafe { wrappers2::DateIsValid(scope.cx_mut(), self.0, &mut valid) };
        JSError::check(ok)?;
        Ok(valid)
    }

    /// Get the milliseconds since the Unix epoch for this `Date` object.
    ///
    /// Returns `NaN` if the date is invalid.
    pub fn msec_since_epoch(&self, scope: &Scope<'_>) -> Result<f64, JSError> {
        let mut msec: f64 = 0.0;
        let ok = unsafe { wrappers2::DateGetMsecSinceEpoch(scope.cx_mut(), self.0, &mut msec) };
        JSError::check(ok)?;
        Ok(msec)
    }
}

impl Is for Date<'_> {
    fn is(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, JSError> {
        Date::is_date(scope, obj)
    }
}

impl<'s> To<Date<'s>> for Object<'s> {
    fn to(&self, scope: &Scope<'_>) -> Result<Date<'s>, JSError> {
        if Date::is(scope, self.0)? {
            Ok(Date(self.0))
        } else {
            Err(JSError)
        }
    }
}

impl<'s> std::ops::Deref for Date<'s> {
    type Target = Object<'s>;

    fn deref(&self) -> &Object<'s> {
        // SAFETY: Date and Object are both repr(transparent) over
        // Handle<'s, *mut JSObject>.
        unsafe { &*(self as *const Date<'s> as *const Object<'s>) }
    }
}
