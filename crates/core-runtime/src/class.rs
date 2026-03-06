// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! High-level facilities for defining JavaScript classes backed by Rust structs.
//!
//! This module provides an ergonomic way to expose Rust types as JavaScript classes,
//! inspired by rquickjs's class system but built on top of SpiderMonkey's JSAPI.
//!
//! # Overview
//!
//! The core abstraction is the [`ClassDef`] trait, which describes how a Rust struct
//! maps to a JavaScript class.  In practice [`ClassDef`] is rarely implemented by hand
//! — the [`#[jsclass]`](macro@jsclass) and [`#[jsmethods]`](macro@jsmethods) proc
//! macros generate the boilerplate (inner data struct, `'s`-lifetime stack newtype, and
//! `HeapRef` wrapper) from ordinary Rust struct and impl syntax.
//!
//! # Example
//!
//! ```rust,ignore
//! use libstarling::{jsclass, jsmethods};
//!
//! #[jsclass]
//! struct Counter {
//!     value: i32,
//! }
//!
//! #[jsmethods]
//! impl Counter {
//!     #[constructor]
//!     fn new(initial: i32) -> Self {
//!         Self { value: initial }
//!     }
//!
//!     #[method]
//!     fn increment(&mut self) {
//!         self.value += 1;
//!     }
//!
//!     #[getter]
//!     fn value(&self) -> i32 {
//!         self.value
//!     }
//! }
//!
//! // Register on a global object and create an instance from Rust:
//! Counter::add_to_global(&scope, global);
//! let counter = Counter::new(&scope, 0);
//! ```

#![allow(clippy::result_unit_err)]

use std::any::TypeId;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::{c_void, CString};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::ptr::{self, NonNull};

use js::class_spec::{
    JSClass, JSClassOps, JSFunctionSpec, JSPropertySpec, JS_EnumerateStandardClasses,
    JS_GlobalObjectTraceHook, JS_MayResolveStandardClass, JS_ResolveStandardClass,
    JSCLASS_FOREGROUND_FINALIZE, JSCLASS_GLOBAL_SLOT_COUNT, JSCLASS_IS_GLOBAL,
    JSCLASS_RESERVED_SLOTS_MASK, JSCLASS_RESERVED_SLOTS_SHIFT,
};
use js::conversions::{
    ConversionBehavior, ConversionResult, FromJSValConvertible, ToJSValConvertible,
};
use js::error::JSError;
use js::heap::{Heap, RootedTraceableBox, Trace};
use js::native::{
    CallArgs, GCContext, JSContext, JSNative, JSObject, JSTracer, MutableHandleValue, RawJSContext,
    Value,
};
use js::object::Object;
use js::value;

// ============================================================================
// Marker types
// ============================================================================

/// Typed variadic rest arguments in `#[jsmethods]`.
///
/// Use this as the type of the last parameter to collect all remaining
/// JS arguments. The macro generates code that converts each argument
/// to the specified type `T` using the [`FromJSValue`] trait.
///
/// The default type parameter is `Value`, which collects raw JS values.
///
/// # Examples
///
/// ```rust,ignore
/// // Collect typed f64 arguments — no manual conversion needed:
/// #[static_method]
/// fn sum(rest: RestArgs<f64>) -> f64 {
///     rest.iter().sum()
/// }
///
/// // Raw Value access (same as the default):
/// #[method]
/// fn process(&self, rest: RestArgs<Value>) -> String { ... }
/// ```
pub struct RestArgs<T = Value>(Vec<T>);

impl<T> RestArgs<T> {
    /// Creates a new `RestArgs` from a pre-converted vector.
    pub fn new(values: Vec<T>) -> Self {
        Self(values)
    }

    /// Returns the number of rest arguments.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` if there are no rest arguments.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns an iterator over the rest arguments.
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.0.iter()
    }
}

impl<T> std::ops::Deref for RestArgs<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T> IntoIterator for RestArgs<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a RestArgs<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

// ============================================================================
// FromJSValue trait — type-safe conversion from JS values
// ============================================================================

/// Trait for converting a JS `Value` into a Rust type.
///
/// Used by `RestArgs<T>` to automatically convert each variadic argument.
/// Implement this trait for custom types to support them in `RestArgs<MyType>`.
pub trait FromJSValue: Sized {
    /// Converts a JS value to this Rust type, or returns `Err(())` on failure.
    fn from_js_value(scope: &js::gc::scope::Scope<'_>, val: Value) -> Result<Self, ()>;
}

impl FromJSValue for Value {
    fn from_js_value(_scope: &js::gc::scope::Scope<'_>, val: Value) -> Result<Self, ()> {
        Ok(val)
    }
}

impl FromJSValue for f64 {
    fn from_js_value(_scope: &js::gc::scope::Scope<'_>, val: Value) -> Result<Self, ()> {
        if val.is_double() {
            Ok(val.to_double())
        } else if val.is_int32() {
            Ok(val.to_int32() as f64)
        } else {
            Err(())
        }
    }
}

impl FromJSValue for i32 {
    fn from_js_value(_scope: &js::gc::scope::Scope<'_>, val: Value) -> Result<Self, ()> {
        if val.is_int32() {
            Ok(val.to_int32())
        } else if val.is_double() {
            Ok(val.to_double() as i32)
        } else {
            Err(())
        }
    }
}

impl FromJSValue for bool {
    fn from_js_value(_scope: &js::gc::scope::Scope<'_>, val: Value) -> Result<Self, ()> {
        Ok(val.to_boolean())
    }
}

impl FromJSValue for String {
    fn from_js_value(scope: &js::gc::scope::Scope<'_>, val: Value) -> Result<Self, ()> {
        if !val.is_string() {
            return Err(());
        }
        let str_ptr = ptr::NonNull::new(val.to_string()).ok_or(())?;
        let str_handle = scope.root_string(str_ptr);
        js::string::to_utf8(scope, str_handle).map_err(|_| ())
    }
}

// ============================================================================
// Inheritance traits
// ============================================================================

/// Trait for classes that extend a parent class.
///
/// Implementing this trait enables JavaScript prototype chain inheritance:
/// the child's prototype will have its `__proto__` set to the parent's prototype
/// during class registration.
///
/// The child struct must embed the parent as a field named `parent`.
///
/// # Usage
///
/// ```rust,ignore
/// #[jsclass(extends = Animal)]
/// struct Dog {
///     parent: Animal,
///     breed: String,
/// }
/// ```
pub trait HasParent: ClassDef {
    type Parent: ClassDef;
    fn as_parent(&self) -> &Self::Parent;
    fn as_parent_mut(&mut self) -> &mut Self::Parent;
}

/// Marker trait indicating that a class derives from another class.
///
/// Every class trivially derives from itself (reflexive), and extending
/// a parent class creates a direct derivation relationship.
pub trait DerivedFrom<T: ClassDef>: ClassDef {}

// ============================================================================
// StackNewtype — trait for generated stack newtypes
// ============================================================================

/// Trait implemented by all generated stack newtype wrappers (e.g. `Dog<'s>`).
///
/// Enables type-checked [`cast`](StackNewtype::cast) between stack newtypes
/// without needing a scope reference, since the underlying handle is already
/// rooted.
pub trait StackNewtype<'s>: Sized + Copy {
    /// The inner `ClassDef` data type (e.g. `__DogInner`).
    type Inner: ClassDef;

    /// Construct from a handle without checking the type tag.
    ///
    /// # Safety
    ///
    /// The handle must point to a JS object backed by `Self::Inner`
    /// (or a subclass).
    unsafe fn from_handle_unchecked(h: js::native::GCHandle<'s, *mut JSObject>) -> Self;

    /// Get the underlying rooted object handle.
    fn js_handle(self) -> js::native::GCHandle<'s, *mut JSObject>;

    /// Type-checked cast to another stack newtype.
    ///
    /// Returns `Some(T)` if the underlying JS object is an instance of `T`
    /// (or a subclass), `None` otherwise. Does not require a scope because
    /// the handle is already rooted.
    fn cast<T: StackNewtype<'s>>(self) -> Option<T> {
        let ptr = self.js_handle().get();
        let concrete_tag = unsafe { js::object::get_object_class(ptr) } as usize;
        let target_tag = class_tag::<T::Inner>();
        if !is_derived_from_type(concrete_tag, target_tag) {
            return None;
        }
        Some(unsafe { T::from_handle_unchecked(self.js_handle()) })
    }
}

// ============================================================================
// Private data storage
// ============================================================================

const PRIVATE_DATA_SLOT: u32 = 0;

/// Minimum number of reserved slots required for a class instance.
/// All ClassDef instances use at least PRIVATE_DATA_SLOT (0) for private Rust data.
///
/// Public for use by generated `ClassDef::CLASS` implementations.
#[doc(hidden)]
pub const MIN_CLASS_RESERVED_SLOTS: u32 = PRIVATE_DATA_SLOT + 1;

/// Store a Rust value in a JS object's reserved slot 0.
///
/// # Safety
///
/// - `obj` must be a valid JS object with at least 1 reserved slot.
/// - The object's class must have a finalize callback that calls [`drop_private`]
///   with the same type `T`.
pub unsafe fn set_private<T: 'static>(obj: *mut JSObject, data: T) {
    let boxed = Box::new(data);
    let ptr = Box::into_raw(boxed);
    let val = unsafe { value::from_private(ptr as *const c_void) };
    unsafe { js::object::set_reserved_slot(obj, PRIVATE_DATA_SLOT, &val) };
}

/// Get the `JSClass` pointer for a `ClassDef` type, cast to `usize`.
///
/// Each `ClassDef` type has a unique `static JSClass` (generated by the proc
/// macro) whose address serves as the type tag.
///
/// This is public for use by generated code (stack newtype `Is` checks).
#[inline]
pub fn class_tag<T: ClassDef>() -> usize {
    T::class() as *const JSClass as usize
}

/// Read the type tag from a JS object by inspecting its `JSClass` pointer.
///
/// # Safety
///
/// - `obj` must be a valid, non-null JS object pointer.
pub unsafe fn get_class_tag(obj: *mut JSObject) -> usize {
    js::object::get_object_class(obj) as usize
}

/// Retrieve a reference to the Rust data stored in a JS object's reserved slot 0.
///
/// # Safety
///
/// - `obj` must be a valid JS object with private data of type `T` stored via [`set_private`].
/// - The returned reference is only valid as long as the JS object is alive and
///   no mutable reference is taken simultaneously.
pub unsafe fn get_private<'a, T: 'static>(obj: *mut JSObject) -> Option<&'a T> {
    let val = unsafe { js::object::get_reserved_slot(obj, PRIVATE_DATA_SLOT) };
    if val.is_undefined() {
        return None;
    }
    let ptr = val.to_private() as *const T;
    if ptr.is_null() {
        return None;
    }
    Some(&*ptr)
}

/// Retrieve a mutable reference to the Rust data stored in a JS object's reserved slot 0.
///
/// # Safety
///
/// - `obj` must be a valid JS object with private data of type `T` stored via [`set_private`].
/// - No other references to the data may exist simultaneously.
pub unsafe fn get_private_mut<'a, T: 'static>(obj: *mut JSObject) -> Option<&'a mut T> {
    let val = unsafe { js::object::get_reserved_slot(obj, PRIVATE_DATA_SLOT) };
    if val.is_undefined() {
        return None;
    }
    let ptr = val.to_private() as *mut T;
    if ptr.is_null() {
        return None;
    }
    Some(&mut *ptr)
}

/// Drop the Rust data stored in a JS object's reserved slot 0.
///
/// This should be called from the class's `finalize` callback.
///
/// # Safety
///
/// - `obj` must be a valid JS object with private data of type `T` stored via [`set_private`].
/// - Must only be called once (typically from the GC finalize callback).
pub unsafe fn drop_private<T: 'static>(obj: *mut JSObject) {
    let val = unsafe { js::object::get_reserved_slot(obj, PRIVATE_DATA_SLOT) };
    if val.is_undefined() {
        return;
    }
    let ptr = val.to_private() as *mut T;
    if !ptr.is_null() {
        let _ = Box::from_raw(ptr);
        let undef = value::undefined();
        js::object::set_reserved_slot(obj, PRIVATE_DATA_SLOT, &undef);
    }
}

// ============================================================================
// HeapRef<T> — traced reference to another JS-backed object
// ============================================================================

/// A traced reference to another JavaScript object backed by a `ClassDef` struct.
///
/// This is analogous to Servo's `Dom<T>` — it stores a `Heap<*mut JSObject>`
/// that is properly traced by the garbage collector, ensuring the referenced
/// object is not collected while this reference exists.
///
/// # Usage
///
/// ```rust,ignore
/// #[jsclass]
/// struct MyClass {
///     other: HeapRef<MyOtherClass>,
/// }
/// ```
#[js::must_root]
pub struct HeapRef<T: ClassDef> {
    heap: Heap<*mut JSObject>,
    _phantom: PhantomData<T>,
}

// Crown: the implementation of HeapRef<T> is allowed to return unrooted
// instances, since HeapRef<T> itself must be rooted, so using the returned
// values incorrectly would make crown fail.
#[js::allow_unrooted]
impl<T: ClassDef> HeapRef<T> {
    /// Create a new `HeapRef` from an `Object`.
    ///
    /// **For use by generated code only.** `HeapRef` must be stored inside a
    /// `Traceable` struct — never on the stack. Use the stack newtype
    /// (e.g. `Object<'s>`) for local variables.
    ///
    /// # Safety
    ///
    /// `obj` must have private data of type `T`.
    #[doc(hidden)]
    pub unsafe fn from_object<'s>(obj: Object<'s>) -> Self {
        Self::from_raw(obj.handle().get())
    }

    /// Create a new `HeapRef` from a raw JS object pointer.
    ///
    /// **For use by generated code only.** `HeapRef` must be stored inside a
    /// `Traceable` struct — never on the stack. Use the stack newtype
    /// (e.g. `Object<'s>`) for local variables.
    ///
    /// # Safety
    ///
    /// `obj` must be a valid, non-null JS object pointer with private data of type `T`.
    #[doc(hidden)]
    pub unsafe fn from_raw(obj: *mut JSObject) -> Self {
        let heap = Heap::default();
        heap.set(obj);
        HeapRef {
            heap,
            _phantom: PhantomData,
        }
    }

    /// Get a reference to the Rust data in the referenced JS object.
    ///
    /// Returns `None` if the underlying JS object has been collected or
    /// doesn't have the expected private data.
    ///
    /// # Safety
    ///
    /// The JS object must still be alive (guaranteed if this `HeapRef` is
    /// embedded in an object that traces it).
    pub unsafe fn get(&self) -> Option<&T> {
        let obj = self.heap.get();
        if obj.is_null() {
            return None;
        }
        get_private_or_ancestor::<T>(obj)
    }

    pub fn handle<'s>(&self, scope: &'s js::gc::scope::Scope<'_>) -> Option<Object<'s>> {
        let obj = self.heap.get();
        if obj.is_null() {
            return None;
        }
        Object::from_raw(scope, self.get_jsobject())
    }

    /// Get a mutable reference to the Rust data in the referenced JS object.
    ///
    /// # Safety
    ///
    /// Same as [`get`](HeapRef::get), plus no other references to the data may exist.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get_mut(&self) -> Option<&mut T> {
        let obj = self.heap.get();
        if obj.is_null() {
            return None;
        }
        get_private_or_ancestor_mut::<T>(obj)
    }

    /// Get the raw JS object pointer.
    pub fn get_jsobject(&self) -> *mut JSObject {
        self.heap.get()
    }

    /// Upcast this reference to a parent class type.
    ///
    /// This wraps the same JS object in a `HeapRef<P>`. The parent data
    /// is accessed through the inheritance chain when `get()` is called.
    pub fn upcast<P: ClassDef>(&self) -> HeapRef<P>
    where
        T: DerivedFrom<P>,
    {
        unsafe { HeapRef::from_raw(self.heap.get()) }
    }

    /// Try to downcast this reference to a more specific class type.
    ///
    /// Returns `Some` if the underlying JS object's concrete type is `C`
    /// or a subclass of `C`. Returns `None` otherwise.
    pub fn downcast<C: ClassDef>(&self) -> Option<HeapRef<C>> {
        let obj = self.heap.get();
        if obj.is_null() {
            return None;
        }
        let concrete_tag = unsafe { get_class_tag(obj) };
        let target_tag = class_tag::<C>();
        if is_derived_from_type(concrete_tag, target_tag) {
            Some(unsafe { HeapRef::from_raw(obj) })
        } else {
            None
        }
    }
}

#[js::allow_unrooted_interior]
unsafe impl<T: ClassDef> Trace for HeapRef<T> {
    #[inline]
    unsafe fn trace(&self, trc: *mut JSTracer) {
        self.heap.trace(trc);
    }
}

#[js::allow_unrooted_interior]
impl<T: ClassDef> FromJSValConvertible for HeapRef<T> {
    type Config = ();

    unsafe fn from_jsval(
        cx: *mut RawJSContext,
        val: js::native::Handle<Value>,
        _option: (),
    ) -> Result<ConversionResult<Self>, ()> {
        if !val.get().is_object() {
            return Ok(ConversionResult::Failure(c"Expected an object".into()));
        }
        let obj = val.get().to_object();
        // Verify the object has the expected private data (inheritance-aware)
        match get_private_or_ancestor::<T>(obj) {
            Some(_) => Ok(ConversionResult::Success(HeapRef::from_raw(obj))),
            None => {
                let msg = CString::new(format!("Object is not an instance of {}", T::NAME))
                    .unwrap_or_else(|_| CString::new("Object is not the expected class").unwrap());
                let mut js_cx = JSContext::from_ptr(NonNull::new_unchecked(cx));
                js::error::throw_type_error(&mut js_cx, &msg);
                Err(())
            }
        }
    }
}

// ============================================================================
// Per-global class prototype registry
// ============================================================================

/// Reserved slot index for the per-global class registry, placed right after
/// SpiderMonkey's own global slots.
const CLASS_REGISTRY_SLOT: u32 = JSCLASS_GLOBAL_SLOT_COUNT;

/// Per-global map from `TypeId` to class prototype.
///
/// Stored as private data in [`CLASS_REGISTRY_SLOT`] of the global object.
#[js::allow_unrooted_interior]
struct ClassRegistry {
    map: HashMap<TypeId, Box<Heap<*mut JSObject>>>,
}

impl ClassRegistry {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    fn register(&mut self, type_id: TypeId, proto: *mut JSObject) {
        let entry = self
            .map
            .entry(type_id)
            .or_insert_with(|| Heap::boxed(ptr::null_mut()));
        entry.set(proto);
    }

    fn get(&self, type_id: TypeId) -> Option<*mut JSObject> {
        self.map.get(&type_id).map(|h| h.get())
    }

    /// Trace all prototype heap values so moving GC can update them.
    #[js::allow_unrooted]
    unsafe fn trace(&self, trc: *mut JSTracer) {
        for heap in self.map.values() {
            heap.trace(trc);
        }
    }
}

/// Read the `ClassRegistry` pointer from a global object's reserved slot.
///
/// Returns `None` if the slot is unset (e.g. on a non-StarlingMonkey global).
unsafe fn get_class_registry(global: *mut JSObject) -> Option<&'static ClassRegistry> {
    let val = js::object::get_reserved_slot(global, CLASS_REGISTRY_SLOT);
    if val.is_undefined() {
        return None;
    }
    let ptr = val.to_private() as *const ClassRegistry;
    if ptr.is_null() {
        return None;
    }
    Some(&*ptr)
}

/// Read the `ClassRegistry` pointer from a global, creating one if absent.
unsafe fn get_or_init_class_registry(global: *mut JSObject) -> &'static mut ClassRegistry {
    let val = js::object::get_reserved_slot(global, CLASS_REGISTRY_SLOT);
    if !val.is_undefined() {
        let ptr = val.to_private() as *mut ClassRegistry;
        if !ptr.is_null() {
            return &mut *ptr;
        }
    }
    let registry = Box::into_raw(Box::new(ClassRegistry::new()));
    let pv = value::from_private(registry as *const c_void);
    js::object::set_reserved_slot(global, CLASS_REGISTRY_SLOT, &pv);
    &mut *registry
}

fn register_prototype<T: 'static>(global: Object, proto: *mut JSObject) {
    let registry = unsafe { get_or_init_class_registry(global.as_raw()) };
    registry.register(TypeId::of::<T>(), proto);
}

fn get_prototype<T: 'static>(global: Object) -> Option<*mut JSObject> {
    let registry = unsafe { get_class_registry(global.as_raw())? };
    registry.get(TypeId::of::<T>())
}

// ============================================================================
// Custom global class
// ============================================================================

/// Starling's global class, extending `SIMPLE_GLOBAL_CLASS` with one extra
/// reserved slot for the per-global [`ClassRegistry`].
///
/// The trace hook is `JS_GlobalObjectTraceHook` (required by SpiderMonkey).
/// Registry entries are traced by the `Runtime`'s GC roots tracer instead.
/// The finalize hook drops the registry.
pub static STARLING_GLOBAL_CLASS: JSClass = JSClass {
    name: c"global".as_ptr(),
    flags: JSCLASS_IS_GLOBAL
        | JSCLASS_FOREGROUND_FINALIZE
        | (((JSCLASS_GLOBAL_SLOT_COUNT + 1) & JSCLASS_RESERVED_SLOTS_MASK)
            << JSCLASS_RESERVED_SLOTS_SHIFT),
    cOps: &STARLING_GLOBAL_OPS as *const JSClassOps,
    spec: ptr::null(),
    ext: ptr::null(),
    oOps: ptr::null(),
};

static STARLING_GLOBAL_OPS: JSClassOps = JSClassOps {
    addProperty: None,
    delProperty: None,
    enumerate: Some(JS_EnumerateStandardClasses),
    newEnumerate: None,
    resolve: Some(JS_ResolveStandardClass),
    mayResolve: Some(JS_MayResolveStandardClass),
    finalize: Some(finalize_starling_global),
    call: None,
    construct: None,
    trace: Some(JS_GlobalObjectTraceHook),
};

/// Destructor for Starling's global class — drops the class registry.
unsafe extern "C" fn finalize_starling_global(_gc: *mut GCContext, obj: *mut JSObject) {
    let val = js::object::get_reserved_slot(obj, CLASS_REGISTRY_SLOT);
    if !val.is_undefined() {
        let ptr = val.to_private() as *mut ClassRegistry;
        if !ptr.is_null() {
            drop(Box::from_raw(ptr));
            // Clear the slot so we don't double-free.
            let undef = value::undefined();
            js::object::set_reserved_slot(obj, CLASS_REGISTRY_SLOT, &undef);
        }
    }
}

/// Trace the class registry stored in a global object's reserved slot.
///
/// Called by the `Runtime`'s GC roots tracer to keep registered prototype
/// `Heap` pointers up-to-date across moving GC.
///
/// # Safety
///
/// `trc` must be a valid tracer. `global` must be a live global object
/// that was created with [`STARLING_GLOBAL_CLASS`].
pub unsafe fn trace_class_registry_for_global(trc: *mut JSTracer, global: *mut JSObject) {
    if let Some(registry) = get_class_registry(global) {
        registry.trace(trc);
    }
}

/// Get the registered prototype for a class type. Public for generated code.
#[doc(hidden)]
pub fn get_prototype_for<T: 'static>(scope: &js::gc::scope::Scope<'_>) -> Option<*mut JSObject> {
    get_prototype::<T>(scope.global())
}

// ============================================================================
// Inheritance registry
// ============================================================================

/// Information about a class's direct parent for inheritance support.
struct InheritanceInfo {
    parent_tag: usize,
    /// Precomputed set of all ancestor type tags (parent, grandparent, ...).
    ancestors: HashSet<usize>,
    accessor: unsafe fn(*const c_void) -> *const c_void,
    accessor_mut: unsafe fn(*mut c_void) -> *mut c_void,
}

// Registry mapping child type tag → parent info.
// Thread-local because the SpiderMonkey runtime is single-threaded.
thread_local! {
    static INHERITANCE_REGISTRY: RefCell<HashMap<usize, InheritanceInfo>> = RefCell::new(HashMap::new());
}

/// Register the parent relationship for a child class.
///
/// Called from generated `__ParentPrototypeRegistrar` code.
#[doc(hidden)]
pub fn register_parent_info<C: HasParent>() {
    unsafe fn immutable_accessor<T: HasParent>(ptr: *const c_void) -> *const c_void {
        let concrete = &*(ptr as *const T);
        T::as_parent(concrete) as *const T::Parent as *const c_void
    }

    unsafe fn mutable_accessor<T: HasParent>(ptr: *mut c_void) -> *mut c_void {
        let concrete = &mut *(ptr as *mut T);
        T::as_parent_mut(concrete) as *mut T::Parent as *mut c_void
    }

    let child_tag = class_tag::<C>();
    let parent_tag = class_tag::<C::Parent>();

    INHERITANCE_REGISTRY.with(|reg| {
        let mut map = reg.borrow_mut();

        // Build the ancestor set: parent + parent's ancestors (if any).
        let mut ancestors = HashSet::new();
        ancestors.insert(parent_tag);
        if let Some(parent_info) = map.get(&parent_tag) {
            ancestors.extend(&parent_info.ancestors);
        }

        map.insert(
            child_tag,
            InheritanceInfo {
                parent_tag,
                ancestors,
                accessor: immutable_accessor::<C>,
                accessor_mut: mutable_accessor::<C>,
            },
        );
    });
}

/// Get the raw private data pointer from slot 0 without type interpretation.
unsafe fn get_raw_private(obj: *mut JSObject) -> Option<*const c_void> {
    let val = unsafe { js::object::get_reserved_slot(obj, PRIVATE_DATA_SLOT) };
    if val.is_undefined() {
        return None;
    }
    let ptr = val.to_private();
    if ptr.is_null() {
        return None;
    }
    Some(ptr)
}

/// Check if a concrete type (by tag) derives from a target type (by tag).
pub fn is_derived_from_type(concrete_tag: usize, target_tag: usize) -> bool {
    if concrete_tag == target_tag {
        return true;
    }
    INHERITANCE_REGISTRY.with(|reg| {
        let map = reg.borrow();
        map.get(&concrete_tag)
            .is_some_and(|info| info.ancestors.contains(&target_tag))
    })
}

/// Inheritance-aware immutable private data access.
///
/// If the object's concrete type matches T, returns a direct reference.
/// If the concrete type derives from T (via HasParent chain), walks the
/// parent accessor chain to find the T reference within the concrete data.
///
/// # Safety
///
/// - `obj` must be a valid JS object with private data stored via [`set_private`].
/// - The returned reference is only valid as long as the JS object is alive.
pub unsafe fn get_private_or_ancestor<'a, T: ClassDef>(obj: *mut JSObject) -> Option<&'a T> {
    // Guard: if the object doesn't have enough reserved slots, it can't be
    // one of our class instances (which always have at least PRIVATE_DATA_SLOT).
    if js::object::reserved_slot_count(obj) < MIN_CLASS_RESERVED_SLOTS {
        return None;
    }

    let concrete_tag = get_class_tag(obj);
    let target_tag = class_tag::<T>();

    if concrete_tag == target_tag {
        // Direct match
        return get_private::<T>(obj);
    }

    // Walk the parent chain
    let data_ptr = get_raw_private(obj)?;
    INHERITANCE_REGISTRY.with(|reg| {
        let map = reg.borrow();
        let mut current_tag = concrete_tag;
        let mut current_ptr = data_ptr;

        loop {
            let info = map.get(&current_tag)?;
            current_ptr = (info.accessor)(current_ptr);
            current_tag = info.parent_tag;
            if current_tag == target_tag {
                return Some(&*(current_ptr as *const T));
            }
        }
    })
}

/// Inheritance-aware mutable private data access.
///
/// # Safety
///
/// - `obj` must be a valid JS object with private data stored via [`set_private`].
/// - No other references to the data may exist simultaneously.
pub unsafe fn get_private_or_ancestor_mut<'a, T: ClassDef>(
    obj: *mut JSObject,
) -> Option<&'a mut T> {
    // Guard: if the object doesn't have enough reserved slots, it can't be
    // one of our class instances (which always have at least PRIVATE_DATA_SLOT).
    if js::object::reserved_slot_count(obj) < MIN_CLASS_RESERVED_SLOTS {
        return None;
    }

    let concrete_tag = get_class_tag(obj);
    let target_tag = class_tag::<T>();

    if concrete_tag == target_tag {
        return get_private_mut::<T>(obj);
    }

    let data_ptr = get_raw_private(obj)? as *mut c_void;
    INHERITANCE_REGISTRY.with(|reg| {
        let map = reg.borrow();
        let mut current_tag = concrete_tag;
        let mut current_ptr = data_ptr;

        loop {
            let info = map.get(&current_tag)?;
            current_ptr = (info.accessor_mut)(current_ptr);
            current_tag = info.parent_tag;
            if current_tag == target_tag {
                return Some(&mut *(current_ptr as *mut T));
            }
        }
    })
}

// ============================================================================
// ClassDef trait
// ============================================================================

/// Trait for Rust types that can be exposed as JavaScript classes.
///
/// Implement this trait to define a JavaScript class backed by a Rust struct.
/// The struct's data will be stored in a reserved slot on the JS object and
/// automatically freed when the object is garbage collected.
///
/// # Required methods
///
/// - [`NAME`](ClassDef::NAME): The JavaScript class name
/// - [`constructor`](ClassDef::constructor): Creates a new instance from JS constructor arguments
///
/// # Optional methods
///
/// - [`register_class_methods`](ClassDef::register_class_methods): Define prototype methods
/// - [`register_static_methods`](ClassDef::register_static_methods): Define static methods
pub trait ClassDef: Sized + Trace + 'static {
    /// The name of the class as it appears in JavaScript.
    const NAME: &'static str;

    /// Return the `JSClass` descriptor for this type.
    ///
    /// Each `ClassDef` type gets a unique `static JSClass` generated by the
    /// `#[jsclass]` / `#[webidl_interface]` proc macro. The address of this
    /// static serves as a stable type tag — no runtime allocation, Mutex,
    /// or HashMap required.
    ///
    /// Implementors should return a reference to a module-level `static` so
    /// the address is guaranteed to be unique and stable.
    fn class() -> &'static JSClass;

    /// Total number of reserved slots on the JS object.  The system always
    /// enforces a minimum of [`MIN_CLASS_RESERVED_SLOTS`] (slot 0 holds the
    /// private Rust data).  Override to request additional user-defined slots
    /// beyond this one.
    const RESERVED_SLOTS: u32 = MIN_CLASS_RESERVED_SLOTS;

    /// Construct a new instance from JavaScript constructor arguments.
    ///
    /// Return `Ok(Self)` to create the object, or `Err(())` with a pending
    /// JS exception to signal an error.
    fn constructor(scope: &js::gc::scope::Scope<'_>, args: &CallArgs) -> Result<Self, ()>;

    /// Register methods on the class prototype.
    ///
    /// Override this to add methods and properties to your class.
    /// The default implementation adds no methods.
    fn register_class_methods(builder: ClassBuilder<Self>) -> ClassBuilder<Self> {
        builder
    }

    /// Register static methods on the constructor.
    ///
    /// Override this to add static methods.
    fn register_static_methods(builder: ClassBuilder<Self>) -> ClassBuilder<Self> {
        builder
    }

    /// Called during GC finalization, before the Rust data is dropped.
    ///
    /// Use `#[destructor]` in `#[jsmethods]` to define this.
    /// The default implementation does nothing.
    fn destructor(&mut self) {}

    /// Return the prototype of the parent class, or null if no parent.
    ///
    /// Override by using `#[jsclass(extends = ParentType)]`.
    fn parent_prototype(_scope: &js::gc::scope::Scope<'_>) -> *mut JSObject {
        ptr::null_mut()
    }

    /// Register inheritance information (parent accessor functions).
    ///
    /// Override by using `#[jsclass(extends = ParentType)]`.
    fn register_inheritance() {}

    /// Ensure the parent class is registered on `global` before `Self` is.
    ///
    /// Called automatically by [`register_class`] — you never need to call
    /// this directly.  Override by using `#[jsclass(extends = ParentType)]`.
    fn ensure_parent_registered(_scope: &js::gc::scope::Scope<'_>, _global: Object<'_>) {}

    /// The `Symbol.toStringTag` value for this class (empty = none).
    ///
    /// When non-empty, `register_class` defines `Symbol.toStringTag` on the
    /// prototype with this value (non-writable, non-enumerable, configurable).
    ///
    /// Override by using `#[jsclass(to_string_tag = "MyClass")]`.
    const TO_STRING_TAG: &'static str = "";

    /// Register integer constants on the constructor.
    ///
    /// Constants are defined as read-only, enumerable, non-configurable
    /// data properties (`JSPROP_READONLY | JSPROP_ENUMERATE | JSPROP_PERMANENT`).
    ///
    /// Use `pub const` items in `#[jsmethods]` to populate this automatically.
    fn register_constants(builder: ClassBuilder<Self>) -> ClassBuilder<Self> {
        builder
    }

    /// Whether this class has `[[ErrorData]]` internal slot semantics.
    ///
    /// When `true`, `generic_constructor` automatically captures the current
    /// call stack (via a temporary `Error` object) and sets it as the `stack`
    /// property on each new instance.
    ///
    /// Set automatically by `#[jsclass(js_proto = "Error")]`.
    const HAS_ERROR_DATA: bool = false;

    /// Whether constants should also be installed on the prototype.
    ///
    /// When `true`, constants from [`register_constants`](ClassDef::register_constants)
    /// are defined on both the constructor AND the prototype (per WebIDL §3.7.3).
    /// When `false` (the default, used by `#[jsclass]`), constants are only on
    /// the constructor.
    ///
    /// Set automatically by `#[webidl_interface]`.
    const CONSTANTS_ON_PROTOTYPE: bool = false;
}

// ============================================================================
// ClassBuilder
// ============================================================================

/// A builder for defining JavaScript class methods and properties.
///
/// Use this in your [`ClassDef::register_class_methods`] implementation to
/// add methods to the class prototype.
pub struct ClassBuilder<T: ClassDef> {
    methods: Vec<JSFunctionSpec>,
    properties: Vec<JSPropertySpec>,
    constants: Vec<(&'static std::ffi::CStr, i32)>,
    _phantom: PhantomData<T>,
}

impl<T: ClassDef> ClassBuilder<T> {
    fn new() -> Self {
        Self {
            methods: Vec::new(),
            properties: Vec::new(),
            constants: Vec::new(),
            _phantom: PhantomData,
        }
    }

    /// Add a read-only, enumerable, non-configurable integer constant.
    ///
    /// Constants are installed on the constructor object after class
    /// initialization. `name` must be a null-terminated C string.
    pub fn constant(mut self, name: &'static std::ffi::CStr, value: i32) -> Self {
        self.constants.push((name, value));
        self
    }

    /// Add a method to the class prototype.
    ///
    /// `name` must be a null-terminated C string (use `c"name"`).
    /// `nargs` is the number of expected arguments.
    /// `func` is a `JSNative` callback — use the [`js_method!`] or
    /// [`js_method_simple!`] macros to generate one easily.
    pub fn method(mut self, name: &'static std::ffi::CStr, nargs: u32, func: JSNative) -> Self {
        self.methods.push(JSFunctionSpec {
            name: js::class_spec::JSPropertySpec_Name {
                string_: name.as_ptr(),
            },
            call: js::class_spec::JSNativeWrapper {
                op: func,
                info: ptr::null(),
            },
            nargs: nargs as u16,
            flags: 0,
            selfHostedName: ptr::null(),
        });
        self
    }

    /// Add a property accessor (getter and/or setter) to the class prototype.
    ///
    /// `name` must be a null-terminated C string (use `c"name"`).
    /// `getter` is the getter callback (`JSNative` is `Option<fn>`: pass `None` for write-only).
    /// `setter` is the setter callback (`JSNative` is `Option<fn>`: pass `None` for read-only).
    pub fn property(
        mut self,
        name: &'static std::ffi::CStr,
        getter: JSNative,
        setter: JSNative,
    ) -> Self {
        self.properties.push(JSPropertySpec {
            name: js::class_spec::JSPropertySpec_Name {
                string_: name.as_ptr(),
            },
            attributes_: js::class_spec::JSPROP_ENUMERATE,
            kind_: js::class_spec::JSPropertySpec_Kind::NativeAccessor,
            u: js::class_spec::JSPropertySpec_AccessorsOrValue {
                accessors: js::class_spec::JSPropertySpec_AccessorsOrValue_Accessors {
                    getter: js::class_spec::JSPropertySpec_Accessor {
                        native: js::class_spec::JSNativeWrapper {
                            op: getter,
                            info: ptr::null(),
                        },
                    },
                    setter: js::class_spec::JSPropertySpec_Accessor {
                        native: js::class_spec::JSNativeWrapper {
                            op: setter,
                            info: ptr::null(),
                        },
                    },
                },
            },
        });
        self
    }

    /// Finalize the method and property arrays with the required terminators.
    fn finalize(
        mut self,
    ) -> (
        Vec<JSFunctionSpec>,
        Vec<JSPropertySpec>,
        Vec<(&'static std::ffi::CStr, i32)>,
    ) {
        // Add sentinel (zeroed) entry
        self.methods.push(unsafe { std::mem::zeroed() });
        if !self.properties.is_empty() {
            self.properties.push(unsafe { std::mem::zeroed() });
        }
        (self.methods, self.properties, self.constants)
    }
}

// ============================================================================
// Class registration
// ============================================================================

/// Register a class on the global object, making it available as a constructor.
///
/// This creates the class's JSClass, constructor, prototype, and methods,
/// and stores the prototype for later use with [`create_instance`].
///
/// # Safety
///
/// - `global` must be a global object.
/// - Must be called within an appropriate realm/compartment.
///
/// # Returns
///
/// The prototype object for the newly registered class.
///
/// # Panics
///
/// Panics if SpiderMonkey fails to create the class or prototype object.
pub unsafe fn register_class<'s, T: ClassDef>(
    scope: &'s js::gc::scope::Scope<'s>,
    global: Object<'s>,
) -> Object<'s> {
    // Idempotency check: if this class is already registered on this global,
    // return the existing prototype without re-registering.
    if let Some(proto) = get_prototype::<T>(scope.global()) {
        return Object::from_raw(scope, proto).expect("registered prototype is non-null");
    }

    // Ensure the parent class is registered before we register ourselves.
    T::ensure_parent_registered(scope, global);

    // Build method and property tables
    let builder = ClassBuilder::<T>::new();
    let builder = T::register_class_methods(builder);
    let (methods, properties, _proto_constants) = builder.finalize();

    let static_builder = ClassBuilder::<T>::new();
    let static_builder = T::register_static_methods(static_builder);
    let (static_methods, static_properties, _static_constants) = static_builder.finalize();

    // Build constructor constants
    let const_builder = ClassBuilder::<T>::new();
    let const_builder = T::register_constants(const_builder);
    let (_cm, _cp, constants) = const_builder.finalize();

    // Leak the method/property arrays so they live for the duration of the program.
    // SpiderMonkey requires these arrays to be valid for the lifetime of the class.
    let methods_ptr = Box::leak(methods.into_boxed_slice()).as_ptr();
    let static_methods_ptr = Box::leak(static_methods.into_boxed_slice()).as_ptr();
    let properties_ptr = if properties.len() > 1 {
        Box::leak(properties.into_boxed_slice()).as_ptr()
    } else {
        ptr::null()
    };
    let static_properties_ptr = if static_properties.len() > 1 {
        Box::leak(static_properties.into_boxed_slice()).as_ptr()
    } else {
        ptr::null()
    };

    let class: &'static JSClass = T::class();

    // Register inheritance information (parent accessor functions)
    T::register_inheritance();
    let parent_proto = T::parent_prototype(scope);
    js::rooted!(in(unsafe { scope.raw_cx_no_gc() }) let parent_proto_rooted = parent_proto);

    // Use init_class to set up constructor + prototype.
    // The class name comes from T::class().name (a static C string pointer).
    let proto = match js::class::init_class(
        scope,
        global.handle(),
        class,
        parent_proto_rooted.handle(),
        class.name,
        Some(generic_constructor::<T>),
        0,
        properties_ptr,
        methods_ptr,
        static_properties_ptr,
        static_methods_ptr,
    ) {
        Ok(handle) => handle.get(),
        Err(_) => ptr::null_mut(),
    };

    if !proto.is_null() {
        register_prototype::<T>(global, proto);

        // Define Symbol.toStringTag if the class specifies one.
        if !T::TO_STRING_TAG.is_empty() {
            let proto_handle = scope.root_object(NonNull::new(proto).unwrap());
            define_to_string_tag(scope, proto_handle, T::TO_STRING_TAG);
        }

        // Install constants on the constructor (and optionally the prototype).
        if !constants.is_empty() {
            let name = CString::new(T::NAME).expect("Class name must not contain null bytes");
            let ctor_val = global
                .get_property(scope, &name)
                .expect("constructor not found on global after init_class");
            let ctor_raw = ctor_val.to_object_or_null();
            let ctor_handle =
                scope.root_object(NonNull::new(ctor_raw).expect("constructor is null"));
            let attrs = (js::class_spec::JSPROP_READONLY
                | js::class_spec::JSPROP_ENUMERATE
                | js::class_spec::JSPROP_PERMANENT) as std::ffi::c_uint;
            for &(const_name, value) in &constants {
                js::object::define_property(scope, ctor_handle, const_name, &value, attrs)
                    .expect("failed to define constant on constructor");
            }

            // WebIDL §3.7.3: constants are also defined on the prototype.
            if T::CONSTANTS_ON_PROTOTYPE {
                let proto_handle: js::native::HandleObject =
                    scope.root_object(NonNull::new(proto).unwrap());
                for &(const_name, value) in &constants {
                    js::object::define_property(scope, proto_handle, const_name, &value, attrs)
                        .expect("failed to define constant on prototype");
                }
            }
        }
    }

    Object::from_raw(scope, proto).unwrap()
}

/// Generic constructor callback for all ClassDef types.
///
/// This is the `JSNative` that gets called when `new MyClass(...)` is invoked in JS.
unsafe extern "C" fn generic_constructor<T: ClassDef>(
    cx: *mut RawJSContext,
    argc: u32,
    vp: *mut Value,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    if !args.is_constructing() {
        let mut js_cx = JSContext::from_ptr(NonNull::new_unchecked(cx));
        js::error::throw_type_error(&mut js_cx, c"Constructor must be called with 'new'");
        return false;
    }

    // Get the JSClass for this type
    let class: &'static JSClass = T::class();

    // Create the new JS object using the constructor's prototype
    let obj = js::class_spec::JS_NewObjectForConstructor(cx, class, &args);
    if obj.is_null() {
        return false;
    }

    // SAFETY: SpiderMonkey guarantees cx is valid and a realm is entered during a native call.
    let mut js_cx = JSContext::from_ptr(NonNull::new_unchecked(cx));
    let scope = js::gc::scope::RootScope::from_current_realm(&mut js_cx);

    // Call the Rust constructor
    match T::constructor(&scope, &args) {
        Ok(instance) => {
            unsafe { set_private(obj, instance) };

            // For error-data classes, capture the current stack.
            if T::HAS_ERROR_DATA {
                let obj_typed = Object::from_raw(&scope, obj).unwrap();
                unsafe { capture_stack_from_error(&scope, &obj_typed) };
            }

            args.rval().set(unsafe { value::from_object(obj) });
            true
        }
        Err(()) => false,
    }
}

/// Generic GC finalize callback that drops the Rust private data.
#[doc(hidden)]
pub unsafe extern "C" fn generic_class_finalize<T: ClassDef>(
    _gc: *mut GCContext,
    obj: *mut JSObject,
) {
    // Call the user-defined destructor before dropping
    if let Some(data) = get_private_mut::<T>(obj) {
        data.destructor();
    }
    drop_private::<T>(obj);
}

/// Generic GC trace callback that traces the Rust private data.
#[doc(hidden)]
pub unsafe extern "C" fn generic_class_trace<T: ClassDef>(trc: *mut JSTracer, obj: *mut JSObject) {
    if let Some(data) = get_private::<T>(obj) {
        data.trace(trc);
    }
}

// ============================================================================
// Argument extraction helpers
// ============================================================================

/// Extract an argument from CallArgs and convert it to the desired Rust type.
///
/// Returns `Ok(value)` on success, or `Err(())` if the argument is missing
/// or conversion fails.
///
/// # Safety
///
/// - `scope` must be in a valid realm.
/// - `args` must be from a valid JSNative call.
pub unsafe fn get_arg<T: FromJSValConvertible<Config = ()>>(
    scope: &js::gc::scope::Scope<'_>,
    args: &CallArgs,
    index: u32,
) -> Result<T, ()> {
    if index >= args.argc_ {
        unsafe { js::error::report_error_ascii(scope.cx_mut(), c"Not enough arguments") };
        return Err(());
    }
    let val = js::native::Handle::from_raw(args.get(index));
    match unsafe { T::from_jsval(scope.cx_mut().raw_cx(), val, ()) }? {
        ConversionResult::Success(v) => Ok(v),
        ConversionResult::Failure(msg) => {
            unsafe { js::error::report_error_ascii(scope.cx_mut(), &msg) };
            Err(())
        }
    }
}

/// Extract an integer argument with configurable conversion behavior.
///
/// # Safety
///
/// - `scope` must be in a valid realm.
/// - `args` must be from a valid JSNative call.
pub unsafe fn get_int_arg<T: FromJSValConvertible<Config = ConversionBehavior>>(
    scope: &js::gc::scope::Scope<'_>,
    args: &CallArgs,
    index: u32,
    behavior: ConversionBehavior,
) -> Result<T, ()> {
    if index >= args.argc_ {
        unsafe { js::error::report_error_ascii(scope.cx_mut(), c"Not enough arguments") };
        return Err(());
    }
    let val = js::native::Handle::from_raw(args.get(index));
    match unsafe { T::from_jsval(scope.cx_mut().raw_cx(), val, behavior) }? {
        ConversionResult::Success(v) => Ok(v),
        ConversionResult::Failure(msg) => {
            unsafe { js::error::report_error_ascii(scope.cx_mut(), &msg) };
            Err(())
        }
    }
}

/// Extract the `this` object's private data in a method callback.
///
/// This is used inside `JSNative` methods to get the Rust struct backing `this`.
///
/// # Safety
///
/// - The CallArgs must be from a valid JSNative call.
/// - The `this` object must have private data of type `T`.
pub unsafe fn get_this<'a, T: ClassDef>(
    scope: &js::gc::scope::Scope<'_>,
    args: &CallArgs,
) -> Result<&'a T, ()> {
    let this_val = args.thisv();
    if !this_val.is_object() {
        js::error::throw_type_error(scope.cx_mut(), c"'this' is not an object");
        return Err(());
    }
    let obj = this_val.to_object();
    match get_private_or_ancestor::<T>(obj) {
        Some(data) => Ok(data),
        None => {
            js::error::throw_type_error(
                scope.cx_mut(),
                c"'this' does not have the expected private data",
            );
            Err(())
        }
    }
}

/// Extract the `this` object's private data mutably.
///
/// # Safety
///
/// Same as [`get_this`], plus no other references to the data may exist.
pub unsafe fn get_this_mut<'a, T: ClassDef>(
    scope: &js::gc::scope::Scope<'_>,
    args: &CallArgs,
) -> Result<&'a mut T, ()> {
    let this_val = args.thisv();
    if !this_val.is_object() {
        js::error::throw_type_error(scope.cx_mut(), c"'this' is not an object");
        return Err(());
    }
    let obj = this_val.to_object();
    match get_private_or_ancestor_mut::<T>(obj) {
        Some(data) => Ok(data),
        None => {
            js::error::throw_type_error(
                scope.cx_mut(),
                c"'this' does not have the expected private data",
            );
            Err(())
        }
    }
}

/// Set the return value of a JSNative callback.
///
/// # Safety
///
/// - `cx` and `args` must be from a valid JSNative call.
pub unsafe fn set_return<T: ToJSValConvertible>(
    scope: &js::gc::scope::Scope<'_>,
    args: &CallArgs,
    value: &T,
) {
    value.to_jsval(
        scope.cx_mut().raw_cx(),
        MutableHandleValue::from_raw(args.rval()),
    );
}

/// Throw a JS TypeError with the given message string.
///
/// This is used by the `#[jsmethods]` macro to convert Rust `Err` values
/// into JS exceptions. The error message is converted to a `CString`.
///
/// # Safety
///
/// - `cx` must be a valid JSContext pointer.
pub unsafe fn throw_error(scope: &js::gc::scope::Scope<'_>, msg: &str) {
    let c_msg = CString::new(msg).unwrap_or_else(|_| CString::new("unknown error").unwrap());
    js::error::throw_type_error(scope.cx_mut(), &c_msg);
}

// ============================================================================
// ThrowException trait — typed error dispatch for proc macros
// ============================================================================

/// Trait for converting a Rust error value into a pending JavaScript exception.
///
/// The `#[jsmethods]`, `#[jsglobals]`, and `#[jsmodule]` proc macros use this
/// trait to dispatch `Err` values to the correct SpiderMonkey error API. For
/// example, returning `Err(TypeError("bad argument".into()))` throws a
/// JavaScript `TypeError`, while returning `Err(String)` throws a `TypeError`.
///
/// # Implementing
///
/// Custom error types (like `DOMExceptionError`) can implement this trait to
/// throw domain-specific exception objects.
///
/// # Safety
///
/// Implementations must set a pending exception on the context. The scope
/// guarantees that a realm is entered.
pub trait ThrowException {
    /// Set a pending JavaScript exception from this error value.
    ///
    /// After this call, a JS exception must be pending on the context.
    ///
    /// # Safety
    ///
    /// A realm must be entered on the scope's context.
    unsafe fn throw(self, scope: &js::gc::scope::Scope<'_>);
}

impl ThrowException for String {
    /// Throw a `TypeError` with this string as the message.
    unsafe fn throw(self, scope: &js::gc::scope::Scope<'_>) {
        throw_error(scope, &self);
    }
}

impl ThrowException for js::error::TypeError {
    unsafe fn throw(self, scope: &js::gc::scope::Scope<'_>) {
        js::error::TypeError::throw(&self, scope);
    }
}

impl ThrowException for js::error::RangeError {
    unsafe fn throw(self, scope: &js::gc::scope::Scope<'_>) {
        js::error::RangeError::throw(&self, scope);
    }
}

impl ThrowException for js::error::SyntaxError {
    unsafe fn throw(self, scope: &js::gc::scope::Scope<'_>) {
        js::error::SyntaxError::throw(&self, scope);
    }
}

impl ThrowException for js::error::JSError {
    /// No-op: `JSError` indicates an exception is already pending on the
    /// context, so there is nothing additional to throw.
    unsafe fn throw(self, _scope: &js::gc::scope::Scope<'_>) {}
}

// ============================================================================
// Autoref specialization types for proc macro support
// ============================================================================

/// Defines a `PhantomData`-wrapper struct with `Default` and `new()` for use
/// in autoref specialization. Each `__*Reg<T>` type has a companion trait
/// whose blanket impl on `&__*Reg<T>` provides a no-op default; `#[jsmethods]`
/// overrides it with a real impl directly on `__*Reg<T>`.
macro_rules! autoref_reg {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[doc(hidden)]
        pub struct $name<T: ClassDef>(PhantomData<T>);

        impl<T: ClassDef> Default for $name<T> {
            fn default() -> Self {
                $name(PhantomData)
            }
        }

        impl<T: ClassDef> $name<T> {
            pub fn new() -> Self {
                Self::default()
            }
        }
    };
}

autoref_reg!(
    __CtorReg,
    "Autoref specialization helper for constructor registration."
);
autoref_reg!(
    __MethodReg,
    "Autoref specialization helper for method registration."
);
autoref_reg!(
    __DtorReg,
    "Autoref specialization helper for destructor registration."
);
autoref_reg!(
    __StaticMethodReg,
    "Autoref specialization helper for static method registration."
);
autoref_reg!(
    __ConstantReg,
    "Autoref specialization helper for constant registration."
);

/// Trait for constructor registration via autoref specialization.
/// The blanket impl on `&__CtorReg<T>` panics; `#[jsmethods]` provides
/// the real impl on `__CtorReg<T>` directly.
#[doc(hidden)]
pub trait __ConstructorRegistrar<T: ClassDef> {
    fn construct(&self, scope: &js::gc::scope::Scope<'_>, args: &CallArgs) -> Result<T, ()>;
}

impl<T: ClassDef> __ConstructorRegistrar<T> for &__CtorReg<T> {
    fn construct(&self, _scope: &js::gc::scope::Scope<'_>, _args: &CallArgs) -> Result<T, ()> {
        panic!("No #[constructor] defined. Use #[jsmethods] with #[constructor] to define one.");
    }
}

/// Trait for method registration via autoref specialization.
/// The blanket impl on `&__MethodReg<T>` is a no-op; `#[jsmethods]` provides
/// the real impl on `__MethodReg<T>` directly.
#[doc(hidden)]
pub trait __MethodRegistrar<T: ClassDef> {
    fn register(&self, builder: ClassBuilder<T>) -> ClassBuilder<T>;
}

impl<T: ClassDef> __MethodRegistrar<T> for &__MethodReg<T> {
    fn register(&self, builder: ClassBuilder<T>) -> ClassBuilder<T> {
        builder
    }
}

/// Trait for destructor registration via autoref specialization.
/// The blanket impl on `&__DtorReg<T>` is a no-op; `#[jsmethods]` provides
/// the real impl on `__DtorReg<T>` directly when `#[destructor]` is used.
#[doc(hidden)]
pub trait __DestructorRegistrar<T: ClassDef> {
    fn destruct(&self, this: &mut T);
}

impl<T: ClassDef> __DestructorRegistrar<T> for &__DtorReg<T> {
    fn destruct(&self, _this: &mut T) {
        // No-op default — no #[destructor] defined
    }
}

/// Trait for static method registration via autoref specialization.
/// The blanket impl on `&__StaticMethodReg<T>` is a no-op; `#[jsmethods]` provides
/// the real impl on `__StaticMethodReg<T>` directly when `#[static_method]` is used.
#[doc(hidden)]
pub trait __StaticMethodRegistrar<T: ClassDef> {
    fn register(&self, builder: ClassBuilder<T>) -> ClassBuilder<T>;
}

impl<T: ClassDef> __StaticMethodRegistrar<T> for &__StaticMethodReg<T> {
    fn register(&self, builder: ClassBuilder<T>) -> ClassBuilder<T> {
        builder
    }
}

/// Trait for constant registration via autoref specialization.
/// The blanket impl on `&__ConstantReg<T>` is a no-op; `#[jsmethods]` provides
/// the real impl on `__ConstantReg<T>` directly when `pub const` items are present.
#[doc(hidden)]
pub trait __ConstantRegistrar<T: ClassDef> {
    fn register(&self, builder: ClassBuilder<T>) -> ClassBuilder<T>;
}

impl<T: ClassDef> __ConstantRegistrar<T> for &__ConstantReg<T> {
    fn register(&self, builder: ClassBuilder<T>) -> ClassBuilder<T> {
        builder
    }
}

/// Convenience function to create a JS object backed by a Rust value,
/// using the registered class for type T.
///
/// # Safety
///
/// - The class for `T` must have been registered via [`register_class`] first.
pub unsafe fn create_instance<'s, T: ClassDef>(
    scope: &'s js::gc::scope::Scope<'_>,
    data: T,
) -> Result<Object<'s>, JSError> {
    let global = scope.global();
    let proto = match get_prototype::<T>(global) {
        Some(p) => Object::from_raw(scope, p).unwrap(),
        None => return Err(JSError), // TODO: Actually throw an error here.
    };

    let class = T::class();
    Object::new_with_proto(scope, class, proto).inspect(|obj| {
        set_private(obj.as_raw(), data);
    })
}

// ============================================================================
// Async / Promise support
// ============================================================================

/// Callback that sets a resolved value on a `MutableHandleValue`.
type ResolveCallback = Box<dyn FnOnce(*mut RawJSContext, MutableHandleValue) -> bool>;

/// A pending promise paired with its future.
type PendingPromise = (
    RootedTraceableBox<Heap<*mut JSObject>>,
    Pin<Box<dyn Future<Output = PromiseOutcome> + 'static>>,
);

/// The outcome of an async method — either resolve with a convertible value
/// or reject with an error message.
pub enum PromiseOutcome {
    /// Resolve the promise. The boxed closure sets the return value on the
    /// provided `MutableHandleValue` and returns `true` on success.
    Resolve(ResolveCallback),
    /// Reject the promise with the given error message.
    Reject(String),
}

/// A future that resolves or rejects a JS Promise.
///
/// Use `JSPromise::new` in a `#[method]` to return a promise from an async
/// operation. The macro detects the `JSPromise` return type and generates
/// code to create a bare SpiderMonkey Promise, spawn the future, and
/// resolve/reject the promise when the future completes.
///
/// The design is async-runtime agnostic: call [`drain_promises`] with your
/// executor in your event loop to resolve/reject completed promises.
///
/// # Example
///
/// ```rust,ignore
/// #[method]
/// fn slow_greet(&self, name: String) -> JSPromise {
///     let greeting = self.prefix.clone();
///     JSPromise::new(async move {
///         // simulate async work
///         Ok(format!("{}, {}!", greeting, name))
///     })
/// }
/// ```
pub struct JSPromise {
    pub(crate) future: Pin<Box<dyn Future<Output = PromiseOutcome> + 'static>>,
}

impl JSPromise {
    /// Create a `JSPromise` from a future that returns `Result<T, E>`.
    ///
    /// - `Ok(value)` resolves the promise; `value` must implement `ToJSValConvertible`.
    /// - `Err(e)` rejects the promise with `e.to_string()` as the error message.
    pub fn new<T, E, F>(future: F) -> Self
    where
        T: ToJSValConvertible + 'static,
        E: std::fmt::Display + 'static,
        F: Future<Output = Result<T, E>> + 'static,
    {
        JSPromise {
            future: Box::pin(async move {
                match future.await {
                    Ok(value) => PromiseOutcome::Resolve(Box::new(
                        move |cx: *mut RawJSContext, rval: MutableHandleValue| unsafe {
                            value.to_jsval(cx, rval);
                            true
                        },
                    )),
                    Err(e) => PromiseOutcome::Reject(e.to_string()),
                }
            }),
        }
    }

    /// Create a `JSPromise` from a future that resolves to `()` (void).
    pub fn new_void<E, F>(future: F) -> Self
    where
        E: std::fmt::Display + 'static,
        F: Future<Output = Result<(), E>> + 'static,
    {
        JSPromise {
            future: Box::pin(async move {
                match future.await {
                    Ok(()) => PromiseOutcome::Resolve(Box::new(
                        move |_cx: *mut RawJSContext, mut rval: MutableHandleValue| {
                            rval.set(value::undefined());
                            true
                        },
                    )),
                    Err(e) => PromiseOutcome::Reject(e.to_string()),
                }
            }),
        }
    }
}

thread_local! {
    // Crown: `PendingPromise` is self-rooting via `RootedTraceableBox`, so we
    // don't need to root the Vec itself.
    #[js::allow_unrooted_interior]
    static PENDING_FUTURES: RefCell<Vec<PendingPromise>> = RefCell::new(Vec::new());
}

/// Queue a future that will resolve or reject a JS Promise.
///
/// This is called by generated JSNative wrappers. It stores the promise
/// object in a `RootedTraceableBox<Heap<*mut JSObject>>` for GC safety
/// and queues the future for later execution via [`drain_promises`].
///
/// # Safety
///
/// - `promise_obj` must be a valid JS Promise object.
#[doc(hidden)]
// Crown: The provided `promise_obj` is rooted immediately.
#[js::allow_unrooted_interior]
pub unsafe fn __spawn_promise(promise_obj: *mut JSObject, js_promise: JSPromise) {
    let boxed_heap = RootedTraceableBox::new(Heap::default());
    boxed_heap.set(promise_obj);

    PENDING_FUTURES.with(|f| {
        f.borrow_mut().push((boxed_heap, js_promise.future));
    });
}

// ---------------------------------------------------------------------------
// Symbol.toStringTag
// ---------------------------------------------------------------------------

/// Define `Symbol.toStringTag` on an object (typically a prototype).
///
/// Sets the well-known `@@toStringTag` property to `tag_value` with attributes
/// non-writable, non-enumerable, configurable (per WebIDL §3.7.6).
///
/// This makes `Object.prototype.toString.call(obj)` return
/// `"[object <tag_value>]"`.
///
/// # Panics
///
/// Panics if the string allocation or property definition fails.
pub fn define_to_string_tag(
    scope: &js::gc::scope::Scope<'_>,
    proto: js::native::GCHandle<'_, *mut JSObject>,
    tag_value: &str,
) {
    let tag_key = js::symbol::get_well_known_key(scope, js::native::SymbolCode::toStringTag);
    let tag_str =
        js::string::from_str(scope, tag_value).expect("failed to create toStringTag string");
    // SAFETY: tag_str is a live JSString* from `from_str` above, valid in the current scope.
    let str_val = unsafe { value::from_string_raw(tag_str.get()) };

    js::rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut desc = js::native::PropertyDescriptor {
        _bitfield_align_1: [0; 0],
        _bitfield_1: js::native::PropertyDescriptor::new_bitfield_1(
            true,  // hasConfigurable
            true,  // configurable
            true,  // hasEnumerable
            false, // enumerable
            true,  // hasWritable
            false, // writable
            true,  // hasValue
            false, // hasGetter
            false, // hasSetter
            false, // resolving
        ),
        getter_: ptr::null_mut(),
        setter_: ptr::null_mut(),
        value_: str_val,
    });

    js::rooted!(in(unsafe { scope.raw_cx_no_gc() }) let tag_id = tag_key);
    let proto_handle: js::native::HandleObject = proto;
    js::object::define_property_by_id(scope, proto_handle, tag_id.handle(), desc.handle())
        .expect("failed to define Symbol.toStringTag");
}

/// Capture the current call stack from a temporary `Error` object and set it
/// as the `stack` property on `obj`.
///
/// This creates `new Error()` to let SpiderMonkey capture the stack trace,
/// reads the resulting `stack` property, and copies it onto `obj`.
///
/// Used by error-like classes (e.g. DOMException, or any class with
/// `js_proto = "Error"`) that need `[[ErrorData]]` behavior.
///
/// Silently returns without setting `stack` if any step fails.
///
/// # Safety
///
/// Must be called within a valid scope with an active realm.
pub unsafe fn capture_stack_from_error(
    scope: &js::gc::scope::Scope<'_>,
    obj: &js::object::Object<'_>,
) {
    // Create `new Error()` to capture the current stack.
    let error_ctor =
        match js::class::get_class_object(scope, js::class_spec::JSProtoKey::JSProto_Error) {
            Ok(ctor) => ctor,
            Err(_) => return,
        };

    let ctor_val = scope.root_value(unsafe { js::value::from_object(error_ctor.get()) });
    let empty_args = js::native::HandleValueArray {
        length_: 0,
        elements_: ptr::null(),
    };

    let error_obj = match js::function::construct(scope, ctor_val, &empty_args) {
        Ok(obj) => obj,
        Err(_) => return,
    };

    // Read the `stack` property from the Error object.
    let stack_val = match error_obj.get_property(scope, c"stack") {
        Ok(v) => v,
        Err(_) => return,
    };

    // Set the stack as an own property on the target object.
    let stack_handle = scope.root_value(stack_val);
    let _ = obj.set_property(scope, c"stack", stack_handle);
}

/// Drain all pending promise futures, resolve/reject them, and run microtasks.
///
/// The `run` callback is called once per pending future and must drive the
/// future to completion (e.g. via `tokio::runtime::Runtime::block_on`).
///
/// # Safety
///
/// - `cx` must be a valid `JSContext` pointer.
/// - Must be called on the JS thread.
///
/// # Example
///
/// ```rust,ignore
/// let rt = tokio::runtime::Builder::new_current_thread()
///     .enable_all()
///     .build()
///     .unwrap();
/// drain_promises(cx, |fut| rt.block_on(fut));
/// ```
pub unsafe fn drain_promises<F>(scope: &js::gc::scope::Scope<'_>, run: F)
where
    F: Fn(Pin<Box<dyn Future<Output = PromiseOutcome> + 'static>>) -> PromiseOutcome,
{
    let futures: Vec<_> = PENDING_FUTURES.with(|f| f.borrow_mut().drain(..).collect());

    for (promise_heap, future) in futures {
        let promise = promise_heap.handle();
        if promise.get().is_null() {
            continue;
        }

        match run(future) {
            PromiseOutcome::Resolve(set_value) => {
                js::rooted!(in(unsafe { scope.raw_cx_no_gc() }) let mut val = value::undefined());
                if set_value(scope.cx_mut().raw_cx(), val.handle_mut()) {
                    let p = js::promise::Promise::from_handle(promise);
                    let _ = p.resolve(scope, val.handle());
                }
            }
            PromiseOutcome::Reject(msg) => {
                let js_str =
                    js::string::from_str(scope, &msg).expect("from_str can't fail except on OOM");
                let err_val = scope.root_value(value::from_string_raw(js_str.get()));
                let p = js::promise::Promise::from_handle(promise);
                let _ = p.reject(scope, err_val);
            }
        }
    }

    // Run microtasks (promise .then() handlers)
    js::jobs::run_jobs(scope);
}
