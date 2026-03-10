// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Typed handles for scope-rooted and heap-traced JS object references.
//!
//! [`Stack<'s, T>`] ties a JS object reference to a rooting scope,
//! preventing the GC from collecting it. [`Heap<T>`] stores a GC-traced
//! reference that can outlive any particular rooting scope.
//!
//! Both types are parameterized by a [`JsType`] marker that carries
//! static type information about the kind of JS object.

use std::marker::PhantomData;
use std::ptr::NonNull;

use crate::gc::scope::Scope;
use crate::heap::{Heap as MozHeap, Trace};
use crate::native::{JSObject, JSTracer};
use mozjs::gc::Handle;

// ============================================================================
// JsType — marker trait for typed JS object handles
// ============================================================================

/// Marker trait for types representable as JavaScript objects.
///
/// Implemented by:
/// - Builtin marker types (`object::Object`, `array::Array`, `promise::Promise`, etc.)
/// - `ClassDef` types (user-defined classes with Rust data in private slots)
///
/// `JsType` is the bound for [`Stack`] and [`Heap`], the universal wrappers
/// for scope-rooted and heap-traced JS object handles.
pub trait JsType: 'static {
    /// The JavaScript-visible name of this type (e.g. `"Object"`, `"Array"`, `"Counter"`).
    const JS_NAME: &'static str;
}

// ============================================================================
// Stack<'s, T> — scope-rooted handle
// ============================================================================

/// A scope-rooted handle to a JavaScript object of type `T`.
///
/// `Stack<'s, T>` ties a JS object reference to a rooting scope `'s`,
/// preventing the GC from collecting it. The type parameter `T` carries
/// static type information about what kind of JS object this is.
///
/// Methods are partitioned by trait bound:
/// - `T: JsType` — handle access, `as_value()`, `as_raw()`
/// - `T: ClassDef` — `data()`, `data_mut()`, `cast()` (defined in [`class`](crate::class))
/// - Concrete `T` — type-specific methods (e.g. `Object<'s>` has property accessors)
#[repr(transparent)]
pub struct Stack<'s, T: JsType> {
    pub(crate) handle: Handle<'s, *mut JSObject>,
    pub(crate) _marker: PhantomData<T>,
}

// Manual Clone/Copy impls: T lives only in PhantomData, so Stack is always
// Copy regardless of whether T is Copy.
impl<'s, T: JsType> Clone for Stack<'s, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'s, T: JsType> Copy for Stack<'s, T> {}

impl<'s, T: JsType> Stack<'s, T> {
    /// Wrap an existing rooted handle.
    ///
    /// # Safety
    ///
    /// The handle must point to a JS object of type `T` (or a subtype).
    pub unsafe fn from_handle_unchecked(handle: Handle<'s, *mut JSObject>) -> Self {
        Stack {
            handle,
            _marker: PhantomData,
        }
    }

    /// Get the underlying rooted handle.
    pub fn handle(self) -> Handle<'s, *mut JSObject> {
        self.handle
    }

    /// Get the raw `*mut JSObject` pointer.
    pub fn as_raw(self) -> *mut JSObject {
        self.handle.get()
    }

    /// Get the JS value representation.
    ///
    /// # Safety
    ///
    /// The handle is rooted, so the object is guaranteed to be alive.
    pub fn as_value(self) -> crate::native::Value {
        unsafe { crate::value::from_object(self.handle.get()) }
    }

    /// Root a raw pointer in the given scope.
    ///
    /// Returns `None` if `ptr` is null.
    ///
    /// # Safety
    ///
    /// `ptr` must point to a JS object of type `T` (or a subtype).
    pub unsafe fn from_raw(scope: &'s Scope<'_>, ptr: *mut JSObject) -> Option<Self> {
        NonNull::new(ptr).map(|nn| Stack {
            handle: scope.root_object(nn),
            _marker: PhantomData,
        })
    }
}

impl<'s, T: JsType> std::fmt::Debug for Stack<'s, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stack")
            .field("type", &T::JS_NAME)
            .field("ptr", &self.handle.get())
            .finish()
    }
}

// ToJSValConvertible: allows returning Stack newtypes from methods/getters.
impl<'s, T: JsType> mozjs::conversions::ToJSValConvertible for Stack<'s, T> {
    #[inline]
    unsafe fn to_jsval(
        &self,
        _cx: *mut mozjs::jsapi::JSContext,
        mut rval: mozjs::gc::MutableHandle<'_, mozjs::jsapi::Value>,
    ) {
        rval.set(crate::value::from_object(self.as_raw()));
    }
}

// ============================================================================
// Heap<T> — GC-traced heap reference
// ============================================================================

/// A GC-traced heap reference to a JavaScript object of type `T`.
///
/// `Heap<T>` stores a JS object pointer that is traced by the garbage
/// collector, allowing it to outlive any particular rooting scope. Use
/// this to store JS objects inside Rust structs that are themselves traced.
///
/// To access the object, root it with [`get`](Heap::get), which returns
/// the stack newtype directly (e.g. `Item<'s>` for `Heap<ItemImpl>`).
#[crate::must_root]
pub struct Heap<T: JsType> {
    heap: MozHeap<*mut JSObject>,
    _marker: PhantomData<T>,
}

#[crate::allow_unrooted]
impl<T: JsType> Heap<T> {
    /// Create from a raw JS object pointer.
    ///
    /// # Safety
    ///
    /// `obj` must be a valid, non-null JS object of type `T`.
    pub unsafe fn from_raw(obj: *mut JSObject) -> Self {
        debug_assert!(!obj.is_null(), "Heap::from_raw called with null pointer");
        let heap = MozHeap::default();
        heap.set(obj);
        Heap {
            heap,
            _marker: PhantomData,
        }
    }

    /// Root the referenced object, returning a stack-rooted handle.
    ///
    /// The return type is inferred from context. For user-defined classes
    /// with a stack newtype (e.g. `Item<'s>`), annotate the binding:
    ///
    /// ```ignore
    /// let item: Item<'_> = heap_ref.get(scope);
    /// ```
    ///
    /// Without annotation, returns `Stack<'s, T>`.
    pub fn get<'s, N: From<Stack<'s, T>>>(&self, scope: &'s Scope<'_>) -> N {
        let obj = self.heap.get();
        debug_assert!(!obj.is_null(), "Heap::get: null JS object");
        let handle = scope.root_object(NonNull::new(obj).expect("Heap contains null"));
        N::from(Stack {
            handle,
            _marker: PhantomData,
        })
    }
}

impl<'s, T: JsType> From<Stack<'s, T>> for Heap<T> {
    #[crate::allow_unrooted]
    fn from(stack: Stack<'s, T>) -> Self {
        let heap = MozHeap::default();
        heap.set(stack.as_raw());
        Heap {
            heap,
            _marker: PhantomData,
        }
    }
}

#[crate::allow_unrooted_interior]
unsafe impl<T: JsType> Trace for Heap<T> {
    #[inline]
    unsafe fn trace(&self, trc: *mut JSTracer) {
        self.heap.trace(trc);
    }
}
