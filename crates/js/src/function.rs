// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Function creation, calling, and closure-based callbacks.
//!
//! The [`Function`] marker type implements
//! [`JSType`](crate::builtins::JSType), enabling
//! [`Function<'s>`](crate::Function) as the scope-rooted function handle
//! type. It implements `Deref` to [`Object<'s>`](crate::Object), so all
//! property access methods are available directly.
//!
//! # Closure-Based Callbacks
//!
//! Use [`Function::new_closure`] to create a JS function backed by a Rust
//! closure. The closure receives a [`Scope`] for interacting with the JS
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

use std::ffi::CStr;
use std::os::raw::c_uint;
use std::ptr;
use std::ptr::NonNull;

use crate::builtins::JSType;
use crate::gc::handle::Stack;
use crate::gc::scope::Scope;
use crate::Object;
use mozjs::gc::{HandleObject, HandleValue};
use mozjs::jsapi::{
    GCContext, HandleValueArray, JSClass, JSClassOps, JSFunction, JSNative, JSObject, Value,
    JSCLASS_FOREGROUND_FINALIZE, JSCLASS_RESERVED_SLOTS_SHIFT,
};
use mozjs::jsval::UndefinedValue;
use mozjs::rooted;
use mozjs::rust::wrappers2;

use super::error::ExnThrown;

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
    pub fn new_closure<F>(
        scope: &'s Scope<'_>,
        name: &CStr,
        nargs: c_uint,
        f: F,
    ) -> Result<Self, ExnThrown>
    where
        F: Fn(&Scope<'_>, &CallbackArgs<'_>) -> Result<Value, ExnThrown> + 'static,
    {
        // Box the closure, then box the trait object to get a thin pointer.
        // `dyn Fn` trait objects are fat pointers (data + vtable), but
        // PrivateValue can only store a single pointer. Double-boxing gives
        // us a thin `*mut ClosureBox` that we can safely round-trip.
        let boxed: ClosureBox = Box::new(f);
        let raw: *mut ClosureBox = Box::into_raw(Box::new(boxed));

        // Create a carrier object that owns the closure pointer and will free
        // it in its GC finalizer.
        rooted!(in(unsafe { scope.raw_cx_no_gc() }) let carrier = unsafe {
            wrappers2::JS_NewObjectWithGivenProto(
                scope.cx_mut(),
                &CLOSURE_CARRIER_CLASS,
                mozjs::gc::HandleObject::null(),
            )
        });
        if carrier.get().is_null() {
            // Allocation failed — free the already-boxed closure before returning.
            unsafe { drop(Box::from_raw(raw)) };
            return Err(ExnThrown);
        }

        // Store the closure pointer in the carrier's reserved slot 0.
        unsafe {
            mozjs::jsapi::JS_SetReservedSlot(
                carrier.get(),
                0,
                &mozjs::jsval::PrivateValue(raw as *const std::ffi::c_void),
            );
        }

        // Create the function itself.
        let fun = Self::new_with_reserved(scope, Some(closure_trampoline), nargs, 0, name)?;

        // Store the carrier object in the function's reserved slot 0 as an
        // ObjectValue. This keeps the carrier alive (traced) as long as the
        // function is alive.
        unsafe {
            let fun_obj = fun.handle().get();
            mozjs::jsapi::SetFunctionNativeReserved(
                fun_obj,
                0,
                &mozjs::jsval::ObjectValue(carrier.get()),
            );
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
    pub fn get(&self, i: u32) -> Value {
        self.args.get(i).get()
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

/// Type-erased closure stored in a helper object's reserved slot.
type ClosureBox = Box<dyn Fn(&Scope<'_>, &CallbackArgs<'_>) -> Result<Value, ExnThrown>>;

// ---------------------------------------------------------------------------
// Closure carrier: a helper JSObject that stores the closure pointer and
// frees it in its `finalize` callback when garbage-collected.
// ---------------------------------------------------------------------------

/// Invoked by the GC when a closure-carrier object is collected.
/// Reconstructs and drops the `Box<ClosureBox>` to free the closure.
///
/// # Safety
///
/// SpiderMonkey guarantees `obj` is a valid object with our class. Slot 0
/// contains the `PrivateValue` we stored in `new_closure`.
unsafe extern "C" fn closure_carrier_finalize(_gcx: *mut GCContext, obj: *mut JSObject) {
    let mut slot = Value::default();
    mozjs::glue::JS_GetReservedSlot(obj, 0, &mut slot);
    let raw = slot.to_private() as *mut ClosureBox;
    if !raw.is_null() {
        // Reconstruct the Box and let it drop, freeing the closure.
        drop(Box::from_raw(raw));
    }
}

/// Class ops for the closure carrier — only the finalize hook is set.
static CLOSURE_CARRIER_OPS: JSClassOps = JSClassOps {
    addProperty: None,
    delProperty: None,
    enumerate: None,
    newEnumerate: None,
    resolve: None,
    mayResolve: None,
    finalize: Some(closure_carrier_finalize),
    call: None,
    construct: None,
    trace: None,
};

/// A class for the hidden carrier object that stores the closure pointer.
/// It has one reserved slot (for the `PrivateValue` closure pointer) and
/// `JSCLASS_FOREGROUND_FINALIZE` so the GC invokes [`closure_carrier_finalize`]
/// on the main thread.
static CLOSURE_CARRIER_CLASS: JSClass = JSClass {
    name: c"ClosureCarrier".as_ptr(),
    flags: JSCLASS_FOREGROUND_FINALIZE | (1 << JSCLASS_RESERVED_SLOTS_SHIFT),
    cOps: &CLOSURE_CARRIER_OPS as *const JSClassOps,
    spec: ptr::null(),
    ext: ptr::null(),
    oOps: ptr::null(),
};

/// The extern "C" trampoline that bridges JSNative to the stored closure.
///
/// # Safety
///
/// This function is called by SpiderMonkey's function dispatch and expects:
/// - `vp` to point to a valid `CallArgs` frame
/// - The callee's reserved slot 0 to contain an ObjectValue referencing a
///   carrier object whose reserved slot 0 holds the `ClosureBox` pointer
unsafe extern "C" fn closure_trampoline(
    cx: *mut mozjs::jsapi::JSContext,
    argc: u32,
    vp: *mut Value,
) -> bool {
    let args = mozjs::jsapi::CallArgs::from_vp(vp, argc);

    // Step 1: Get the carrier object from the function's reserved slot 0.
    let callee = args.callee();
    let fn_slot = mozjs::jsapi::GetFunctionNativeReserved(callee, 0);
    let carrier = (*fn_slot).to_object_or_null();
    debug_assert!(!carrier.is_null(), "closure carrier must not be null");

    // Step 2: Get the closure pointer from the carrier's reserved slot 0.
    let mut carrier_slot = Value::default();
    mozjs::glue::JS_GetReservedSlot(carrier, 0, &mut carrier_slot);
    let outer_ptr = carrier_slot.to_private() as *mut ClosureBox;

    // Create a scope for the callback. We're inside a native call so a
    // realm is always entered.
    // SAFETY: SpiderMonkey guarantees cx is valid and a realm is entered
    // when calling a native function.
    let mut js_cx = mozjs::context::JSContext::from_ptr(std::ptr::NonNull::new_unchecked(cx));
    let scope = crate::gc::scope::RootScope::from_current_realm(&mut js_cx);

    let cb_args = CallbackArgs { args: &args };

    // Deref through the outer Box to reach the inner Box<dyn Fn>,
    // then call through the trait object.
    match (**outer_ptr)(&scope, &cb_args) {
        Ok(val) => {
            args.rval().set(val);
            true
        }
        Err(_) => {
            // If no exception is already pending, throw a generic one.
            if !mozjs::jsapi::JS_IsExceptionPending(cx) {
                let msg = std::ffi::CString::new("closure callback returned an error")
                    .unwrap_or_default();
                mozjs::jsapi::JS_ReportErrorASCII(cx, msg.as_ptr());
            }
            false
        }
    }
}
