// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Date object creation and queries.
//!
//! The [`Date`] marker type implements [`JSType`](crate::gc::handle::JSType),
//! enabling [`Stack<'s, Date>`](crate::gc::handle::Stack) as the scope-rooted
//! handle type. It provides methods for creating Date objects and extracting
//! their values.

use std::ptr::NonNull;

use crate::builtins::JSType;
use crate::gc::handle::Stack;
use crate::gc::scope::Scope;
use mozjs::jsapi::ClippedTime;
use mozjs::rust::wrappers2;
use mozjs::rust::HandleObject;

use super::error::ExnThrown;
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

impl JSType for Date {
    const JS_NAME: &'static str = "Date";

    fn js_class() -> *const mozjs::jsapi::JSClass {
        crate::class::proto_key_to_class(mozjs::jsapi::JSProtoKey::JSProto_Date)
    }
}

impl<'s> Stack<'s, Date> {
    /// Create a new `Date` object from a `ClippedTime` value.
    pub fn new(scope: &'s Scope<'_>, time: ClippedTime) -> Result<Self, ExnThrown> {
        let obj = unsafe { wrappers2::NewDateObject(scope.cx_mut(), time) };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(ExnThrown)
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
    ) -> Result<Self, ExnThrown> {
        let obj = unsafe {
            wrappers2::NewDateObject1(scope.cx_mut(), year, month, day, hour, minute, second)
        };
        NonNull::new(obj)
            .map(|nn| unsafe { Self::from_handle_unchecked(scope.root_object(nn)) })
            .ok_or(ExnThrown)
    }

    /// Check whether an object is a `Date`.
    pub fn is_date(scope: &Scope<'_>, obj: HandleObject) -> Result<bool, ExnThrown> {
        let mut result = false;
        // SAFETY: cx and obj are valid; ObjectIsDate writes to result.
        let ok = unsafe { wrappers2::ObjectIsDate(scope.cx_mut(), obj, &mut result) };
        ExnThrown::check(ok)?;
        Ok(result)
    }

    /// Check whether this date is valid (not NaN).
    pub fn is_valid(&self, scope: &Scope<'_>) -> Result<bool, ExnThrown> {
        let mut valid = false;
        // SAFETY: cx is valid; self is a rooted handle to a valid Date object.
        let ok = unsafe { wrappers2::DateIsValid(scope.cx_mut(), self.handle(), &mut valid) };
        ExnThrown::check(ok)?;
        Ok(valid)
    }

    /// Get the milliseconds since the Unix epoch for this `Date` object.
    ///
    /// Returns `NaN` if the date is invalid.
    pub fn msec_since_epoch(&self, scope: &Scope<'_>) -> Result<f64, ExnThrown> {
        let mut msec: f64 = 0.0;
        let ok =
            unsafe { wrappers2::DateGetMsecSinceEpoch(scope.cx_mut(), self.handle(), &mut msec) };
        ExnThrown::check(ok)?;
        Ok(msec)
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
