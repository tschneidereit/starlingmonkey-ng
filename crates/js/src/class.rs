// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! JSClass definition, standard class initialization, and the class system.
//!
//! This module wraps SpiderMonkey's class system, providing access to
//! `JS_InitClass`, standard class resolution, and global object creation.
//! It also provides the [`ClassDef`] trait and supporting infrastructure
//! for defining JavaScript classes backed by Rust structs.

use std::any::TypeId;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::{c_void, CStr, CString};
use std::marker::PhantomData;
use std::os::raw::c_char;
use std::ptr::{self, NonNull};

pub use crate::builtins::is_derived_from_type;
use crate::builtins::{get_class_tag, CastError, CastTarget, JSType};
use crate::class_spec::{
    JS_EnumerateStandardClasses, JS_GlobalObjectTraceHook, JS_MayResolveStandardClass,
    JS_ResolveStandardClass,
};
use crate::conversion::{ConversionBehavior, ConversionError, FromJSVal, ToJSVal};
use crate::error::{capture_stack_from_error, ExnThrown};
use crate::gc::handle::Stack;
use crate::gc::scope::{RootScope, Scope};
use crate::heap::{MozHeap, Trace};
use crate::native::{
    CallArgs, GCContext, HandleObject, JSNative, JSObject, JSTracer, RawJSContext, Value,
};
use crate::value;
use crate::Object;
use mozjs::gc::Handle;
use mozjs::jsapi::{
    JSClass, JSClassOps, JSFunctionSpec, JSPrincipals, JSPropertySpec, JSProtoKey,
    JS_IsGlobalObject, OnNewGlobalHookOption, PropertyKey, RealmOptions,
    JSCLASS_FOREGROUND_FINALIZE, JSCLASS_GLOBAL_FLAGS, JSCLASS_IS_GLOBAL,
};
use mozjs::rooted;
use mozjs::rust::wrappers2;
use mozjs_sys::jsapi::JS::SymbolCode;
use starling_macro::must_root;

/// Initialize a class on a global object.
///
/// This defines a constructor and prototype, wiring them together.
///
/// # Safety
///
/// All pointer parameters must be valid. `ps`, `fs`, `static_ps`, `static_fs`
/// must be null-terminated arrays or null.
pub unsafe fn init_class<'s>(
    scope: &'s Scope<'_>,
    global: HandleObject,
    proto_class: *const JSClass,
    proto_proto: HandleObject,
    name: *const c_char,
    constructor: JSNative,
    nargs: u32,
    ps: *const JSPropertySpec,
    fs: *const JSFunctionSpec,
    static_ps: *const JSPropertySpec,
    static_fs: *const JSFunctionSpec,
) -> Result<Object<'s>, ExnThrown> {
    let obj = wrappers2::JS_InitClass(
        scope.cx_mut(),
        global,
        proto_class,
        proto_proto,
        name,
        constructor,
        nargs,
        ps,
        fs,
        static_ps,
        static_fs,
    );
    Object::from_raw(scope, obj).ok_or(ExnThrown)
}

/// Create a new global object.
///
/// # Safety
///
/// `clasp` must be a valid `JSClass` with the `GLOBAL` flag.
/// `principals` may be null. `options` must be valid.
pub unsafe fn new_global_object<'s>(
    scope: &'s Scope<'_>,
    clasp: *const JSClass,
    principals: *mut JSPrincipals,
    hook_option: OnNewGlobalHookOption,
    options: *const RealmOptions,
) -> Result<Object<'s>, ExnThrown> {
    let obj =
        wrappers2::JS_NewGlobalObject(scope.cx_mut(), clasp, principals, hook_option, options);
    Object::from_raw(scope, obj).ok_or(ExnThrown)
}

/// Initialize the standard classes on a global object.
pub fn init_standard_classes(scope: &Scope<'_>) -> Result<(), ExnThrown> {
    let ok = unsafe { wrappers2::InitRealmStandardClasses(scope.cx_mut()) };
    ExnThrown::check(ok)
}

/// Resolve a standard class by name (lazily).
pub fn resolve_standard_class(
    scope: &Scope<'_>,
    obj: HandleObject,
    id: Handle<PropertyKey>,
) -> Result<bool, ExnThrown> {
    let mut resolved = false;
    let ok = unsafe { wrappers2::JS_ResolveStandardClass(scope.cx_mut(), obj, id, &mut resolved) };
    ExnThrown::check(ok)?;
    Ok(resolved)
}

/// Eagerly enumerate all standard classes on a global object.
pub fn enumerate_standard_classes(scope: &Scope<'_>, obj: HandleObject) -> Result<(), ExnThrown> {
    let ok = unsafe { wrappers2::JS_EnumerateStandardClasses(scope.cx_mut(), obj) };
    ExnThrown::check(ok)
}

/// Get the constructor for a standard class by `JSProtoKey`.
pub fn get_class_object<'s>(
    scope: &'s Scope<'_>,
    key: JSProtoKey,
) -> Result<Handle<'s, *mut JSObject>, ExnThrown> {
    let mut objp = scope.root_object_mut(std::ptr::null_mut());
    let ok = unsafe { wrappers2::JS_GetClassObject(scope.cx_mut(), key, objp.reborrow()) };
    ExnThrown::check(ok)?;
    Ok(objp.handle())
}

/// Get the realm's `%AsyncIteratorPrototype%` — the object that supplies the
/// `[Symbol.asyncIterator]` method (returning `this`) inherited by all async
/// iterators.
pub fn get_async_iterator_prototype<'s>(
    scope: &'s Scope<'_>,
) -> Result<Handle<'s, *mut JSObject>, ExnThrown> {
    let cx = unsafe { scope.raw_cx_no_gc() };
    let obj = unsafe { mozjs::jsapi::GetRealmAsyncIteratorPrototype(cx) };
    std::ptr::NonNull::new(obj)
        .map(|p| scope.root_object(p))
        .ok_or(ExnThrown)
}

/// Get the realm's `%IteratorPrototype%` — the object at the top of the
/// prototype chain of all built-in iterators, supplying `[Symbol.iterator]`
/// (returning `this`). WebIDL pair iterators chain their prototype under this.
pub fn get_iterator_prototype<'s>(scope: &'s Scope<'_>) -> Result<Object<'s>, ExnThrown> {
    let cx = unsafe { scope.raw_cx_no_gc() };
    let obj = unsafe { mozjs::jsapi::GetRealmIteratorPrototype(cx) };
    // SAFETY: GetRealmIteratorPrototype returns an object or null.
    unsafe { Object::from_mozjs_rval(scope, obj) }
}

/// Get the prototype for a standard class by `JSProtoKey`.
pub fn get_class_prototype<'s>(
    scope: &'s Scope<'_>,
    key: JSProtoKey,
) -> Result<Handle<'s, *mut JSObject>, ExnThrown> {
    let mut objp = scope.root_object_mut(std::ptr::null_mut());
    let ok = unsafe { wrappers2::JS_GetClassPrototype(scope.cx_mut(), key, objp.reborrow()) };
    ExnThrown::check(ok)?;
    Ok(objp.handle())
}

/// Get the `JSClass` for a standard class by `JSProtoKey`.
///
/// Returns a stable pointer to the `JSClass` that SpiderMonkey uses for
/// builtin types like `Array`, `Date`, `Promise`, etc.
pub fn proto_key_to_class(key: JSProtoKey) -> *const JSClass {
    // SAFETY: ProtoKeyToClass is a pure lookup into a static table.
    unsafe { mozjs::jsapi::ProtoKeyToClass(key) }
}

/// Initialize `Reflect.parse` on a global object.
pub fn init_reflect_parse(scope: &Scope<'_>, global: HandleObject) -> Result<(), ExnThrown> {
    let ok = unsafe { wrappers2::JS_InitReflectParse(scope.cx_mut(), global) };
    ExnThrown::check(ok)
}

/// Link a constructor and its prototype.
pub fn link_constructor_and_prototype(
    scope: &Scope<'_>,
    ctor: HandleObject,
    proto: HandleObject,
) -> Result<(), ExnThrown> {
    let ok = unsafe { wrappers2::JS_LinkConstructorAndPrototype(scope.cx_mut(), ctor, proto) };
    ExnThrown::check(ok)
}

/// Fire the `onNewGlobalObject` hook for a newly created global.
pub fn fire_on_new_global_object(scope: &Scope<'_>, global: HandleObject) {
    unsafe { wrappers2::JS_FireOnNewGlobalObject(scope.cx_mut(), global) }
}

/// Check whether an object is an instance of the given `JSClass`.
///
/// This checks the object's direct class — not the prototype chain.
/// Returns `false` for null objects or objects of different classes.
///
/// Unlike `JS_InstanceOf`, this does **not** throw on failure: pass `null`
/// for the `args` parameter to suppress the TypeError.
///
/// # Safety
///
/// `obj` must be a valid rooted object handle. `clasp` must point to a
/// valid `JSClass` that will remain valid for the duration of the call.
pub fn instance_of(scope: &Scope<'_>, obj: HandleObject, clasp: &JSClass) -> bool {
    // Safety: JS_InstanceOf with a null CallArgs pointer performs a
    // non-throwing check: it returns true if `obj` has `clasp` as its
    // direct class, false otherwise.
    unsafe {
        mozjs::jsapi::JS_InstanceOf(
            scope.cx_mut().raw_cx(),
            obj.into(),
            clasp,
            std::ptr::null_mut(),
        )
    }
}

// ============================================================================
// Class system — ClassDef, registration, private data, inheritance, etc.
//
// Facilities for defining JavaScript classes backed by Rust structs. The
// [`ClassDef`] trait describes how a Rust struct maps to a JavaScript class.
// In practice, classes are defined via the [`#[jsclass]`] and [`#[jsmethods]`]
// proc macros.
// ============================================================================

// The code below was originally in core-runtime/src/class.rs. All of its
// imports reference `crate::*` (i.e. the `js` crate). No external
// dependencies on core-runtime are needed.

// ============================================================================
// Marker types
// ============================================================================

impl<'s, T: JSType + ClassDef> Stack<'s, T> {
    /// Borrow the private Rust data, returning a guard that dereferences to
    /// `&T`.
    ///
    /// Returns `None` if the object doesn't have private data of type `T`.
    ///
    /// # Panics
    ///
    /// Panics if the data is already mutably borrowed — i.e. a JS reentry
    /// borrowed this object's data while a [`data_mut`](Self::data_mut) guard
    /// was still live.
    pub fn data(&self) -> Option<Ref<'_, T>> {
        let obj = self.handle.get();
        let flag = unsafe { get_borrow_flag(obj)? };
        // Check (and panic on conflict) before materializing the `&T`, so the
        // reference is never created while a mutable borrow is live.
        acquire_shared(flag);
        match unsafe { get_private_or_ancestor::<T>(obj) } {
            Some(value) => Some(Ref { value, flag }),
            None => {
                flag.set(flag.get() - 1);
                None
            }
        }
    }

    /// Mutably borrow the private Rust data, returning a guard that
    /// dereferences to `&mut T`.
    ///
    /// Returns `None` if the object doesn't have private data of type `T`.
    ///
    /// # Panics
    ///
    /// Panics if the data is already borrowed (shared or mutable) — i.e. a JS
    /// reentry borrowed this object's data while a borrow was still live. See
    /// [`data`](Self::data).
    pub fn data_mut(&self) -> Option<RefMut<'_, T>> {
        let obj = self.handle.get();
        let flag = unsafe { get_borrow_flag(obj)? };
        acquire_mut(flag);
        match unsafe { get_private_or_ancestor_mut::<T>(obj) } {
            Some(value) => Some(RefMut { value, flag }),
            None => {
                flag.set(UNUSED);
                None
            }
        }
    }
}

// Blanket impl: every ClassDef is automatically a JSType.
impl<T: ClassDef> JSType for T {
    type Rooted<'s> = <T as ClassDef>::Rooted<'s>;
    const JS_NAME: &'static str = T::NAME;

    fn js_class() -> *const JSClass {
        T::class()
    }

    #[inline]
    unsafe fn is_instance(obj: *mut JSObject) -> bool {
        // The class tag must match (allowing subclasses), and the object must carry private data.
        // Both must be checked because the prototype object shares the instance JSClass but
        // doesn't have the private slot.
        is_derived_from_type(unsafe { get_class_tag(obj) }, Self::js_class() as usize)
            && unsafe { get_raw_private(obj).is_some() }
    }
}

/// Typed variadic rest arguments.
///
/// Use this as the type of the last parameter to collect all remaining JS
/// arguments. It is accepted anywhere a callable takes arguments: `#[method]`,
/// `#[static_method]`, and `#[constructor]` in `#[jsmethods]`/
/// `#[webidl_methods]` blocks, and the free functions exposed by `#[jsmodule]`,
/// `#[jsglobals]`, `#[jsnamespace]`, and `#[webidl_namespace]`.
///
/// `T` must implement [`FromJSVal`](crate::conversion::FromJSVal) and either
/// not need rooting, such as `i32`, `f64` or `String`, or be a scope-rooted
/// type, such as `HandleValue<'_>`, `Promise<'_>`, or `Request<'_>`.
///
/// Note that instead of `HandleValue<'_>`, you can also use `&CallArgs`, which gives you access
/// to the raw, already rooted, arguments vector and count.
///
/// # Examples
///
/// ```rust,ignore
/// // Collect typed f64 arguments — no manual conversion needed:
/// #[static_method]
/// fn sum(rest: RestArgs<f64>) -> f64 {
///     rest.iter().sum()
/// }
/// ```
pub struct RestArgs<T>(Vec<T>);

impl<'s, 'v, T: FromJSVal<'s, 'v>> RestArgs<T> {
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

/// Trait implemented by all generated stack newtype wrappers (e.g. `Dog<'s>`).
///
/// Enables type-checked [`cast`](StackType::cast) between stack newtypes
/// without needing a scope reference, since the underlying handle is already
/// rooted.
pub trait StackType<'s>: Sized + Copy {
    /// The inner `ClassDef` data type (e.g. `DogImpl`).
    ///
    /// Implementations always bind an `#[allow_unrooted_interior]` type (the
    /// class macros mark every generated `FooImpl`); crown requires the
    /// declaration to carry the same marker once an implementation lives in
    /// this crate, as `iteration::AsyncIteratorRecordImpl` does.
    #[must_root]
    type Inner: ClassDef;

    /// Construct from a handle without checking the type tag.
    ///
    /// # Safety
    ///
    /// The handle must point to a JS object backed by `Self::Inner`
    /// (or a subclass).
    unsafe fn from_handle_unchecked(h: crate::native::GCHandle<'s, *mut JSObject>) -> Self;

    /// Get the underlying rooted object handle.
    fn js_handle(self) -> crate::native::GCHandle<'s, *mut JSObject>;

    /// Type-checked cast to another type.
    ///
    /// Returns `Ok(T)` if the underlying JS object is an instance of `T`
    /// or a subclass of `T`, `Err(CastError)` otherwise.
    fn cast<T: CastTarget<'s>>(self) -> Result<T::Output, CastError> {
        let ptr = self.js_handle().get();
        if !unsafe { T::is_instance(ptr) } {
            return Err(CastError {
                from: Self::Inner::NAME,
                to: T::TARGET_NAME,
            });
        }
        Ok(unsafe { T::construct_unchecked(self.js_handle()) })
    }
}

/// Blanket impl: every `Stack<'s, T>` where `T: ClassDef` is a `StackType`.
impl<'s, T: ClassDef> StackType<'s> for Stack<'s, T> {
    type Inner = T;

    unsafe fn from_handle_unchecked(h: crate::native::GCHandle<'s, *mut JSObject>) -> Self {
        Stack {
            handle: h,
            _marker: PhantomData,
        }
    }

    fn js_handle(self) -> crate::native::GCHandle<'s, *mut JSObject> {
        self.handle
    }
}

// ============================================================================
// Private data storage
// ============================================================================

const PRIVATE_DATA_SLOT: u32 = 0;
const BORROW_FLAG_SLOT: u32 = 1;

/// Minimum number of reserved slots required for a class instance.
///
/// Slot 0 ([`PRIVATE_DATA_SLOT`]) holds the boxed Rust data; slot 1
/// ([`BORROW_FLAG_SLOT`]) holds the boxed borrow flag that makes
/// [`Stack::data`]/[`Stack::data_mut`] safe (see [`BorrowFlag`]).
///
/// Public for use by generated `ClassDef::CLASS` implementations.
#[doc(hidden)]
pub const MIN_CLASS_RESERVED_SLOTS: u32 = BORROW_FLAG_SLOT + 1;

// ---------------------------------------------------------------------------
// Runtime borrow tracking for private data
// ---------------------------------------------------------------------------
//
// The private data lives behind a raw pointer in a reserved slot, so the borrow
// checker cannot track aliasing of it: `data()` and `data_mut()` both conjure a
// reference from that pointer, and a JS object is reachable through any number
// of copyable `Stack` handles. The danger that remains is *reentrancy* — a
// native method takes `&mut` data, calls back into JS, and the re-entered code
// touches the same object's data, minting a second overlapping reference.
//
// To make `data_mut()` safe we track borrows at runtime with a per-object flag
// stored in `BORROW_FLAG_SLOT`, exactly mirroring `RefCell`: a conflicting
// borrow (mut/mut or mut/shared) panics rather than aliasing. The flag is a
// single `Cell` per object shared across the whole inheritance hierarchy, so
// borrowing a parent's slice conflicts with borrowing the child's. The check is
// a single-threaded `Cell` read/compare/write, which we assume to have
// negligible cost next to the JSAPI calls around it, so it's always on, in
// release as well as debug builds.
// TODO: benchmark this assumption and consider a debug-only check if it turns out to be costly.

/// Borrow state for an object's private data: `0` = unborrowed, `n > 0` = `n`
/// live shared borrows, `-1` = a live mutable borrow.
type BorrowFlag = isize;

const UNUSED: BorrowFlag = 0;
const WRITING: BorrowFlag = -1;

#[cold]
#[inline(never)]
fn borrow_conflict(mutable: bool) -> ! {
    if mutable {
        panic!(
            "private data is already borrowed (a JS reentry mutably borrowed the \
             same object's data while a borrow was live)"
        );
    } else {
        panic!(
            "private data is already mutably borrowed (a JS reentry borrowed the \
             same object's data while a mutable borrow was live)"
        );
    }
}

/// Take a shared borrow on `flag`, panicking if it is mutably borrowed.
///
/// Must be called *before* materializing the `&T`, so the reference is never
/// created while a conflicting borrow is live.
fn acquire_shared(flag: &Cell<BorrowFlag>) {
    let f = flag.get();
    if f < UNUSED {
        borrow_conflict(false);
    }
    flag.set(f + 1);
}

/// Take a mutable borrow on `flag`, panicking if it is borrowed at all.
fn acquire_mut(flag: &Cell<BorrowFlag>) {
    if flag.get() != UNUSED {
        borrow_conflict(true);
    }
    flag.set(WRITING);
}

/// A shared guard over private data, released when dropped.
///
/// Returned by [`Stack::data`] and [`get_this_data`]. Dereferences to `&T`;
/// holding it keeps a shared borrow live, so a reentrant mutable borrow of the
/// same object panics.
#[crate::allow_unrooted_interior]
pub struct Ref<'a, T: ?Sized> {
    value: &'a T,
    flag: &'a Cell<BorrowFlag>,
}

impl<T: ?Sized> std::ops::Deref for Ref<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        self.value
    }
}

impl<T: ?Sized> Drop for Ref<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.flag.set(self.flag.get() - 1);
    }
}

/// A mutable guard over private data, released when dropped.
///
/// Returned by [`Stack::data_mut`] and [`get_this_data_mut`]. Dereferences to
/// `&mut T`; holding it keeps the object's data exclusively borrowed, so any
/// reentrant borrow of the same object panics.
#[crate::allow_unrooted_interior]
pub struct RefMut<'a, T: ?Sized> {
    value: &'a mut T,
    flag: &'a Cell<BorrowFlag>,
}

impl<T: ?Sized> std::ops::Deref for RefMut<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        self.value
    }
}

impl<T: ?Sized> std::ops::DerefMut for RefMut<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        self.value
    }
}

impl<T: ?Sized> Drop for RefMut<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.flag.set(UNUSED);
    }
}

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
    unsafe { crate::object::set_reserved_slot(obj, PRIVATE_DATA_SLOT, &val) };

    // Install the borrow flag (slot 1). One flag per object, shared across the
    // inheritance hierarchy; dropped alongside the data in `drop_private`.
    let flag = Box::into_raw(Box::new(Cell::new(UNUSED)));
    let flag_val = unsafe { value::from_private(flag as *const c_void) };
    unsafe { crate::object::set_reserved_slot(obj, BORROW_FLAG_SLOT, &flag_val) };
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

/// Retrieve a reference to the Rust data stored in a JS object's reserved slot 0.
///
/// # Safety
///
/// - `obj` must be a valid JS object with private data of type `T` stored via [`set_private`].
/// - The returned reference is only valid as long as the JS object is alive and
///   no mutable reference is taken simultaneously.
pub unsafe fn get_private<'a, T: 'static>(obj: *mut JSObject) -> Option<&'a T> {
    let val = unsafe { crate::object::get_reserved_slot(obj, PRIVATE_DATA_SLOT) };
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
    let val = unsafe { crate::object::get_reserved_slot(obj, PRIVATE_DATA_SLOT) };
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
    let val = unsafe { crate::object::get_reserved_slot(obj, PRIVATE_DATA_SLOT) };
    if !val.is_undefined() {
        let ptr = val.to_private() as *mut T;
        if !ptr.is_null() {
            let _ = Box::from_raw(ptr);
            let undef = value::undefined();
            crate::object::set_reserved_slot(obj, PRIVATE_DATA_SLOT, &undef);
        }
    }

    // Drop the borrow flag installed by `set_private`.
    let flag_val = unsafe { crate::object::get_reserved_slot(obj, BORROW_FLAG_SLOT) };
    if !flag_val.is_undefined() {
        let flag_ptr = flag_val.to_private() as *mut Cell<BorrowFlag>;
        if !flag_ptr.is_null() {
            let _ = Box::from_raw(flag_ptr);
            let undef = value::undefined();
            crate::object::set_reserved_slot(obj, BORROW_FLAG_SLOT, &undef);
        }
    }
}

// ============================================================================
// Per-global class prototype registry
// ============================================================================

/// Reserved slot index for the per-global class registry, placed after
/// private class data.
///
/// Note: SpiderMonkey global objects have their first 5 reserved slots available for embedder use.
const CLASS_REGISTRY_SLOT: u32 = MIN_CLASS_RESERVED_SLOTS + 1;

/// Per-global map from `TypeId` to class prototype.
///
/// Stored as private data in [`CLASS_REGISTRY_SLOT`] of the global object.
#[crate::allow_unrooted_interior]
struct ClassRegistry {
    map: HashMap<TypeId, Box<MozHeap<*mut JSObject>>>,
    /// Per-global shared objects keyed by an arbitrary `usize` (by convention a
    /// native function pointer unique to the call site), so the same object is
    /// reused for every caller on a given global. Used for functions whose spec
    /// requires per-global identity (e.g. `[LegacyUnforgeable]` accessor getters,
    /// the streams queuing-strategy `size` functions) and for internal
    /// plumbing objects reused across calls (e.g. a resolved promise serving
    /// as a microtask trigger). Traced like prototypes.
    shared_objects: HashMap<usize, Box<MozHeap<*mut JSObject>>>,
    /// Per-global monotonic `u64` counters keyed by an arbitrary `usize`. Used
    /// where a spec requires ids unique across a whole global rather than per
    /// call site — e.g. HTML's setTimeout/setInterval ids, which must not
    /// collide across the concurrent event loops that share one global. Plain
    /// integers, so no GC tracing.
    counters: HashMap<usize, u64>,
    /// Type-erased GC glue for the global's private data,
    /// installed by [`set_global_private`].
    ///
    /// `None` until a parent class stores its data on the global.
    global_private: Option<GlobalPrivateGlue>,
}

/// Type-erased GC glue for a global's private data, captured at the
/// embedder's typed [`set_global_private`] call site.
struct GlobalPrivateGlue {
    trace: unsafe extern "C" fn(*mut JSTracer, *mut JSObject),
    finalize: unsafe extern "C" fn(*mut GCContext, *mut JSObject),
}

impl ClassRegistry {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            shared_objects: HashMap::new(),
            counters: HashMap::new(),
            global_private: None,
        }
    }

    /// Return the counter for `key` and advance it by one (wrapping on
    /// overflow). Counters start at 0.
    fn bump_counter(&mut self, key: usize) -> u64 {
        let slot = self.counters.entry(key).or_insert(0);
        let value = *slot;
        *slot = slot.wrapping_add(1);
        value
    }

    fn register(&mut self, type_id: TypeId, proto: *mut JSObject) {
        let entry = self
            .map
            .entry(type_id)
            .or_insert_with(|| MozHeap::boxed(ptr::null_mut()));
        entry.set(proto);
    }

    fn get(&self, type_id: TypeId) -> Option<*mut JSObject> {
        self.map.get(&type_id).map(|h| h.get())
    }

    fn get_shared_object(&self, key: usize) -> Option<*mut JSObject> {
        self.shared_objects.get(&key).map(|h| h.get())
    }

    fn set_shared_object(&mut self, key: usize, obj: *mut JSObject) {
        let entry = self
            .shared_objects
            .entry(key)
            .or_insert_with(|| MozHeap::boxed(ptr::null_mut()));
        entry.set(obj);
    }

    /// Trace all prototype and shared-object heap values so moving GC can update them.
    #[crate::allow_unrooted]
    unsafe fn trace(&self, trc: *mut JSTracer) {
        for heap in self.map.values() {
            heap.trace(trc);
        }
        for heap in self.shared_objects.values() {
            heap.trace(trc);
        }
    }
}

/// Read the `ClassRegistry` pointer from a global object's reserved slot.
///
/// Returns `None` if the slot is unset (e.g. on a non-StarlingMonkey global).
unsafe fn get_class_registry(global: *mut JSObject) -> Option<&'static ClassRegistry> {
    let val = crate::object::get_reserved_slot(global, CLASS_REGISTRY_SLOT);
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
    let val = crate::object::get_reserved_slot(global, CLASS_REGISTRY_SLOT);
    if !val.is_undefined() {
        let ptr = val.to_private() as *mut ClassRegistry;
        if !ptr.is_null() {
            return &mut *ptr;
        }
    }
    let registry = Box::into_raw(Box::new(ClassRegistry::new()));
    let pv = value::from_private(registry as *const c_void);
    crate::object::set_reserved_slot(global, CLASS_REGISTRY_SLOT, &pv);
    LIVE_REGISTRIES.with(|regs| regs.borrow_mut().push(registry));
    &mut *registry
}

thread_local! {
    /// All live `ClassRegistry` boxes on this thread, traced by [`trace_class_registries`].
    static LIVE_REGISTRIES: RefCell<Vec<*mut ClassRegistry>> = const { RefCell::new(Vec::new()) };
}

/// Trace each global's class registry, rooting prototype and shared-function objects.
/// Registered as an extra-GC-roots tracer by [`crate::gc::init`].
///
/// # Safety
///
/// `trc` must be a valid `JSTracer` provided by SpiderMonkey's GC.
pub(crate) unsafe extern "C" fn trace_class_registries(trc: *mut JSTracer, _data: *mut c_void) {
    LIVE_REGISTRIES.with(|regs| {
        // `as_ptr` bypasses RefCell borrow tracking: GC tracing pauses JS execution, so a borrow
        // held by interrupted code isn't actively used while the list is read here.
        let regs = &*regs.as_ptr();
        for &registry in regs {
            (*registry).trace(trc);
        }
    });
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
/// The finalize hook drops the registry.
pub static STARLING_GLOBAL_CLASS: JSClass = JSClass {
    name: c"ServiceWorkerGlobal".as_ptr(),
    flags: JSCLASS_IS_GLOBAL | JSCLASS_FOREGROUND_FINALIZE | JSCLASS_GLOBAL_FLAGS,
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

/// Destructor for Starling's global class — drops the global's
/// class's private data (if any) and the class registry.
unsafe extern "C" fn finalize_starling_global(gc: *mut GCContext, obj: *mut JSObject) {
    let val = crate::object::get_reserved_slot(obj, CLASS_REGISTRY_SLOT);
    if !val.is_undefined() {
        let ptr = val.to_private() as *mut ClassRegistry;
        if !ptr.is_null() {
            // Drop the global's private data before the registry that owns its glue.
            if let Some(glue) = &(*ptr).global_private {
                (glue.finalize)(gc, obj);
            }
            // Unregister from the tracer's list before freeing.
            LIVE_REGISTRIES.with(|regs| regs.borrow_mut().retain(|&r| r != ptr));
            drop(Box::from_raw(ptr));
            // Clear the slot so we don't double-free.
            let undef = value::undefined();
            crate::object::set_reserved_slot(obj, CLASS_REGISTRY_SLOT, &undef);
        }
    }
}

/// Store `data` as the global's private class data and record the GC glue
/// needed to trace and finalize it, and set `proto` as the `__proto__`.
///
/// # Safety
///
/// - `global` must be a [`STARLING_GLOBAL_CLASS`] global.
/// - Must be called at most once per global; the data outlives every borrow.
pub unsafe fn set_global_private_and_proto<T: ClassDef>(
    scope: &Scope<'_>,
    global: crate::Object<'_>,
    data: T,
    proto: crate::Object<'_>,
) {
    unsafe {
        assert!(JS_IsGlobalObject(global.as_raw()));
        assert!(
            get_private::<T>(global.as_raw()).is_none(),
            "global private data already set"
        );
    }
    set_private(global.as_raw(), data);
    global.set_prototype(scope, proto.handle()).unwrap();
    let success = global
        .make_proto_immutable(scope)
        .expect("Making the global's proto immutable mustn't error");

    assert!(success, "Making the global's proto immutable must succeed");
    let registry = get_or_init_class_registry(global.as_raw());
    registry.global_private = Some(GlobalPrivateGlue {
        trace: generic_class_trace::<T>,
        finalize: generic_class_finalize::<T>,
    });
    register_global_parent::<T>();
}

/// Custom global trace op for private data, installed via
/// [`install_global_private_trace`].
///
/// # Safety
///
/// `trc` and `global` must be the valid tracer and global supplied by the GC.
unsafe extern "C" fn global_private_trace(trc: *mut JSTracer, global: *mut JSObject) {
    if let Some(registry) = get_class_registry(global) {
        if let Some(glue) = &registry.global_private {
            (glue.trace)(trc, global);
        }
    }
}

/// Install the custom global trace op that traces a global's private
/// data as the realm's `setTrace` op.
///
/// Must be called on the [`RealmOptions`](crate::engine::RealmOptions) used to
/// create a [`STARLING_GLOBAL_CLASS`] global, before `JS_NewGlobalObject`. The
/// op is a no-op until [`set_global_private`] populates the global, so it is
/// safe to install unconditionally for Starling globals.
pub fn install_global_private_trace(options: &mut crate::engine::RealmOptions) {
    options.creationOptions_.traceGlobal_ = Some(global_private_trace);
}

/// Get the registered prototype for a class type. Public for generated code.
#[doc(hidden)]
pub fn get_prototype_for<T: 'static>(scope: &Scope<'_>) -> Option<*mut JSObject> {
    get_prototype::<T>(scope.global())
}

/// Get the registered prototype for a class type as a rooted [`Object`].
pub fn get_prototype_object_for<'s, T: 'static>(scope: &'s Scope<'_>) -> Option<Object<'s>> {
    // SAFETY: the pointer comes straight from the registry into a rooted Object,
    // with no intervening allocation.
    unsafe { Object::from_raw(scope, get_prototype::<T>(scope.global())?) }
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

pub(crate) fn inherits_from(concrete_tag: usize, target_tag: usize) -> bool {
    INHERITANCE_REGISTRY.with(|reg| {
        let map = reg.borrow();
        map.get(&concrete_tag)
            .is_some_and(|info| info.ancestors.contains(&target_tag))
    })
}

// Registry mapping child type tag → parent info.
// Thread-local because the SpiderMonkey runtime is single-threaded.
thread_local! {
    static INHERITANCE_REGISTRY: RefCell<HashMap<usize, InheritanceInfo>> = RefCell::new(HashMap::new());
}

/// Register a child→parent derivation in the inheritance registry, keyed by the
/// child's runtime `JSClass` tag.
///
/// `accessor`/`accessor_mut` map a pointer to the child's private data onto a
/// pointer to the embedded parent data, so [`get_private_or_ancestor`] can reach
/// a `Parent`'s data through a concrete child instance.
fn register_parent_info_raw(
    child_tag: usize,
    parent_tag: usize,
    accessor: unsafe fn(*const c_void) -> *const c_void,
    accessor_mut: unsafe fn(*mut c_void) -> *mut c_void,
) {
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
                accessor,
                accessor_mut,
            },
        );
    });
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

    register_parent_info_raw(
        class_tag::<C>(),
        class_tag::<C::Parent>(),
        immutable_accessor::<C>,
        mutable_accessor::<C>,
    );
}

/// Register that the [`STARLING_GLOBAL_CLASS`] global derives from `P`, whose
/// data is stored as the global's private data (slot 0) via
/// [`set_global_private_and_proto`].
///
/// This makes brand checks and casts against `P` succeed on the global — e.g.
/// `global instanceof EventTarget` and `obj.cast::<EventTarget>()`.
fn register_global_parent<P: ClassDef>() {
    unsafe fn identity_const(ptr: *const c_void) -> *const c_void {
        ptr
    }
    unsafe fn identity_mut(ptr: *mut c_void) -> *mut c_void {
        ptr
    }

    register_parent_info_raw(
        &STARLING_GLOBAL_CLASS as *const JSClass as usize,
        class_tag::<P>(),
        identity_const,
        identity_mut,
    );
}

/// Get the raw private data pointer from slot 0 without type interpretation.
unsafe fn get_raw_private(obj: *mut JSObject) -> Option<*const c_void> {
    let val = unsafe { crate::object::get_reserved_slot(obj, PRIVATE_DATA_SLOT) };
    if val.is_undefined() {
        return None;
    }
    let ptr = val.to_private();
    if ptr.is_null() {
        return None;
    }
    Some(ptr)
}

/// Get the per-object borrow flag from slot 1.
///
/// Returns `None` for objects without private data (e.g. a prototype object).
///
/// # Safety
///
/// `obj` must be a valid JS object created with at least
/// [`MIN_CLASS_RESERVED_SLOTS`] reserved slots.
unsafe fn get_borrow_flag<'a>(obj: *mut JSObject) -> Option<&'a Cell<BorrowFlag>> {
    let val = unsafe { crate::object::get_reserved_slot(obj, BORROW_FLAG_SLOT) };
    if val.is_undefined() {
        return None;
    }
    let ptr = val.to_private() as *const Cell<BorrowFlag>;
    if ptr.is_null() {
        return None;
    }
    Some(&*ptr)
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
    if crate::object::reserved_slot_count(obj) < MIN_CLASS_RESERVED_SLOTS {
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
    if crate::object::reserved_slot_count(obj) < MIN_CLASS_RESERVED_SLOTS {
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
    /// The generated stack newtype for this class (e.g. `Dog<'s>`).
    ///
    /// The blanket `impl<T: ClassDef> JSType for T` forwards this as
    /// [`JSType::Rooted`](crate::builtins::JSType::Rooted), so rooting a
    /// `Heap<DogImpl>` yields a `Dog<'s>` without a call-site annotation.
    type Rooted<'s>: From<Stack<'s, Self>>;

    /// The name of the class as it appears in JavaScript.
    const NAME: &'static str;
    /// The name as a CStr for compile-time formatting.
    const NAME_CSTR: &'static CStr;

    /// Pre-formatted error message: `'this' is not of type <Name>`.
    /// Generated by the proc macro as a compile-time C string literal.
    const NOT_TYPE_ERROR: &'static CStr;

    /// Pre-formatted error message for method/getter calls on the prototype.
    /// Generated by the proc macro as a compile-time C string literal.
    const PROTOTYPE_CALL_ERROR: &'static CStr;

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
    fn constructor(scope: &Scope<'_>, args: &CallArgs) -> Result<Self, ExnThrown>;

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

    /// Whether this interface inherits from a parent interface via WebIDL
    /// `extends` (as opposed to `js_proto`, which only borrows a built-in
    /// prototype). When true, the interface object's [[Prototype]] is chained to
    /// the parent interface object. Set by the `extends` macro option.
    const INHERITS_INTERFACE: bool = false;

    /// The constructor function's `length`: its number of required arguments.
    ///
    /// The `#[jsclass]`/`#[webidl_interface]` macros override this to delegate
    /// to the constructor registrar in monomorphic context (where autoref
    /// specialization can see the `#[jsmethods]` impl). The default is 0.
    fn constructor_nargs() -> u32 {
        0
    }

    /// Called during GC finalization, before the Rust data is dropped.
    ///
    /// Use `#[destructor]` in `#[jsmethods]` to define this.
    /// The default implementation does nothing.
    fn destructor(&mut self) {}

    /// Return the prototype of the parent class, or null if no parent.
    ///
    /// Override by using `#[jsclass(extends = ParentType)]`.
    fn parent_prototype(_scope: &Scope<'_>) -> *mut JSObject {
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
    fn ensure_parent_registered(_scope: &Scope<'_>, _global: Object<'_>) {}

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

    /// Whether `new Foo()` is allowed from JavaScript, i.e. whether the
    /// class's `#[jsmethods]` block declares a `#[constructor]`.
    ///
    /// The macro-generated impl delegates to
    /// [`__ConstructorRegistrar::defined`], so the answer comes from the
    /// `#[jsmethods]` expansion rather than the class attribute.
    fn has_js_constructor() -> bool {
        true
    }

    /// Whether `Foo` is installed as a property on the global object.
    /// When `true`, the class is hidden and accessible from JavaScript only
    /// via the `constructor` property on instances' prototype.
    ///
    /// Set by the `hidden` macro option.
    const HIDDEN: bool = false;

    /// Post-construction hook called after the private data has been stored
    /// on the JS object.
    ///
    /// This runs inside `generic_constructor`, after `set_private`, with access
    /// to the JS object and the original constructor arguments. Use this for
    /// initialization steps that require the JS object reference (e.g., setting
    /// up child objects that back-reference the parent).
    ///
    /// Use `#[post_init]` in `#[jsmethods]` to define this.
    fn post_init(_scope: &Scope<'_>, _obj: Object<'_>, _args: &CallArgs) -> Result<(), ExnThrown> {
        Ok(())
    }

    /// Install this interface's `[LegacyUnforgeable]` accessors as own,
    /// non-configurable properties on a freshly constructed instance.
    ///
    /// WebIDL §3.7.4 requires unforgeable members to live on each instance
    /// rather than the prototype, with a getter shared across instances. The
    /// proc macro generates an override for interfaces that declare
    /// `#[getter(unforgeable)]`; it installs its own attributes and then chains
    /// to the parent's via `extends`. Runs inside `generic_constructor` after
    /// `post_init`. The default is a no-op.
    fn install_unforgeable(_scope: &Scope<'_>, _obj: Object<'_>) -> Result<(), ExnThrown> {
        Ok(())
    }

    /// Debug assertion that all bare `Heap<T>` fields have been initialized.
    ///
    /// Called automatically after construction + post_init completes.
    /// The proc macro generates an override that checks each `Heap<T>`
    /// field; `Option<Heap<T>>` fields are skipped since `None` is valid.
    ///
    /// The default implementation is a no-op for classes without `Heap<T>`
    /// fields.
    fn debug_assert_fully_initialized(&self) {}
}

// ============================================================================
// ClassBuilder
// ============================================================================

/// A builder for defining JavaScript class methods and properties.
///
/// Use this in your [`ClassDef::register_class_methods`] implementation to
/// add methods to the class prototype.
#[crate::allow_unrooted_interior]
pub struct ClassBuilder<T: ClassDef> {
    methods: Vec<JSFunctionSpec>,
    properties: Vec<JSPropertySpec>,
    constants: Vec<(&'static CStr, i32)>,
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
    pub fn method(
        mut self,
        name: &'static std::ffi::CStr,
        nargs: u32,
        func: JSNative,
        flags: u16,
    ) -> Self {
        self.methods.push(JSFunctionSpec {
            name: crate::class_spec::JSPropertySpec_Name {
                string_: name.as_ptr(),
            },
            call: crate::class_spec::JSNativeWrapper {
                op: func,
                info: ptr::null(),
            },
            nargs: nargs as u16,
            flags,
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
            name: crate::class_spec::JSPropertySpec_Name {
                string_: name.as_ptr(),
            },
            attributes_: crate::class_spec::JSPROP_ENUMERATE,
            kind_: crate::class_spec::JSPropertySpec_Kind::NativeAccessor,
            u: crate::class_spec::JSPropertySpec_AccessorsOrValue {
                accessors: crate::class_spec::JSPropertySpec_AccessorsOrValue_Accessors {
                    getter: crate::class_spec::JSPropertySpec_Accessor {
                        native: crate::class_spec::JSNativeWrapper {
                            op: getter,
                            info: ptr::null(),
                        },
                    },
                    setter: crate::class_spec::JSPropertySpec_Accessor {
                        native: crate::class_spec::JSNativeWrapper {
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

/// Leaked per-type spec tables.
///
/// SpiderMonkey requires the method/property arrays to outlive the class.
/// They are identical for every global, so they are built and leaked once
/// per type and reused when the class is registered on further globals.
#[derive(Clone, Copy)]
struct SpecTables {
    methods: *const JSFunctionSpec,
    static_methods: *const JSFunctionSpec,
    properties: *const JSPropertySpec,
    static_properties: *const JSPropertySpec,
}

thread_local! {
    static SPEC_TABLES: RefCell<HashMap<TypeId, SpecTables>> = RefCell::new(HashMap::new());
}

fn spec_tables_for<T: ClassDef>() -> SpecTables {
    SPEC_TABLES.with(|tables| {
        if let Some(t) = tables.borrow().get(&TypeId::of::<T>()) {
            return *t;
        }

        let builder = ClassBuilder::<T>::new();
        let builder = T::register_class_methods(builder);
        let (methods, properties, _proto_constants) = builder.finalize();

        let static_builder = ClassBuilder::<T>::new();
        let static_builder = T::register_static_methods(static_builder);
        let (static_methods, static_properties, _static_constants) = static_builder.finalize();

        let t = SpecTables {
            methods: Box::leak(methods.into_boxed_slice()).as_ptr(),
            static_methods: Box::leak(static_methods.into_boxed_slice()).as_ptr(),
            properties: if properties.len() > 1 {
                Box::leak(properties.into_boxed_slice()).as_ptr()
            } else {
                ptr::null()
            },
            static_properties: if static_properties.len() > 1 {
                Box::leak(static_properties.into_boxed_slice()).as_ptr()
            } else {
                ptr::null()
            },
        };
        tables.borrow_mut().insert(TypeId::of::<T>(), t);
        t
    })
}

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
    scope: &'s Scope<'s>,
    global: Object<'s>,
) -> Object<'s> {
    // Idempotency check: if this class is already registered on this global,
    // return the existing prototype without re-registering.
    if let Some(proto) = get_prototype::<T>(scope.global()) {
        return Object::from_raw(scope, proto).expect("registered prototype is non-null");
    }

    // Ensure the parent class is registered before we register ourselves.
    T::ensure_parent_registered(scope, global);

    // The leaked method/property tables, shared across globals.
    let tables = spec_tables_for::<T>();

    // Build constructor constants
    let const_builder = ClassBuilder::<T>::new();
    let const_builder = T::register_constants(const_builder);
    let (_cm, _cp, constants) = const_builder.finalize();

    let class: &'static JSClass = T::class();

    // The constructor function's `length` is its number of required arguments
    // (WebIDL §3.7.1), carried from the `#[constructor]` by `ClassDef`.
    let ctor_nargs = T::constructor_nargs();

    // Register inheritance information (parent accessor functions)
    T::register_inheritance();
    let parent_proto = scope.root_object_mut(T::parent_prototype(scope));

    // Use init_class to set up constructor + prototype.
    // The class name comes from T::class().name (a static C string pointer).
    let proto = self::init_class(
        scope,
        global.handle(),
        class,
        parent_proto.handle(),
        class.name,
        Some(generic_constructor::<T>),
        ctor_nargs,
        tables.properties,
        tables.methods,
        tables.static_properties,
        tables.static_methods,
    )
    .expect("init_class failed");

    let ctor = Object::from_raw(
        scope,
        wrappers2::JS_GetConstructor(scope.cx_mut(), proto.handle()),
    )
    .expect("Constructor was just installed");

    register_prototype::<T>(global, proto.handle().get());

    // Define Symbol.toStringTag if the class specifies one.
    if !T::TO_STRING_TAG.is_empty() {
        define_to_string_tag(scope, proto.handle(), T::TO_STRING_TAG);
    }

    // Install constants on the constructor (and optionally the prototype).
    if !constants.is_empty() {
        let attrs = (crate::class_spec::JSPROP_READONLY
            | crate::class_spec::JSPROP_ENUMERATE
            | crate::class_spec::JSPROP_PERMANENT) as std::ffi::c_uint;
        for &(const_name, value) in &constants {
            ctor.define_property(scope, const_name, value, attrs)
                .expect("failed to define constant on constructor");
        }

        // WebIDL §3.7.3: constants are also defined on the prototype.
        if T::CONSTANTS_ON_PROTOTYPE {
            for &(const_name, value) in &constants {
                proto
                    .define_property(scope, const_name, value, attrs)
                    .expect("failed to define constant on prototype");
            }
        }
    }

    // WebIDL §3.7: an inherited interface's interface object has its
    // [[Prototype]] set to the inherited interface's interface object (so that
    // e.g. `Object.getPrototypeOf(CustomEvent) === Event`). `init_class` only
    // links the prototype chain, so set the constructor chain here.
    if T::INHERITS_INTERFACE && !parent_proto.is_null() {
        let parent_ctor = Object::from_raw(
            scope,
            wrappers2::JS_GetConstructor(scope.cx_mut(), parent_proto.handle()),
        )
        .expect("Parent constructor exists since parent_proto is non-null");
        ctor.set_prototype(scope, parent_ctor.handle())
            .expect("failed to set constructor prototype chain");
    }

    proto
}

/// Generic constructor callback for all ClassDef types.
///
/// This is the `JSNative` that gets called when `new MyClass(...)` is invoked in JS.
unsafe extern "C" fn generic_constructor<T: ClassDef>(
    cx: *mut RawJSContext,
    argc: u32,
    vp: *mut Value,
) -> bool {
    // SAFETY: SpiderMonkey guarantees cx is valid and a realm is entered during a native call.
    let scope = RootScope::from_current_realm(cx);
    let args = CallArgs::from_vp(vp, argc);

    if !args.is_constructing() {
        crate::error::throw_type_error(&scope, c"Constructor must be called with 'new'");
        return false;
    }

    if !T::has_js_constructor() {
        crate::error::throw_type_error(&scope, c"Illegal constructor");
        return false;
    }

    // Get the JSClass for this type
    let class: &'static JSClass = T::class();

    // Create the new JS object using the constructor's prototype
    let obj = match Object::from_raw(
        &scope,
        crate::class_spec::JS_NewObjectForConstructor(cx, class, &args),
    ) {
        Some(obj) => obj,
        None => return false,
    };

    // Call the Rust constructor
    match T::constructor(&scope, &args) {
        Ok(instance) => {
            unsafe { set_private(obj.handle().get(), instance) };

            // Post-construction hook: runs after the private data is at its
            // final heap location, so GC write barriers are registered at
            // stable addresses.
            if T::post_init(&scope, obj, &args).is_err() {
                return false;
            }

            // Install `[LegacyUnforgeable]` own accessors (e.g. Event.isTrusted).
            if T::install_unforgeable(&scope, obj).is_err() {
                return false;
            }

            #[cfg(debug_assertions)]
            if let Some(data) = get_private::<T>(obj.handle().get()) {
                data.debug_assert_fully_initialized();
            }

            // For error-data classes, capture the current stack.
            if T::HAS_ERROR_DATA {
                capture_stack_from_error(&scope, &obj);
            }

            args.rval()
                .set(unsafe { value::from_object(obj.handle().get()) });
            true
        }
        Err(ExnThrown) => false,
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

/// Shared core of the `get_*arg*` extractors: optionally require the
/// argument's presence, then convert it via `FromJSVal` with the given
/// config, throwing a `TypeError` for conversion failures.
///
/// # Safety
///
/// - `scope` must be in a valid realm.
/// - `args` must be from a valid JSNative call.
unsafe fn extract_arg<'s, 'v, T: FromJSVal<'s, 'v>>(
    scope: &'s Scope<'s>,
    args: &CallArgs,
    index: u32,
    required: bool,
    config: T::Config,
) -> Result<T, ExnThrown> {
    if required && index >= args.argc_ {
        return Err(crate::error::throw_type_error(
            scope,
            c"Not enough arguments",
        ));
    }
    // `CallArgs::get` yields `undefined` for an out-of-range index.
    let val = crate::native::Handle::from_raw(args.get(index));
    T::from_jsval(scope, val, config).map_err(|e| match e {
        ConversionError::ExnPending => ExnThrown,
        ConversionError::Failure(msg) => crate::error::throw_type_error(scope, &msg),
    })
}

/// Extract an argument from CallArgs and convert it to the desired Rust type.
///
/// Throws "Not enough arguments" if the argument is missing.
///
/// # Safety
///
/// - `scope` must be in a valid realm.
/// - `args` must be from a valid JSNative call.
pub unsafe fn get_arg<'s, 'v, T: FromJSVal<'s, 'v, Config = ()>>(
    scope: &'s Scope<'s>,
    args: &CallArgs,
    index: u32,
) -> Result<T, ExnThrown> {
    extract_arg(scope, args, index, true, ())
}

/// Extract an argument like [`get_arg`], but treat a missing argument as
/// `undefined` rather than throwing "Not enough arguments".
///
/// Used for optional `any` parameters (typed `HandleValue`/`Value`), where a
/// missing argument is indistinguishable from an explicit `undefined` and both
/// are valid values.
///
/// # Safety
///
/// - `scope` must be in a valid realm.
/// - `args` must be from a valid JSNative call.
pub unsafe fn get_arg_or_undefined<'s, 'v, T: FromJSVal<'s, 'v, Config = ()>>(
    scope: &'s Scope<'s>,
    args: &CallArgs,
    index: u32,
) -> Result<T, ExnThrown> {
    extract_arg(scope, args, index, false, ())
}

/// Extract an integer argument with configurable conversion behavior.
///
/// # Safety
///
/// - `scope` must be in a valid realm.
/// - `args` must be from a valid JSNative call.
pub unsafe fn get_int_arg<'s, 'v, T: FromJSVal<'s, 'v, Config = ConversionBehavior>>(
    scope: &'s Scope<'s>,
    args: &CallArgs,
    index: u32,
    behavior: ConversionBehavior,
) -> Result<T, ExnThrown> {
    extract_arg(scope, args, index, true, behavior)
}

/// Extract an argument with an explicit conversion config.
///
/// Used for container types like `Vec<T>` and `Record<V>` whose
/// `FromJSVal::Config` forwards the inner type's config. When the inner
/// type is an integer, the config is `ConversionBehavior`; for other
/// inner types it is `()`.
///
/// # Safety
///
/// - `scope` must be in a valid realm.
/// - `args` must be from a valid JSNative call.
#[doc(hidden)]
pub unsafe fn get_arg_with_config<'s, 'v, T: FromJSVal<'s, 'v>>(
    scope: &'s Scope<'s>,
    args: &CallArgs,
    index: u32,
    config: T::Config,
) -> Result<T, ExnThrown> {
    extract_arg(scope, args, index, true, config)
}

/// Extract a stack newtype argument from CallArgs.
///
/// Verifies that the argument is an object with the correct class, roots it,
/// and wraps it in the stack newtype `T`. Used by generated constructor and
/// method trampolines for parameters typed as stack newtypes (e.g. `Item<'_>`).
///
/// # Safety
///
/// - `scope` must be in a valid realm.
/// - `args` must be from a valid JSNative call.
#[doc(hidden)]
pub unsafe fn get_stack_arg<'s, T: StackType<'s>>(
    scope: &'s Scope<'_>,
    args: &CallArgs,
    index: u32,
) -> Result<T, ExnThrown> {
    if index >= args.argc_ {
        return Err(crate::error::throw_type_error(
            scope,
            c"Not enough arguments",
        ));
    }
    let val = *args.get(index);
    if !val.is_object() {
        let msg = CString::new(format!(
            "argument {} is not an instance of {}",
            index,
            T::Inner::NAME,
        ))
        .unwrap_or_else(|_| c"argument is not an object".into());
        return Err(crate::error::throw_type_error(scope, &msg));
    }
    let obj = val.to_object();
    let concrete_tag = crate::object::get_object_class(obj) as usize;
    let target_tag = class_tag::<T::Inner>();
    // Reject builtins' prototype objects, which match the class tag but carry no private data.
    if !is_derived_from_type(concrete_tag, target_tag) || get_raw_private(obj).is_none() {
        let msg = CString::new(format!(
            "argument {} is not an instance of {}",
            index,
            T::Inner::NAME,
        ))
        .unwrap_or_else(|_| c"argument is not the expected class".into());
        return Err(crate::error::throw_type_error(scope, &msg));
    }
    let nn = NonNull::new(obj).unwrap();
    Ok(unsafe { T::from_handle_unchecked(scope.root_object(nn)) })
}

/// Resolve a WebIDL operation's receiver object from the call's `this`.
///
/// Returns the `this` value as an object, except that — per WebIDL `[Global]`
/// operation semantics — a null or undefined `this` resolves to the global
/// object when the global implements `target_tag`. This is what lets a bare,
/// unqualified `addEventListener('fetch', …)` in a ServiceWorker script reach
/// the global's own `EventTarget` data: an unqualified sloppy-mode call passes
/// `this = undefined`, and SpiderMonkey performs no global substitution for
/// native callees. The substitution is gated on the same inheritance registry
/// the brand check uses, so for any interface the global does not implement, a
/// non-object `this` is rejected exactly as before.
///
/// # Safety
///
/// - `args` must be from a valid `JSNative` call.
unsafe fn resolve_webidl_this(
    scope: &Scope<'_>,
    args: &CallArgs,
    target_tag: usize,
) -> Result<*mut JSObject, ExnThrown> {
    let this_val = args.thisv();
    if this_val.is_object() {
        return Ok(this_val.to_object());
    }
    if this_val.is_null_or_undefined() {
        let global = scope.global().as_raw();
        if is_derived_from_type(crate::object::get_object_class(global) as usize, target_tag) {
            return Ok(global);
        }
    }
    Err(crate::error::throw_type_error(
        scope,
        c"'this' is not an object",
    ))
}

/// Extract the `this` object's private data in a method callback.
///
/// This is used inside `JSNative` methods to get the Rust struct backing `this`.
///
/// # Safety
///
/// - The CallArgs must be from a valid JSNative call.
/// - The `this` object must have private data of type `T`.
pub unsafe fn get_this_data<'a, T: ClassDef>(
    scope: &'a Scope<'_>,
    args: &CallArgs,
) -> Result<Ref<'a, T>, ExnThrown> {
    let obj = resolve_webidl_this(scope, args, class_tag::<T>())?;
    let Some(flag) = get_borrow_flag(obj) else {
        return Err(crate::error::throw_type_error(scope, T::NOT_TYPE_ERROR));
    };
    acquire_shared(flag);
    match get_private_or_ancestor::<T>(obj) {
        Some(value) => Ok(Ref { value, flag }),
        None => {
            flag.set(flag.get() - 1);
            Err(crate::error::throw_type_error(scope, T::NOT_TYPE_ERROR))
        }
    }
}

/// Extract the `this` object's private data mutably.
///
/// This is used inside `JSNative` methods to mutably get the Rust struct backing `this`.
///
/// # Safety
///
/// Same as [`get_this_data`], plus no other references to the data may exist.
pub unsafe fn get_this_data_mut<'a, T: ClassDef>(
    scope: &'a Scope<'_>,
    args: &CallArgs,
) -> Result<RefMut<'a, T>, ExnThrown> {
    let obj = resolve_webidl_this(scope, args, class_tag::<T>())?;
    let Some(flag) = get_borrow_flag(obj) else {
        return Err(crate::error::throw_type_error(scope, T::NOT_TYPE_ERROR));
    };
    acquire_mut(flag);
    match get_private_or_ancestor_mut::<T>(obj) {
        Some(value) => Ok(RefMut { value, flag }),
        None => {
            flag.set(UNUSED);
            Err(crate::error::throw_type_error(scope, T::NOT_TYPE_ERROR))
        }
    }
}

/// Extract the `this` object as a rooted stack newtype in a method callback.
///
/// Unlike [`get_this_data`] which returns `&T` (a reference to the private data),
/// this returns the full stack newtype `T` (e.g. `DOMException<'s>`), giving
/// access to both the JS object handle and the private data via `data()`/`data_mut()`.
///
/// # Safety
///
/// - The CallArgs must be from a valid JSNative call.
#[doc(hidden)]
pub unsafe fn get_this<'s, T: StackType<'s>>(
    scope: &'s Scope<'s>,
    args: &CallArgs,
) -> Result<T, ExnThrown>
where
    T::Inner: ClassDef,
{
    let obj = resolve_webidl_this(scope, args, class_tag::<T::Inner>())?;
    let concrete_tag = crate::object::get_object_class(obj) as usize;
    let target_tag = class_tag::<T::Inner>();
    if !is_derived_from_type(concrete_tag, target_tag) {
        return Err(crate::error::throw_type_error(
            scope,
            T::Inner::NOT_TYPE_ERROR,
        ));
    }
    // The prototype object shares the same JSClass as instances but has no
    // private data (set_private is only called during construction). Reject
    // it here so callers can safely use data()/data_mut() via unwrap_unchecked.
    if get_private::<T::Inner>(obj).is_none() {
        return Err(crate::error::throw_type_error(
            scope,
            T::Inner::PROTOTYPE_CALL_ERROR,
        ));
    }
    let nn = NonNull::new(obj).unwrap();
    Ok(unsafe { T::from_handle_unchecked(scope.root_object(nn)) })
}

/// Set the return value of a JSNative callback.
///
/// On conversion failure the error is thrown as a pending exception and `false` is returned.
///
/// # Safety
///
/// - `cx` and `args` must be from a valid JSNative call.
pub unsafe fn set_return<'s, T: ToJSVal<'s>>(
    scope: &'s Scope<'s>,
    args: &CallArgs,
    value: &T,
) -> bool {
    match value.to_jsval(scope) {
        Ok(val) => {
            args.rval().set(val.get());
            true
        }
        Err(e) => {
            e.throw(scope);
            false
        }
    }
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
        #[crate::allow_unrooted_interior]
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
autoref_reg!(
    __PostInitReg,
    "Autoref specialization helper for post-construction initialization."
);
autoref_reg!(
    __UnforgeableReg,
    "Autoref specialization helper for [LegacyUnforgeable] accessor installation."
);

/// Trait for `[LegacyUnforgeable]` accessor installation via autoref
/// specialization. The blanket impl on `&__UnforgeableReg<T>` is a no-op;
/// `#[jsmethods]` provides the real impl on `__UnforgeableReg<T>` when an
/// interface declares `#[getter(unforgeable)]`.
#[doc(hidden)]
pub trait __UnforgeableRegistrar<T: ClassDef> {
    fn install(&self, scope: &Scope<'_>, obj: Object<'_>) -> Result<(), ExnThrown>;
}

impl<T: ClassDef> __UnforgeableRegistrar<T> for &__UnforgeableReg<T> {
    fn install(&self, _scope: &Scope<'_>, _obj: Object<'_>) -> Result<(), ExnThrown> {
        Ok(())
    }
}

/// Trait for constructor registration via autoref specialization.
/// The blanket impl on `&__CtorReg<T>` panics; `#[jsmethods]` provides
/// the real impl on `__CtorReg<T>` directly.
#[doc(hidden)]
pub trait __ConstructorRegistrar<T: ClassDef> {
    fn construct(&self, scope: &Scope<'_>, args: &CallArgs) -> Result<T, ExnThrown>;

    /// The number of required (non-optional, non-variadic) constructor
    /// arguments, used as the constructor function's `length`. The `#[jsmethods]`
    /// impl on `__CtorReg<T>` overrides this.
    fn nargs(&self) -> u32 {
        0
    }

    /// Whether the `#[jsmethods]` block declares an explicit `#[constructor]`.
    /// When `false`, the class is not constructible from JS (`new Foo()`
    /// throws "Illegal constructor"). Rust-side factories (`Foo::new(scope)`)
    /// are unaffected.
    fn defined(&self) -> bool {
        false
    }
}

impl<T: ClassDef> __ConstructorRegistrar<T> for &__CtorReg<T> {
    fn construct(&self, _scope: &Scope<'_>, _args: &CallArgs) -> Result<T, ExnThrown> {
        panic!("No #[constructor] defined. Use #[jsmethods] with #[constructor] to define one.");
    }

    fn nargs(&self) -> u32 {
        0
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

/// Trait for post-construction initialization via autoref specialization.
/// The blanket impl on `&__PostInitReg<T>` is a no-op; `#[jsmethods]` provides
/// the real impl on `__PostInitReg<T>` directly when `#[post_init]` is used.
#[doc(hidden)]
pub trait __PostInitRegistrar<T: ClassDef> {
    fn post_init(
        &self,
        scope: &Scope<'_>,
        obj: Object<'_>,
        args: &CallArgs,
    ) -> Result<(), ExnThrown>;
}

impl<T: ClassDef> __PostInitRegistrar<T> for &__PostInitReg<T> {
    fn post_init(
        &self,
        _scope: &Scope<'_>,
        _obj: Object<'_>,
        _args: &CallArgs,
    ) -> Result<(), ExnThrown> {
        Ok(())
    }
}

/// Create a JS object backed by a Rust value constructed by `init`.
///
/// The closure receives the newly allocated (but empty) JS object and
/// returns the Rust data to store in it. Because the JS object is
/// allocated *before* `init` runs, any `Heap<U>` fields created inside
/// the closure are safe from GC hazards: the object is already rooted.
pub fn create_instance_with<'s, T: ClassDef>(
    scope: &'s Scope<'_>,
    init: impl FnOnce(Object<'s>) -> T,
) -> Result<T::Rooted<'s>, ExnThrown> {
    try_create_instance_with(scope, |obj| Ok(init(obj)))
}

/// Fallible variant of [`create_instance_with`]: when `init` fails, its error
/// propagates and the freshly allocated object is discarded without private
/// data.
pub fn try_create_instance_with<'s, T: ClassDef>(
    scope: &'s Scope<'_>,
    init: impl FnOnce(Object<'s>) -> Result<T, ExnThrown>,
) -> Result<T::Rooted<'s>, ExnThrown> {
    let global = scope.global();
    let proto = match get_prototype::<T>(global) {
        // SAFETY: builtins' prototypes are always valid objects.
        Some(p) => unsafe { Object::from_raw(scope, p) }.unwrap(),
        None => return Err(ExnThrown), // TODO: Actually throw an error here.
    };

    let class = T::class();
    let obj = Object::new_with_proto(scope, class, proto)?;
    let data = init(obj)?;
    unsafe {
        set_private(obj.as_raw(), data);
    }
    // SAFETY: The handle points to a just-created object backed by `T`.
    let stack = unsafe { Stack::<T>::from_handle_unchecked(obj.handle()) };
    Ok(T::Rooted::from(stack))
}

// ---------------------------------------------------------------------------
// Per-global shared objects and functions
// ---------------------------------------------------------------------------

/// Get a per-global shared object, keyed by `key` (by convention a native
/// function pointer unique to the call site, cast to `usize`), creating it via
/// `init` and caching it on first use.
///
/// The cached object is traced and finalized with the global, so every call on
/// a given global returns the same object. Use this for internal objects whose
/// per-call identity does not matter and that are reused indefinitely, such as
/// a resolved promise serving as a microtask trigger. The key namespace is
/// shared with [`get_or_init_shared_function`].
pub fn get_or_init_shared_object<'s>(
    scope: &'s Scope<'_>,
    key: usize,
    init: impl FnOnce(&'s Scope<'_>) -> Result<Object<'s>, ExnThrown>,
) -> Result<Object<'s>, ExnThrown> {
    // Scope the registry borrow to the lookup so nothing under `init` can cause a second borrow.
    let cached = unsafe {
        get_class_registry(scope.global().as_raw()).and_then(|r| r.get_shared_object(key))
    };
    if let Some(o) = cached {
        // SAFETY: `o` is a live object cached for this global and kept alive by
        // the registry's tracer.
        if let Some(obj) = unsafe { Object::from_raw(scope, o) } {
            return Ok(obj);
        }
    }
    let obj = init(scope)?;
    let registry = unsafe { get_or_init_class_registry(scope.global().as_raw()) };
    registry.set_shared_object(key, obj.as_raw());
    Ok(obj)
}

/// Get a per-global shared function, keyed by `key` (typically the native
/// callback's function pointer cast to `usize`), creating it via `init` and
/// caching it on first use.
///
/// The cached `JSFunction` is traced and finalized with the global, so every
/// call on a given global returns the same function object. Use this where a
/// function's identity must be stable per global, such as WebIDL
/// `[LegacyUnforgeable]` getters, or the streams spec's queuing-strategy
/// `size` functions ("the relevant global object's … size function").
pub fn get_or_init_shared_function<'s>(
    scope: &'s Scope<'_>,
    key: usize,
    init: impl FnOnce(&'s Scope<'_>) -> Result<crate::Function<'s>, ExnThrown>,
) -> Result<crate::Function<'s>, ExnThrown> {
    let obj = get_or_init_shared_object(scope, key, |scope| init(scope).map(|f| *f))?;
    // SAFETY: the cached object was created by `init`, so it is a JSFunction.
    unsafe { crate::Function::from_raw(scope, obj.as_raw()) }.ok_or(ExnThrown)
}

/// Read-and-increment a per-global monotonic `u64` counter keyed by `key`.
///
/// Returns the counter's current value and advances it (wrapping on overflow);
/// counters start at 0. The counter lives on the global's [`ClassRegistry`], so
/// every caller on a given global draws from one sequence — use this for ids a
/// spec requires to be unique per global rather than per call site, such as
/// HTML's setTimeout/setInterval ids, which must not collide across the
/// concurrent event loops that share a single global.
///
/// `key` is an arbitrary namespace token; pass the address of a dedicated
/// `static` to get a process-unique key, mirroring how shared functions key on
/// their native pointer.
///
/// Note: the counter is a u64 instead of a u32 because otherwise it'd be possible
/// (though implausible) for a long-running script to wrap the counter and reuse ids,
/// which would violate the uniqueness guarantee. We convert the id to a JS value,
/// which can represent about 53 bits of integer precision, making wrap-around
/// effectively impossible.
pub fn next_global_counter(scope: &Scope<'_>, key: usize) -> u64 {
    // The bump is a HashMap entry plus an integer add — no JS-GC-capable call
    // in between — so briefly holding the `&mut ClassRegistry` aliases nothing.
    let registry = unsafe { get_or_init_class_registry(scope.global().as_raw()) };
    let counter = registry.bump_counter(key);
    assert!(
        counter < crate::conversion::Number::MAX,
        "counter for key {key} wrapped around"
    );
    counter
}

// ---------------------------------------------------------------------------
// LegacyUnforgeable accessors
// ---------------------------------------------------------------------------

/// Define a `[LegacyUnforgeable]` read-only accessor as an own, enumerable,
/// non-configurable property on `obj`, using a getter function shared across
/// all instances (cached per global, keyed by the native pointer).
///
/// Used by the proc-macro-generated `ClassDef::install_unforgeable`.
pub fn define_unforgeable_accessor(
    scope: &Scope<'_>,
    obj: Object<'_>,
    name: &std::ffi::CStr,
    getter: crate::native::JSNative,
) -> Result<(), ExnThrown> {
    let key = getter.map(|f| f as usize).unwrap_or(0);
    let name_str = name.to_str().map_err(|_| ExnThrown)?;

    // Reuse the cached getter if present; otherwise create it once and store it.
    let getter_fn = get_or_init_shared_function(scope, key, |scope| {
        // WebIDL names accessor getter functions "get <attribute>".
        let getter_name =
            std::ffi::CString::new(format!("get {name_str}")).map_err(|_| ExnThrown)?;
        crate::Function::new(scope, getter, 0, 0, &getter_name)
    })?;

    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let desc = crate::native::PropertyDescriptor {
        _bitfield_align_1: [0; 0],
        _bitfield_1: crate::native::PropertyDescriptor::new_bitfield_1(
            true,  // hasConfigurable
            false, // configurable (unforgeable)
            true,  // hasEnumerable
            true,  // enumerable
            false, // hasWritable
            false, // writable
            false, // hasValue
            true,  // hasGetter
            false, // hasSetter
            false, // resolving
        ),
        getter_: getter_fn.as_raw(),
        setter_: ptr::null_mut(),
        value_: mozjs::jsval::UndefinedValue(),
    });

    let js_str = crate::string::Str::from_str(scope, name_str)?;
    let id = crate::id::string_to_id(scope, js_str.handle())?;
    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let id_rooted = id);
    obj.define_property_by_id(scope, id_rooted.handle(), desc.handle())
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
    scope: &Scope<'_>,
    proto: crate::native::GCHandle<'_, *mut JSObject>,
    tag_value: &str,
) {
    let tag_key = crate::symbol::get_well_known_key(scope, SymbolCode::toStringTag);
    let tag_str = crate::string::Str::from_str(scope, tag_value)
        .expect("failed to create toStringTag string");
    // SAFETY: tag_str is a live JSString* from `from_str` above, valid in the current scope.
    let str_val = unsafe { value::from_string_raw(tag_str.as_raw()) };

    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let desc = crate::native::PropertyDescriptor {
        _bitfield_align_1: [0; 0],
        _bitfield_1: crate::native::PropertyDescriptor::new_bitfield_1(
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

    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let tag_id = tag_key);
    let proto_obj = Object::from_handle(proto).expect("prototype is null");
    proto_obj
        .define_property_by_id(scope, tag_id.handle(), desc.handle())
        .expect("failed to define Symbol.toStringTag");
}

/// Define a data property keyed by a well-known symbol on `obj` (typically a
/// prototype), with the attributes WebIDL gives symbol-keyed prototype
/// members: writable, non-enumerable, configurable.
///
/// Used for symbol-keyed method aliases, such as an `async iterable<>`
/// declaration installing `@@asyncIterator` as an alias of the `values`
/// method (WebIDL §3.7.10.2).
pub fn define_well_known_symbol_property(
    scope: &Scope<'_>,
    obj: Object<'_>,
    which: SymbolCode,
    value: crate::prelude::HandleValue<'_>,
) -> Result<(), ExnThrown> {
    let key = crate::symbol::get_well_known_key(scope, which);

    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let desc = crate::native::PropertyDescriptor {
        _bitfield_align_1: [0; 0],
        _bitfield_1: crate::native::PropertyDescriptor::new_bitfield_1(
            true,  // hasConfigurable
            true,  // configurable
            true,  // hasEnumerable
            false, // enumerable
            true,  // hasWritable
            true,  // writable
            true,  // hasValue
            false, // hasGetter
            false, // hasSetter
            false, // resolving
        ),
        getter_: ptr::null_mut(),
        setter_: ptr::null_mut(),
        value_: value.get(),
    });

    rooted!(in(unsafe { scope.raw_cx_no_gc() }) let id = key);
    obj.define_property_by_id(scope, id.handle(), desc.handle())
}

/// Adds an alias to the property given by `property` under the symbol `symbol` to the
/// prototype of the builtin class `T`.
pub fn add_symbol_alias<T: ClassDef>(scope: &Scope, property: &CStr, symbol: SymbolCode) {
    let proto = get_prototype_object_for::<T>(scope).expect("prototype not found");
    let values = proto
        .get_property(scope, property)
        .expect("property not found");
    let _ = define_well_known_symbol_property(scope, proto, symbol, values);
}
