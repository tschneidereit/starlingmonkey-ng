// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Function creation, calling, and callbacks.
//!
//! The [`Function`] marker type implements
//! [`JSType`](crate::builtins::JSType), enabling
//! [`Function<'s>`](crate::Function) as the scope-rooted function handle
//! type. It implements `Deref` to [`Object<'s>`](crate::Object), so all
//! property access methods are available directly.
//!
//! # Callbacks
//!
//! Use [`Function::new_callback`] to create a JS function backed by a Rust
//! callback. The callback receives a [`Scope`] for interacting with the JS
//! engine and a [`CallbackArgs`] struct for safe argument access.
//!
//! ```ignore
//! # use core_runtime::js::gc::scope::Scope;
//! # fn example(scope: &Scope<'_>) {
//! use core_runtime::js;
//!
//! let add = js::Function::new_closure(&scope, c"add", 2, |_scope, args| {
//!     let a = args.get_i32(0).unwrap_or(0);
//!     let b = args.get_i32(1).unwrap_or(0);
//!     Ok(js::value::from_i32(a + b))
//! }).unwrap();
//! # }
//! ```
//!
//! # Calling Functions
//!
//! Use the [`call`](Stack::call), [`call_value`](Stack::call_value), or
//! [`call_by_name`](Stack::call_by_name) methods to invoke JS functions
//! from Rust.

use std::borrow::Cow;
use std::ffi::CStr;
use std::os::raw::c_uint;
use std::ptr::NonNull;

use super::error::{report_error_ascii, ExnThrown};
use crate::builtins::JSType;
use crate::conversion::{ConversionError, FromJSVal, ToJSVal};
use crate::gc::handle::Stack;
use crate::gc::scope::Scope;
use crate::Object;
use mozjs::gc::{HandleObject, HandleValue};
use mozjs::jsapi::{
    GetFunctionNativeReserved, HandleValueArray, JSClass, JSFunction, JSNative,
    SetFunctionNativeReserved, Value,
};
use mozjs::jsval::UndefinedValue;
use mozjs::rust::wrappers2;

// ---------------------------------------------------------------------------
// Function marker type
// ---------------------------------------------------------------------------

/// Marker type for JavaScript `Function` objects.
///
/// [`Function<'s>`](crate::Function) is the scope-rooted handle type:
///
/// ```ignore
/// let fun = js::Function::define(&scope, global.handle(), c"greet", Some(my_native), 1, 0)?;
/// ```
///
/// `Function<'s>` derefs to [`Object<'s>`](crate::Object), so all property
/// access methods are available directly.
pub struct Function;

impl JSType for Function {
    const JS_NAME: &'static str = "Function";

    fn js_class() -> *const JSClass {
        crate::class::proto_key_to_class(mozjs::jsapi::JSProtoKey::JSProto_Function)
    }
}

impl<'s> Stack<'s, Function> {
    // ---------------------------------------------------------------------------
    // Function creation
    // ---------------------------------------------------------------------------

    /// Define a native function on an object.
    ///
    /// `name` is the JS-visible function name. `nargs` is the declared number of
    /// arguments (used for `Function.length`). `attrs` are property attributes.
    pub fn define(
        scope: &'s Scope<'_>,
        obj: HandleObject,
        name: &CStr,
        call: JSNative,
        nargs: c_uint,
        attrs: c_uint,
    ) -> Result<Self, ExnThrown> {
        let fun = unsafe {
            wrappers2::JS_DefineFunction(scope.cx_mut(), obj, name.as_ptr(), call, nargs, attrs)
        };
        let fun = NonNull::new(fun).ok_or(ExnThrown)?;
        let obj = unsafe { mozjs::jsapi::JS_GetFunctionObject(fun.as_ptr()) };
        let obj = NonNull::new(obj).ok_or(ExnThrown)?;
        Ok(unsafe { Self::from_handle_unchecked(scope.root_object(obj)) })
    }

    /// Create a new standalone function (not attached to an object).
    pub fn new(
        scope: &'s Scope<'_>,
        call: JSNative,
        nargs: c_uint,
        flags: c_uint,
        name: &CStr,
    ) -> Result<Self, ExnThrown> {
        let fun =
            unsafe { wrappers2::JS_NewFunction(scope.cx_mut(), call, nargs, flags, name.as_ptr()) };
        let fun = NonNull::new(fun).ok_or(ExnThrown)?;
        let obj = unsafe { mozjs::jsapi::JS_GetFunctionObject(fun.as_ptr()) };
        let obj = NonNull::new(obj).ok_or(ExnThrown)?;
        Ok(unsafe { Self::from_handle_unchecked(scope.root_object(obj)) })
    }

    /// Create a new function with reserved slots for storing closure data.
    ///
    /// Reserved slots can be accessed via `GetFunctionNativeExtra` /
    /// `SetFunctionNativeExtra`.
    pub fn new_with_reserved(
        scope: &'s Scope<'_>,
        call: JSNative,
        nargs: c_uint,
        flags: c_uint,
        name: &CStr,
    ) -> Result<Self, ExnThrown> {
        let fun = unsafe {
            wrappers2::NewFunctionWithReserved(scope.cx_mut(), call, nargs, flags, name.as_ptr())
        };
        let fun = NonNull::new(fun).ok_or(ExnThrown)?;
        let obj = unsafe { mozjs::jsapi::JS_GetFunctionObject(fun.as_ptr()) };
        let obj = NonNull::new(obj).ok_or(ExnThrown)?;
        Ok(unsafe { Self::from_handle_unchecked(scope.root_object(obj)) })
    }

    /// Get the underlying `JSFunction` pointer.
    pub fn as_function_ptr(&self) -> *mut JSFunction {
        unsafe { mozjs::jsapi::JS_GetObjectFunction(self.handle().get()) }
    }

    /// Get a reserved slot value on this function.
    ///
    /// # Safety
    ///
    /// `self` must have been created with `new_with_reserved`, or otherwise be guaranteed
    /// to be a function object with reserved slots.
    pub unsafe fn get_reserved<'r>(
        &self,
        scope: &'r Scope<'_>,
        slot: ReservedSlot,
    ) -> HandleValue<'r> {
        scope.root_value(*GetFunctionNativeReserved(self.as_raw(), slot.into()))
    }

    /// Set a reserved slot value on this function.
    ///
    /// # Safety
    ///
    /// `self` must have been created with `new_with_reserved`, or otherwise be guaranteed
    /// to be a function object with reserved slots.
    pub unsafe fn set_reserved(&self, slot: ReservedSlot, val: impl Into<Value>) {
        SetFunctionNativeReserved(self.as_raw(), slot.into(), &val.into());
    }

    // ---------------------------------------------------------------------------
    // Function calling
    // ---------------------------------------------------------------------------

    /// Call a function value with the given `this` object and arguments.
    pub fn call_value<'a>(
        scope: &Scope<'a>,
        this: HandleObject,
        fval: HandleValue,
        args: &HandleValueArray,
    ) -> Result<HandleValue<'a>, ExnThrown> {
        let mut rval = scope.root_value_mut(UndefinedValue());
        let ok = unsafe {
            wrappers2::JS_CallFunctionValue(scope.cx_mut(), this, fval, args, rval.reborrow())
        };
        ExnThrown::check(ok)?;
        Ok(rval.handle())
    }

    /// Call a named method on an object.
    pub fn call_by_name<'a>(
        scope: &Scope<'a>,
        obj: HandleObject,
        name: &CStr,
        args: &HandleValueArray,
    ) -> Result<HandleValue<'a>, ExnThrown> {
        let mut rval = scope.root_value_mut(UndefinedValue());
        let ok = unsafe {
            wrappers2::JS_CallFunctionName(
                scope.cx_mut(),
                obj,
                name.as_ptr(),
                args,
                rval.reborrow(),
            )
        };
        ExnThrown::check(ok)?;
        Ok(rval.handle())
    }

    /// Call a function object with a given `this` value.
    pub fn call<'a>(
        scope: &Scope<'a>,
        thisv: HandleValue,
        fun: HandleValue,
        args: &HandleValueArray,
    ) -> Result<HandleValue<'a>, ExnThrown> {
        let mut rval = scope.root_value_mut(UndefinedValue());
        let ok = unsafe { wrappers2::Call(scope.cx_mut(), thisv, fun, args, rval.reborrow()) };
        ExnThrown::check(ok)?;
        Ok(rval.handle())
    }

    /// Invoke the `new` operator on a constructor function.
    pub fn construct(
        scope: &'s Scope<'_>,
        fun: HandleValue,
        args: &HandleValueArray,
    ) -> Result<Object<'s>, ExnThrown> {
        let mut result = scope.root_object_mut(std::ptr::null_mut());
        let ok = unsafe { wrappers2::Construct1(scope.cx_mut(), fun, args, result.reborrow()) };
        ExnThrown::check(ok)?;
        Object::from_raw_obj(scope, result.get()).ok_or(ExnThrown)
    }

    /// Invoke the `new` operator on a constructor with an explicit `new.target`.
    pub fn construct_with_new_target(
        scope: &'s Scope<'_>,
        fun: HandleValue,
        new_target: HandleObject,
        args: &HandleValueArray,
    ) -> Result<Object<'s>, ExnThrown> {
        let mut result = scope.root_object_mut(std::ptr::null_mut());
        let ok = unsafe {
            wrappers2::Construct(scope.cx_mut(), fun, new_target, args, result.reborrow())
        };
        ExnThrown::check(ok)?;
        Object::from_raw_obj(scope, result.get()).ok_or(ExnThrown)
    }

    // ---------------------------------------------------------------------------
    // Function introspection
    // ---------------------------------------------------------------------------

    /// Get the `.length` property of this function.
    pub fn length(&self, scope: &Scope<'_>) -> Result<u16, ExnThrown> {
        let fun_ptr = self.as_function_ptr();
        let fun_nn = NonNull::new(fun_ptr).expect("function object has no JSFunction");
        let fun_handle = scope.root_function(fun_nn);
        let mut length: u16 = 0;
        let ok =
            unsafe { wrappers2::JS_GetFunctionLength(scope.cx_mut(), fun_handle, &mut length) };
        ExnThrown::check(ok)?;
        Ok(length)
    }

    // ---------------------------------------------------------------------------
    // Closure-based callbacks
    // ---------------------------------------------------------------------------

    /// Create a new JS function backed by a Rust closure.
    ///
    /// The closure receives a [`Scope`] for interacting with the JS engine
    /// (creating strings, objects, calling functions, etc.) and a [`CallbackArgs`]
    /// for safe argument access. It returns a [`Value`] (or an error) that is
    /// automatically set as the function's return value.
    ///
    /// The closure is stored in a hidden carrier object that is traced from the
    /// function's reserved slot. When the function is garbage-collected the
    /// carrier becomes unreachable, and its finalizer frees the closure.
    ///
    /// # Example
    ///
    /// ```ignore
    /// # use core_runtime::js::gc::scope::Scope;
    /// # fn example(scope: &Scope<'_>) {
    /// use core_runtime::js;
    ///
    /// let greet = js::Function::new_closure(scope, c"greet", 1, |scope, args| {
    ///     Ok(js::value::from_i32(42))
    /// }).unwrap();
    /// # }
    /// ```
    pub fn new_callback(
        scope: &'s Scope<'_>,
        name: &CStr,
        nargs: c_uint,
        cb: Callback,
        payload: impl ToJSVal<'s>,
    ) -> Result<Self, ExnThrown> {
        // Create the function itself.
        let fun = Self::new_with_reserved(scope, Some(callback_trampoline), nargs, 0, name)?;

        unsafe {
            fun.set_reserved(
                ReservedSlot::Slot0,
                mozjs::jsval::PrivateValue(cb as *const std::ffi::c_void),
            );
            fun.set_reserved(ReservedSlot::Slot1, payload.to_jsval(scope).unwrap().get());
        }

        Ok(fun)
    }
}

impl<'s> std::ops::Deref for Stack<'s, Function> {
    type Target = Object<'s>;

    fn deref(&self) -> &Object<'s> {
        // SAFETY: Stack<Function> and Stack<Object> are both repr(transparent)
        // over Handle<'s, *mut JSObject>, so they have identical layout.
        unsafe { &*(self as *const Stack<'s, Function> as *const Object<'s>) }
    }
}

impl<'s> FromJSVal<'s> for Stack<'s, Function> {
    type Config = ();

    fn from_jsval(
        scope: &'s Scope<'s>,
        val: HandleValue<'s>,
        _option: Self::Config,
    ) -> Result<Self, ConversionError> {
        Object::from_value(scope, *val)?
            .cast::<Self>()
            .map_err(|_| ConversionError::Failure(Cow::Borrowed(c"Value isn't a Function")))
    }
}

pub enum ReservedSlot {
    Slot0 = 0,
    Slot1 = 1,
    Slot2 = 2,
}

impl From<ReservedSlot> for usize {
    fn from(slot: ReservedSlot) -> Self {
        slot as usize
    }
}

// ---------------------------------------------------------------------------
// Safe callback trait
// ---------------------------------------------------------------------------

/// Trait for defining safe native callbacks callable from JavaScript.
///
/// Implementors provide a `call` method that receives a safe context and
/// parsed arguments, instead of raw pointers.
///
/// # Example
///
/// ```ignore
/// use crate::function::JSCallable;
/// use crate::error::ExnThrown;
/// use mozjs::context::JSContext;
/// use mozjs::jsapi::CallArgs;
///
/// struct MyFunction;
///
/// impl JSCallable for MyFunction {
///     fn call(
///         &self,
///         cx: &mut JSContext,
///         args: &CallArgs,
///     ) -> Result<(), ExnThrown> {
///         // Implementation here
///         Ok(())
///     }
/// }
/// ```
///
/// Register a `JSCallable` implementor as a native function by wrapping it
/// in an `unsafe extern "C"` callback. The raw callback is inherently unsafe
/// at the FFI boundary, but the *implementation* via [`JSCallable::call`] can
/// be safe.
pub trait JSCallable {
    /// Handle a call from JavaScript.
    ///
    /// `args` provides access to `this`, the arguments, and the return value
    /// slot. Set the return value via `args.rval().set(...)`.
    fn call(
        &self,
        cx: &mut mozjs::context::JSContext,
        args: &mozjs::jsapi::CallArgs,
    ) -> Result<(), ExnThrown>;
}

// ---------------------------------------------------------------------------
// Closure-based callbacks
// ---------------------------------------------------------------------------

/// Safe wrapper around [`CallArgs`](mozjs::jsapi::CallArgs) for use in
/// closure-based callbacks.
///
/// Provides indexed access to arguments, the `this` value, and the argument
/// count.
pub struct CallbackArgs<'a> {
    args: &'a mozjs::jsapi::CallArgs,
}

impl<'a> CallbackArgs<'a> {
    /// Number of arguments passed by the caller.
    #[inline]
    pub fn len(&self) -> u32 {
        self.args.argc_
    }

    /// Whether no arguments were passed.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.args.argc_ == 0
    }

    /// Get argument `i` as a raw [`Value`].
    ///
    /// Returns `undefined` if `i` is out of range.
    #[inline]
    pub fn get(&'a self, i: u32) -> HandleValue<'a> {
        unsafe { HandleValue::from_raw(self.args.get(i)) }
    }

    /// Get argument `i` as an `i32`, or `None` if it isn't an int32.
    #[inline]
    pub fn get_i32(&self, i: u32) -> Option<i32> {
        let v = self.args.get(i).get();
        if v.is_int32() {
            Some(v.to_int32())
        } else {
            None
        }
    }

    /// Get argument `i` as an `f64`, or `None` if it isn't a number.
    #[inline]
    pub fn get_f64(&self, i: u32) -> Option<f64> {
        let v = self.args.get(i).get();
        if v.is_double() {
            Some(v.to_double())
        } else if v.is_int32() {
            Some(v.to_int32() as f64)
        } else {
            None
        }
    }

    /// Get argument `i` as a `bool`, or `None` if it isn't a boolean.
    #[inline]
    pub fn get_bool(&self, i: u32) -> Option<bool> {
        let v = self.args.get(i).get();
        if v.is_boolean() {
            Some(v.to_boolean())
        } else {
            None
        }
    }

    /// Get the `this` value.
    #[inline]
    pub fn this(&self) -> Value {
        self.args.thisv().get()
    }

    /// Whether this is a constructor call (`new`).
    #[inline]
    pub fn is_constructing(&self) -> bool {
        self.args.is_constructing()
    }
}

/// Type-erased callback stored in a helper function's reserved slot.
pub type Callback = fn(&Scope<'_>, CallbackArgs<'_>, HandleValue) -> Result<Value, ExnThrown>;

/// The extern "C" trampoline that bridges JSNative to the stored callback.
///
/// # Safety
///
/// This function is called by SpiderMonkey's function dispatch and expects:
/// - `vp` to point to a valid `CallArgs` frame
/// - The callee's reserved slot 0 to contain a function with the `Callback` signature
/// - The callee's reserved slot 1 to contain the closure payload as a `Value`
unsafe extern "C" fn callback_trampoline(
    cx: *mut mozjs::jsapi::JSContext,
    argc: u32,
    vp: *mut Value,
) -> bool {
    let args = mozjs::jsapi::CallArgs::from_vp(vp, argc);

    // Create a scope for the callback. We're inside a native call so a
    // realm is always entered.
    // SAFETY: SpiderMonkey guarantees cx is valid and a realm is entered
    // when calling a native function.
    let scope = crate::gc::scope::RootScope::from_current_realm(cx);

    // Get the callback and payload from the function's reserved slots.
    let callee = args.callee();
    let cb: Callback =
        std::mem::transmute((*mozjs::jsapi::GetFunctionNativeReserved(callee, 0)).to_private());
    let payload = scope.root_value(*mozjs::jsapi::GetFunctionNativeReserved(callee, 1));

    let cb_args = CallbackArgs { args: &args };

    // Call the function pointer.
    match cb(&scope, cb_args, payload) {
        Ok(val) => {
            args.rval().set(val);
            true
        }
        Err(_) => {
            // If no exception is already pending, throw a generic one.
            if !mozjs::jsapi::JS_IsExceptionPending(cx) {
                report_error_ascii(&scope, c"Native callback returned an error");
            }
            false
        }
    }
}
