/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! Conversions of Rust values to and from `JSVal`, adapted from
//! mozjs.
//!
//! | IDL type                | Type                             |
//! |-------------------------|----------------------------------|
//! | any                     | `JSVal`                          |
//! | boolean                 | `bool`                           |
//! | byte                    | `i8`                             |
//! | octet                   | `u8`                             |
//! | short                   | `i16`                            |
//! | unsigned short          | `u16`                            |
//! | long                    | `i32`                            |
//! | unsigned long           | `u32`                            |
//! | long long               | `i64`                            |
//! | unsigned long long      | `u64`                            |
//! | unrestricted float      | `f32`                            |
//! | float                   | `Finite<f32>`                    |
//! | unrestricted double     | `f64`                            |
//! | double                  | `Finite<f64>`                    |
//! | USVString               | `String`                         |
//! | object                  | `*mut JSObject`                  |
//! | symbol                  | `*mut Symbol`                    |
//! | nullable types          | `Option<T>`                      |
//! | sequences               | `Vec<T>`                         |

#![deny(missing_docs)]

use mozjs::error::throw_type_error;
use mozjs::jsapi::AssertSameCompartment;
use mozjs::jsapi::JS_GetTwoByteStringCharsAndLength;
use mozjs::jsapi::JS;
use mozjs::jsapi::{ForOfIterator, ForOfIterator_NonIterableBehavior};
use mozjs::jsapi::{Heap, JS_DefineElement, JS_GetLatin1StringCharsAndLength};
use mozjs::jsapi::{JSContext, JSObject, JSString, RootedObject, RootedValue};
use mozjs::jsapi::{JS_DeprecatedStringHasLatin1Chars, JS_NewStringCopyUTF8N, JSPROP_ENUMERATE};
use mozjs::jsval::{BooleanValue, DoubleValue, Int32Value, UInt32Value, UndefinedValue};
use mozjs::jsval::{JSVal, ObjectOrNullValue, ObjectValue, StringValue, SymbolValue};
use mozjs::rooted;
use mozjs::rust::HandleValue;
use mozjs::rust::{maybe_wrap_object_or_null_value, maybe_wrap_object_value, ToString};
use mozjs::rust::{ToBoolean, ToInt32, ToInt64, ToNumber, ToUint16, ToUint32, ToUint64};
use mozjs_sys::jsgc::Rooted;
use num_traits::PrimInt;
use std::borrow::Cow;
use std::ffi::CStr;
use std::ptr::NonNull;
use std::rc::Rc;
use std::{ptr, slice};

use crate::prelude::Scope;

trait As<O>: Copy {
    fn cast(self) -> O;
}

macro_rules! impl_as {
    ($I:ty, $O:ty) => {
        impl As<$O> for $I {
            fn cast(self) -> $O {
                self as $O
            }
        }
    };
}

impl_as!(f64, u8);
impl_as!(f64, u16);
impl_as!(f64, u32);
impl_as!(f64, u64);
impl_as!(f64, i8);
impl_as!(f64, i16);
impl_as!(f64, i32);
impl_as!(f64, i64);

impl_as!(u8, f64);
impl_as!(u16, f64);
impl_as!(u32, f64);
impl_as!(u64, f64);
impl_as!(i8, f64);
impl_as!(i16, f64);
impl_as!(i32, f64);
impl_as!(i64, f64);

impl_as!(i32, i8);
impl_as!(i32, u8);
impl_as!(i32, i16);
impl_as!(u16, u16);
impl_as!(i32, i32);
impl_as!(u32, u32);
impl_as!(i64, i64);
impl_as!(u64, u64);

/// Similar to num_traits, but we need to be able to customize values
pub trait Number {
    /// Zero value of this type
    const ZERO: Self;
    /// Smallest finite number this type can represent
    const MIN: Self;
    /// Largest finite number this type can represent
    const MAX: Self;
}

macro_rules! impl_num {
    ($N:ty, $zero:expr, $min:expr, $max:expr) => {
        impl Number for $N {
            const ZERO: $N = $zero;
            const MIN: $N = $min;
            const MAX: $N = $max;
        }
    };
}

// lower upper bound per: https://webidl.spec.whatwg.org/#abstract-opdef-converttoint
impl_num!(u8, 0, u8::MIN, u8::MAX);
impl_num!(u16, 0, u16::MIN, u16::MAX);
impl_num!(u32, 0, u32::MIN, u32::MAX);
impl_num!(u64, 0, 0, (1 << 53) - 1);

impl_num!(i8, 0, i8::MIN, i8::MAX);
impl_num!(i16, 0, i16::MIN, i16::MAX);
impl_num!(i32, 0, i32::MIN, i32::MAX);
impl_num!(i64, 0, -(1 << 53) + 1, (1 << 53) - 1);

impl_num!(f32, 0.0, f32::MIN, f32::MAX);
impl_num!(f64, 0.0, f64::MIN, f64::MAX);

/// A trait to convert Rust types to `JSVal`s.
pub trait ToJSVal<'s> {
    /// Convert `self` to a `JSVal`. JSAPI failure results in `Err(ConversionError)`.
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError>;
}

/// Error type for conversions of Rust types to `JSVal`s.
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum ConversionError {
    /// Conversion failed, resulting in a pending JS exception.
    ExnPending,
    /// Conversion failed, without a pending JS exception.
    Failure(Cow<'static, CStr>),
}

/// A trait to convert `JSVal`s to Rust types.
pub trait FromJSVal: Sized {
    /// Optional configurable behaviour switch; use () for no configuration.
    type Config;
    /// Convert `val` to type `Self`.
    /// Optional configuration of type `T` can be passed as the `option`
    /// argument.
    fn from_jsval<'s>(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        option: Self::Config,
    ) -> Result<Self, ConversionError>;
}

/// Behavior for converting out-of-range integers.
#[derive(PartialEq, Eq, Clone)]
pub enum ConversionBehavior {
    /// Wrap into the integer's range.
    Default,
    /// Throw an exception.
    EnforceRange,
    /// Clamp into the integer's range.
    Clamp,
}

/// Try to cast the number to a smaller type, but
/// if it doesn't fit, it will return an error.
// https://searchfox.org/mozilla-esr128/rev/1aa97f9d67f7a7231e62af283eaa02a6b31380e1/dom/bindings/PrimitiveConversions.h#166
fn enforce_range<D>(scope: &Scope<'_>, d: f64) -> Result<D, ()>
where
    D: Number + As<f64>,
    f64: As<D>,
{
    if d.is_infinite() {
        unsafe {
            throw_type_error(
                scope.cx_mut().raw_cx(),
                c"value out of range in an EnforceRange argument",
            )
        };
        return Err(());
    }

    let rounded = d.signum() * d.abs().floor();
    if D::MIN.cast() <= rounded && rounded <= D::MAX.cast() {
        Ok(rounded.cast())
    } else {
        unsafe {
            throw_type_error(
                scope.cx_mut().raw_cx(),
                c"value out of range in an EnforceRange argument",
            )
        };
        Err(())
    }
}

/// WebIDL ConvertToInt (Clamp) conversion.
/// Spec: <https://webidl.spec.whatwg.org/#abstract-opdef-converttoint>
///
/// This function is ported from Gecko’s
/// [`PrimitiveConversionTraits_Clamp`](https://searchfox.org/firefox-main/rev/aee7c0f24f488cd7f5a835803b48dd0c0cb2fd5f/dom/bindings/PrimitiveConversions.h#226).
///
/// # Warning
/// This function must only be used when the target type `D` represents an
/// integer WebIDL type. Using it with non-integer types would be incorrect.
fn clamp_to<D>(d: f64) -> D
where
    D: Number + PrimInt + As<f64>,
    f64: As<D>,
{
    // NaN maps to zero.
    if d.is_nan() {
        return D::ZERO;
    }

    if d >= D::MAX.cast() {
        return D::MAX;
    }
    if d <= D::MIN.cast() {
        return D::MIN;
    }

    debug_assert!(d.is_finite());

    // Banker's rounding (round ties towards even).
    // We move away from 0 by 0.5 and then truncate. That gets us the right
    // answer for any starting value except plus or minus N.5. With a starting
    // value of that form, we now have plus or minus N+1. If N is odd, this is
    // the correct result. If N is even, plus or minus N is the correct result.
    let to_truncate = if d < 0.0 { d - 0.5 } else { d + 0.5 };

    let mut truncated: D = to_truncate.cast();

    if truncated.cast() == to_truncate {
        // It was a tie (since moving away from 0 by 0.5 gave us the exact integer
        // we want). Since we rounded away from 0, we either already have an even
        // number or we have an odd number but the number we want is one closer to
        // 0. So just unconditionally masking out the ones bit should do the trick
        // to get us the value we want.
        truncated = truncated & !D::one();
    }

    truncated
}

// https://heycam.github.io/webidl/#es-void
impl<'s> ToJSVal<'s> for () {
    #[inline]
    fn to_jsval(&self, _scope: &'s Scope<'_>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(HandleValue::undefined())
    }
}

impl FromJSVal for JSVal {
    type Config = ();
    fn from_jsval(
        _scope: &Scope<'_>,
        value: HandleValue,
        _option: (),
    ) -> Result<JSVal, ConversionError> {
        Ok(value.get())
    }
}

impl<'s> ToJSVal<'s> for JSVal {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(scope.root_value(*self))
    }
}

impl<'s> ToJSVal<'s> for HandleValue<'s> {
    #[inline]
    fn to_jsval(&self, _scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(*self)
    }
}

impl<'s> ToJSVal<'s> for Heap<JSVal> {
    #[inline]
    fn to_jsval(&self, _scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(unsafe { HandleValue::from_marked_location(self.get_unsafe()) })
    }
}

#[inline]
fn convert_int_from_jsval<T, M>(
    scope: &Scope<'_>,
    value: HandleValue,
    option: ConversionBehavior,
    convert_fn: unsafe fn(*mut JSContext, HandleValue) -> Result<M, ()>,
) -> Result<T, ConversionError>
where
    T: Number + As<f64> + PrimInt,
    M: Number + As<T>,
    f64: As<T>,
{
    let result = match option {
        ConversionBehavior::Default => {
            unsafe { convert_fn(scope.cx_mut().raw_cx(), value) }.map(|v| v.cast())
        }
        _ => match unsafe { ToNumber(scope.cx_mut().raw_cx(), value) } {
            Ok(num) => {
                if matches!(option, ConversionBehavior::EnforceRange) {
                    enforce_range(scope, num)
                } else {
                    Ok(clamp_to(num))
                }
            }
            Err(()) => Err(()),
        },
    };
    result.map_err(|_| ConversionError::ExnPending)
}

// https://heycam.github.io/webidl/#es-boolean
impl<'s> ToJSVal<'s> for bool {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(scope.root_value(BooleanValue(*self)))
    }
}

// https://heycam.github.io/webidl/#es-boolean
impl FromJSVal for bool {
    type Config = ();
    fn from_jsval(
        _scope: &Scope<'_>,
        val: HandleValue,
        _option: (),
    ) -> Result<bool, ConversionError> {
        Ok(unsafe { ToBoolean(val) })
    }
}

// https://heycam.github.io/webidl/#es-byte
impl<'s> ToJSVal<'s> for i8 {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(scope.root_value(Int32Value(*self as i32)))
    }
}

// https://heycam.github.io/webidl/#es-byte
impl FromJSVal for i8 {
    type Config = ConversionBehavior;
    fn from_jsval<'s>(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        option: ConversionBehavior,
    ) -> Result<i8, ConversionError> {
        convert_int_from_jsval(scope, val, option, ToInt32)
    }
}

// https://heycam.github.io/webidl/#es-octet
impl<'s> ToJSVal<'s> for u8 {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(scope.root_value(Int32Value(*self as i32)))
    }
}

// https://heycam.github.io/webidl/#es-octet
impl FromJSVal for u8 {
    type Config = ConversionBehavior;
    fn from_jsval<'s>(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        option: ConversionBehavior,
    ) -> Result<u8, ConversionError> {
        convert_int_from_jsval(scope, val, option, ToInt32)
    }
}

// https://heycam.github.io/webidl/#es-short
impl<'s> ToJSVal<'s> for i16 {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(scope.root_value(Int32Value(*self as i32)))
    }
}

// https://heycam.github.io/webidl/#es-short
impl FromJSVal for i16 {
    type Config = ConversionBehavior;
    fn from_jsval<'s>(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        option: ConversionBehavior,
    ) -> Result<i16, ConversionError> {
        convert_int_from_jsval(scope, val, option, ToInt32)
    }
}

// https://heycam.github.io/webidl/#es-unsigned-short
impl<'s> ToJSVal<'s> for u16 {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(scope.root_value(Int32Value(*self as i32)))
    }
}

// https://heycam.github.io/webidl/#es-unsigned-short
impl FromJSVal for u16 {
    type Config = ConversionBehavior;
    fn from_jsval<'s>(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        option: ConversionBehavior,
    ) -> Result<u16, ConversionError> {
        convert_int_from_jsval(scope, val, option, ToUint16)
    }
}

// https://heycam.github.io/webidl/#es-long
impl<'s> ToJSVal<'s> for i32 {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(scope.root_value(Int32Value(*self)))
    }
}

// https://heycam.github.io/webidl/#es-long
impl FromJSVal for i32 {
    type Config = ConversionBehavior;
    fn from_jsval<'s>(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        option: ConversionBehavior,
    ) -> Result<i32, ConversionError> {
        convert_int_from_jsval(scope, val, option, ToInt32)
    }
}

// https://heycam.github.io/webidl/#es-unsigned-long
impl<'s> ToJSVal<'s> for u32 {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(scope.root_value(UInt32Value(*self)))
    }
}

// https://heycam.github.io/webidl/#es-unsigned-long
impl FromJSVal for u32 {
    type Config = ConversionBehavior;
    fn from_jsval<'s>(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        option: ConversionBehavior,
    ) -> Result<u32, ConversionError> {
        convert_int_from_jsval(scope, val, option, ToUint32)
    }
}

// https://heycam.github.io/webidl/#es-long-long
impl<'s> ToJSVal<'s> for i64 {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(scope.root_value(DoubleValue(*self as f64)))
    }
}

// https://heycam.github.io/webidl/#es-long-long
impl FromJSVal for i64 {
    type Config = ConversionBehavior;
    fn from_jsval<'s>(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        option: ConversionBehavior,
    ) -> Result<i64, ConversionError> {
        convert_int_from_jsval(scope, val, option, ToInt64)
    }
}

// https://heycam.github.io/webidl/#es-unsigned-long-long
impl<'s> ToJSVal<'s> for u64 {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(scope.root_value(DoubleValue(*self as f64)))
    }
}

// https://heycam.github.io/webidl/#es-unsigned-long-long
impl FromJSVal for u64 {
    type Config = ConversionBehavior;
    fn from_jsval<'s>(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        option: ConversionBehavior,
    ) -> Result<u64, ConversionError> {
        convert_int_from_jsval(scope, val, option, ToUint64)
    }
}

// https://heycam.github.io/webidl/#es-float
impl<'s> ToJSVal<'s> for f32 {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(scope.root_value(DoubleValue(*self as f64)))
    }
}

// https://heycam.github.io/webidl/#es-float
impl FromJSVal for f32 {
    type Config = ();
    fn from_jsval<'s>(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        _option: (),
    ) -> Result<f32, ConversionError> {
        match unsafe { ToNumber(scope.cx_mut().raw_cx(), val) } {
            Ok(result) => Ok(result as f32),
            Err(_) => Err(ConversionError::ExnPending),
        }
    }
}

// https://heycam.github.io/webidl/#es-double
impl<'s> ToJSVal<'s> for f64 {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(scope.root_value(DoubleValue(*self)))
    }
}

// https://heycam.github.io/webidl/#es-double
impl FromJSVal for f64 {
    type Config = ();
    fn from_jsval<'s>(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        _option: (),
    ) -> Result<f64, ConversionError> {
        match unsafe { ToNumber(scope.cx_mut().raw_cx(), val) } {
            Ok(result) => Ok(result),
            Err(_) => Err(ConversionError::ExnPending),
        }
    }
}

/// Converts a `JSString`, encoded in "Latin1" (i.e. U+0000-U+00FF encoded as 0x00-0xFF) into a
/// `String`.
pub fn latin1_to_string(scope: &Scope<'_>, s: NonNull<JSString>) -> String {
    assert!(unsafe { JS_DeprecatedStringHasLatin1Chars(s.as_ptr()) });

    let mut length = 0;
    let chars = unsafe {
        let chars = JS_GetLatin1StringCharsAndLength(
            scope.cx_mut().raw_cx(),
            ptr::null(),
            s.as_ptr(),
            &mut length,
        );
        assert!(!chars.is_null());

        slice::from_raw_parts(chars, length)
    };
    // The `encoding.rs` documentation for `convert_latin1_to_utf8` states that:
    // > The length of the destination buffer must be at least the length of the source
    // > buffer times two.
    let mut v = vec![0; chars.len() * 2];
    let real_size = encoding_rs::mem::convert_latin1_to_utf8(chars, v.as_mut_slice());

    v.truncate(real_size);

    // Safety: convert_latin1_to_utf8 converts the raw bytes to utf8 and the
    // buffer is the size specified in the documentation, so this should be safe.
    unsafe { String::from_utf8_unchecked(v) }
}

/// Converts a `JSString` into a `String`, regardless of used encoding.
pub fn jsstr_to_string(scope: &Scope<'_>, jsstr: NonNull<JSString>) -> String {
    if unsafe { JS_DeprecatedStringHasLatin1Chars(jsstr.as_ptr()) } {
        return latin1_to_string(scope, jsstr);
    }

    let mut length = 0;
    let chars = unsafe {
        JS_GetTwoByteStringCharsAndLength(
            scope.cx_mut().raw_cx(),
            ptr::null(),
            jsstr.as_ptr(),
            &mut length,
        )
    };
    assert!(!chars.is_null());
    let char_vec = unsafe { slice::from_raw_parts(chars, length) };
    String::from_utf16_lossy(char_vec)
}

// https://heycam.github.io/webidl/#es-USVString
impl<'s> ToJSVal<'s> for str {
    #[inline]
    #[deny(unsafe_op_in_unsafe_fn)]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        // Spidermonkey will automatically only copy latin1
        // or similar if the given encoding can be small enough.
        // So there is no need to distinguish between ascii only or similar.
        let s = Utf8Chars::from(self);
        let jsstr = unsafe { JS_NewStringCopyUTF8N(scope.cx_mut().raw_cx(), &*s as *const _) };
        if jsstr.is_null() {
            panic!("JS String copy routine failed");
        }
        Ok(scope.root_value(StringValue(unsafe { &*jsstr })))
    }
}

// https://heycam.github.io/webidl/#es-USVString
impl<'s> ToJSVal<'s> for String {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        ToJSVal::to_jsval(&(**self), scope)
    }
}

// https://heycam.github.io/webidl/#es-USVString
impl FromJSVal for String {
    type Config = ();
    fn from_jsval<'s>(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        _: (),
    ) -> Result<String, ConversionError> {
        let jsstr = unsafe { ToString(scope.cx_mut().raw_cx(), val) };
        let Some(jsstr) = NonNull::new(jsstr) else {
            return Err(ConversionError::ExnPending);
        };
        Ok(jsstr_to_string(scope, jsstr))
    }
}

impl<'s, T: ToJSVal<'s>> ToJSVal<'s> for Option<T> {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        match self {
            Some(value) => value.to_jsval(scope),
            None => Ok(HandleValue::null()),
        }
    }
}

impl<T: FromJSVal> FromJSVal for Option<T> {
    type Config = T::Config;
    fn from_jsval<'s>(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        option: T::Config,
    ) -> Result<Option<T>, ConversionError> {
        if val.get().is_null_or_undefined() {
            Ok(None)
        } else {
            FromJSVal::from_jsval(scope, val, option).map(Some)
        }
    }
}

impl<'s, T: ToJSVal<'s>> ToJSVal<'s> for &'_ T {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        (**self).to_jsval(scope)
    }
}

impl<'s, T: ToJSVal<'s>> ToJSVal<'s> for Box<T> {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        (**self).to_jsval(scope)
    }
}

impl<'s, T: ToJSVal<'s>> ToJSVal<'s> for Rc<T> {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        (**self).to_jsval(scope)
    }
}

// https://heycam.github.io/webidl/#es-sequence
impl<'s, T: ToJSVal<'s>> ToJSVal<'s> for [T] {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        let array =
            crate::Array::new(scope, self.len()).map_err(|_| ConversionError::ExnPending)?;

        for (index, obj) in self.iter().enumerate() {
            // TODO: this would be much better to do with a reused rooted value,
            //       which we don't currently have an API for.
            let val = obj.to_jsval(scope)?;

            if !unsafe {
                JS_DefineElement(
                    scope.cx_mut().raw_cx(),
                    array.handle().into(),
                    index as u32,
                    val.into(),
                    JSPROP_ENUMERATE as u32,
                )
            } {
                return Err(ConversionError::ExnPending);
            }
        }

        array.to_jsval(scope)
    }
}

// https://heycam.github.io/webidl/#es-sequence
impl<'s, T: ToJSVal<'s>> ToJSVal<'s> for Vec<T> {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'_>) -> Result<HandleValue<'s>, ConversionError> {
        ToJSVal::to_jsval(self.as_slice(), scope)
    }
}

/// Rooting guard for the iterator field of ForOfIterator.
/// Behaves like RootedGuard (roots on creation, unroots on drop),
/// but borrows and allows access to the whole ForOfIterator, so
/// that methods on ForOfIterator can still be used through it.
struct ForOfIteratorGuard<'s> {
    root: &'s mut ForOfIterator,
}

impl<'s> ForOfIteratorGuard<'s> {
    fn new(scope: &'s Scope<'_>, root: &'s mut ForOfIterator) -> Self {
        unsafe {
            Rooted::add_to_root_stack(&raw mut root.iterator, scope.cx_mut().raw_cx());
        }
        ForOfIteratorGuard { root }
    }
}

impl<'s> Drop for ForOfIteratorGuard<'s> {
    fn drop(&mut self) {
        unsafe {
            self.root.iterator.remove_from_root_stack();
        }
    }
}

impl<C: Clone, T: FromJSVal<Config = C>> FromJSVal for Vec<T> {
    type Config = C;

    fn from_jsval<'s>(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        option: C,
    ) -> Result<Vec<T>, ConversionError> {
        if !val.is_object() {
            return Err(ConversionError::Failure(c"Value is not an object".into()));
        }

        // Depending on the version of LLVM in use, bindgen can end up including
        // a padding field in the ForOfIterator. To support multiple versions of
        // LLVM that may not have the same fields as a result, we create an empty
        // iterator instance and initialize a non-empty instance using the empty
        // instance as a base value.
        let mut iterator = ForOfIterator {
            cx_: unsafe { scope.cx_mut().raw_cx() },
            iterator: RootedObject::new_unrooted(ptr::null_mut()),
            nextMethod: RootedValue::new_unrooted(JSVal { asBits_: 0 }),
            index: u32::MAX, // NOT_ARRAY
        };
        let iterator = ForOfIteratorGuard::new(scope, &mut iterator);
        let iterator: &mut ForOfIterator = &mut *iterator.root;

        if !unsafe {
            iterator.init(
                val.into(),
                ForOfIterator_NonIterableBehavior::AllowNonIterable,
            )
        } {
            return Err(ConversionError::ExnPending);
        }

        if iterator.iterator.data.is_null() {
            return Err(ConversionError::Failure(c"Value is not iterable".into()));
        }

        let mut ret = vec![];

        loop {
            let mut done = false;
            rooted!(in(unsafe { scope.cx_mut().raw_cx() }) let mut val = UndefinedValue());
            if !unsafe { iterator.next(val.handle_mut().into(), &mut done) } {
                return Err(ConversionError::ExnPending);
            }

            if done {
                break;
            }

            ret.push(T::from_jsval(scope, val.handle(), option.clone())?);
        }

        Ok(ret)
    }
}

// https://heycam.github.io/webidl/#es-object
impl<'s> ToJSVal<'s> for *mut JSObject {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        rooted!(in(unsafe { scope.cx_mut().raw_cx() }) let mut rval = UndefinedValue());
        rval.set(ObjectOrNullValue(*self));
        unsafe { maybe_wrap_object_or_null_value(scope.cx_mut().raw_cx(), rval.handle_mut()) };
        Ok(scope.root_value(rval.get()))
    }
}

// https://heycam.github.io/webidl/#es-object
impl<'s> ToJSVal<'s> for ptr::NonNull<JSObject> {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        rooted!(in(unsafe { scope.cx_mut().raw_cx() }) let mut rval = UndefinedValue());
        rval.set(ObjectValue(self.as_ptr()));
        unsafe { maybe_wrap_object_value(scope.cx_mut().raw_cx(), rval.handle_mut()) };
        Ok(scope.root_value(rval.get()))
    }
}

// https://heycam.github.io/webidl/#es-object
impl<'s> ToJSVal<'s> for Heap<*mut JSObject> {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        rooted!(in(unsafe { scope.cx_mut().raw_cx() }) let mut rval = UndefinedValue());
        rval.set(ObjectOrNullValue(self.get()));
        unsafe { maybe_wrap_object_or_null_value(scope.cx_mut().raw_cx(), rval.handle_mut()) };
        Ok(scope.root_value(rval.get()))
    }
}

// https://heycam.github.io/webidl/#es-object
impl FromJSVal for *mut JSObject {
    type Config = ();
    #[inline]
    fn from_jsval<'s>(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        _option: (),
    ) -> Result<*mut JSObject, ConversionError> {
        if !val.is_object() {
            unsafe { throw_type_error(scope.cx_mut().raw_cx(), c"value is not an object") };
            return Err(ConversionError::ExnPending);
        }

        unsafe { AssertSameCompartment(scope.cx_mut().raw_cx(), val.to_object()) };

        Ok(val.to_object())
    }
}

impl<'s> ToJSVal<'s> for *mut JS::Symbol {
    #[inline]
    fn to_jsval(&self, scope: &'s Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(scope.root_value(SymbolValue(unsafe { &**self })))
    }
}

impl FromJSVal for *mut JS::Symbol {
    type Config = ();
    #[inline]
    fn from_jsval<'s>(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        _option: (),
    ) -> Result<*mut JS::Symbol, ConversionError> {
        if !val.is_symbol() {
            unsafe { throw_type_error(scope.cx_mut().raw_cx(), c"value is not a symbol") };
            return Err(ConversionError::ExnPending);
        }

        Ok(val.to_symbol())
    }
}

/// A wrapper type over [`mozjs::jsapi::UTF8Chars`]. This is created to help transferring
/// a rust string to mozjs. The inner [`mozjs::jsapi::UTF8Chars`] can be accessed via the
/// [`std::ops::Deref`] trait.
pub struct Utf8Chars<'s> {
    lt_marker: std::marker::PhantomData<&'s ()>,
    inner: mozjs::jsapi::UTF8Chars,
}

impl<'s> std::ops::Deref for Utf8Chars<'s> {
    type Target = mozjs::jsapi::UTF8Chars;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'s> From<&'s str> for Utf8Chars<'s> {
    #[allow(unsafe_code)]
    fn from(value: &'s str) -> Self {
        use std::marker::PhantomData;

        use mozjs::jsapi::mozilla::{Range, RangedPtr};
        use mozjs::jsapi::UTF8Chars;

        let range = value.as_bytes().as_ptr_range();
        let range_start = range.start as *mut _;
        let range_end = range.end as *mut _;
        let start = RangedPtr {
            _phantom_0: PhantomData,
            mPtr: range_start,
            #[cfg(feature = "debugmozjs")]
            mRangeStart: range_start,
            #[cfg(feature = "debugmozjs")]
            mRangeEnd: range_end,
        };
        let end = RangedPtr {
            _phantom_0: PhantomData,
            mPtr: range_end,
            #[cfg(feature = "debugmozjs")]
            mRangeStart: range_start,
            #[cfg(feature = "debugmozjs")]
            mRangeEnd: range_end,
        };
        let base = Range {
            _phantom_0: PhantomData,
            mStart: start,
            mEnd: end,
        };
        let inner = UTF8Chars { _base: base };
        Self {
            lt_marker: PhantomData,
            inner,
        }
    }
}
