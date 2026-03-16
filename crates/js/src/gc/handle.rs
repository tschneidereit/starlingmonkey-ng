// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Typed handles for scope-rooted and heap-traced JS object references.
//!
//! [`Stack<'s, T>`] ties a JS object reference to a rooting scope,
//! preventing the GC from collecting it. [`Heap<T>`] stores a GC-traced
//! reference that can outlive any particular rooting scope.
//!
//! Both types are parameterized by a [`JSType`] marker that carries
//! static type information about the kind of JS object.

use std::marker::PhantomData;
use std::ptr::NonNull;

use crate::builtins::{get_class_tag, is_derived_from_type, CastError, CastTarget, JSType};
use crate::conversion::{ConversionError, ToJSVal};
use crate::gc::scope::Scope;
use crate::heap::{Heap as MozHeap, Trace};
use crate::native::{JSObject, JSTracer, RawHandle};
use mozjs::gc::{Handle, HandleValue};

/// A scope-rooted handle to a JavaScript object of type `T`.
///
/// `Stack<'s, T>` ties a JS object reference to a rooting scope `'s`,
/// preventing the GC from collecting it. The type parameter `T` carries
/// static type information about what kind of JS object this is.
///
/// Methods are partitioned by trait bound:
/// - `T: JSType` — handle access, `as_value()`, `as_raw()`
/// - `T: ClassDef` — `data()`, `data_mut()`, `cast()` (defined in [`class`](crate::class))
/// - Concrete `T` — type-specific methods (e.g. `Object<'s>` has property accessors)
#[repr(transparent)]
pub struct Stack<'s, T: JSType> {
    pub(crate) handle: Handle<'s, *mut JSObject>,
    pub(crate) _marker: PhantomData<T>,
}

impl<'s, T: JSType> Stack<'s, T> {
    /// Check whether `self` is an instance of the target type `U`.
    ///
    /// Accepts both builtin marker types (`Date`, `Array`, …) and
    /// proc-macro newtypes (`Dog<'s>`, etc.) as the target.
    pub fn is<U: CastTarget<'s>>(&self) -> bool {
        let concrete_tag = unsafe { get_class_tag(self.as_raw()) };
        let target_tag = U::target_class_tag();
        is_derived_from_type(concrete_tag, target_tag)
    }

    /// Type-checked downcast to any supported target type.
    ///
    /// Reads the object's actual `JSClass` pointer at runtime and checks
    /// whether it matches `U` (or a subclass). Works for both builtin
    /// types (Array, Date, Promise, …) and proc-macro newtypes
    /// (Dog, Cat, …).
    ///
    /// ```ignore
    /// let date: Stack<'s, Date> = obj.cast::<Date>()?;
    /// let dog: Dog<'s> = obj.cast::<Dog>()?;
    /// ```
    pub fn cast<U: CastTarget<'s>>(self) -> Result<U::Output, CastError> {
        if !self.is::<U>() {
            return Err(CastError {
                from: T::JS_NAME,
                to: U::TARGET_NAME,
            });
        }
        Ok(unsafe { U::construct_unchecked(self.handle()) })
    }
}

// Manual Clone/Copy impls: T lives only in PhantomData, so Stack is always
// Copy regardless of whether T is Copy.
impl<'s, T: JSType> Clone for Stack<'s, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'s, T: JSType> Copy for Stack<'s, T> {}

impl<'s, T: JSType> Stack<'s, T> {
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

    /// Get the underlying raw (lifetime-erased) rooted handle.
    pub fn raw_handle(self) -> RawHandle<*mut JSObject> {
        self.handle.into()
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

impl<'s, T: JSType> std::fmt::Debug for Stack<'s, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stack")
            .field("type", &T::JS_NAME)
            .field("ptr", &self.handle.get())
            .finish()
    }
}

// ToJSValConvertible: allows returning Stack newtypes from methods/getters.
impl<'s, T: JSType> ToJSVal<'s> for Stack<'s, T> {
    #[inline]
    fn to_jsval(&self, scope: &Scope<'s>) -> Result<HandleValue<'s>, ConversionError> {
        Ok(scope.root_value(unsafe { crate::value::from_object(self.as_raw()) }))
    }
}

/// A GC-traced heap reference to a JavaScript object of type `T`.
///
/// `Heap<T>` stores a JS object pointer that is traced by the garbage
/// collector, allowing it to outlive any particular rooting scope. Use
/// this to store JS objects inside Rust structs that are themselves traced.
///
/// The inner `MozHeap` is boxed so that the GC write barrier address
/// remains stable when the `Heap` is moved (e.g. into a `Vec` or
/// `Option`). This makes `From<Stack<T>> for Heap<T>` safe and the overall
/// API much more ergonomic.
///
/// The downside is an extra level of indirection and heap allocation for
/// every `Heap`. In practice this cost is relatively small compared to the
/// cost of the JS objects being stored.
///
/// To access the object, root it with [`get`](Heap::get), which returns
/// the stack newtype directly (e.g. `Item<'s>` for `Heap<ItemImpl>`).
#[crate::must_root]
pub struct Heap<T: JSType> {
    heap: Box<MozHeap<*mut JSObject>>,
    _marker: PhantomData<T>,
}

#[crate::allow_unrooted]
impl<T: JSType> Heap<T> {
    /// Create from a raw JS object pointer.
    ///
    /// # Safety
    ///
    /// `obj` must be a valid, non-null JS object of type `T`.
    pub unsafe fn from_raw(obj: *mut JSObject) -> Self {
        debug_assert!(!obj.is_null(), "Heap::from_raw called with null pointer");
        let heap = Box::new(MozHeap::default());
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
    ///
    /// # Panics
    ///
    /// Panics if called on a default-constructed (null) `Heap`.
    pub fn get<'s, N: From<Stack<'s, T>>>(&self, scope: &'s Scope<'_>) -> N {
        let obj = self.heap.get();
        let handle = scope.root_object(
            NonNull::new(obj).expect("Heap::get called on a null (default-constructed) Heap"),
        );
        N::from(Stack {
            handle,
            _marker: PhantomData,
        })
    }

    /// Whether this heap reference has been initialized (is non-null).
    ///
    /// A default-constructed `Heap` is null; one created via `From<Stack>`
    /// or `from_raw` is always initialized. Once initialized, a `Heap`
    /// remains non-null for its entire lifetime.
    pub fn is_initialized(&self) -> bool {
        !self.heap.get().is_null()
    }
}

/// Default creates a null `Heap`. Only intended for use by the proc
/// macro's setup-style constructor path, which initializes all fields
/// before the object is exposed. Calling `get()` on a default `Heap`
/// panics.
#[crate::allow_unrooted]
impl<T: JSType> Default for Heap<T> {
    fn default() -> Self {
        Heap {
            heap: Box::new(MozHeap::default()),
            _marker: PhantomData,
        }
    }
}

/// Create a `Heap<T>` from a rooted `Stack<'s, T>`.
///
/// The inner `MozHeap` is boxed, so the GC write barrier address is
/// stable regardless of subsequent moves.
#[crate::allow_unrooted]
impl<'s, T: JSType> From<Stack<'s, T>> for Heap<T> {
    fn from(stack: Stack<'s, T>) -> Self {
        let heap = Box::new(MozHeap::default());
        heap.set(stack.as_raw());
        Heap {
            heap,
            _marker: PhantomData,
        }
    }
}

#[crate::allow_unrooted_interior]
unsafe impl<T: JSType> Trace for Heap<T> {
    #[inline]
    unsafe fn trace(&self, trc: *mut JSTracer) {
        self.heap.trace(trc);
    }
}
