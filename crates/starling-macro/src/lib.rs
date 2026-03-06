// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Proc macros for defining JavaScript classes backed by Rust structs in StarlingMonkey.
//!
//! Provides `#[jsclass]` and `#[jsmethods]` attribute macros that
//! generate the boilerplate needed to expose Rust types as SpiderMonkey JS classes.

use heck::{ToLowerCamelCase, ToUpperCamelCase};
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{
    parse_macro_input, Data, DeriveInput, Fields, FnArg, Ident, ImplItem, ImplItemFn, ItemImpl,
    ItemStruct, LitStr, Pat, ReturnType, Token, Type, Visibility,
};

// ============================================================================
// Attribute option parsing (shared across all macros)
// ============================================================================

/// Parsed key-value options from attribute arguments.
/// Used by `#[jsclass]`, `#[jsmethods]`, `#[jsmodule]`, and `#[method]`.
struct AttrOpts {
    name: Option<String>,
    extends: Option<Ident>,
    /// Inherit the prototype from a built-in JS class by `JSProtoKey`.
    ///
    /// `#[jsclass(js_proto = "Error")]` uses `Error.prototype` as the
    /// class prototype's `__proto__`. Mutually exclusive with `extends`.
    js_proto: Option<String>,
    /// Define `Symbol.toStringTag` on the prototype.
    ///
    /// `#[jsclass(to_string_tag = "DOMException")]` sets the well-known
    /// `@@toStringTag` property to the given string value (non-writable,
    /// non-enumerable, configurable — per WebIDL §3.7.6).
    to_string_tag: Option<String>,
}

impl Parse for AttrOpts {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut opts = Self {
            name: None,
            extends: None,
            js_proto: None,
            to_string_tag: None,
        };
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            let _: Token![=] = input.parse()?;
            match key.to_string().as_str() {
                "name" => opts.name = Some(input.parse::<LitStr>()?.value()),
                "extends" => opts.extends = Some(input.parse()?),
                "js_proto" => opts.js_proto = Some(input.parse::<LitStr>()?.value()),
                "to_string_tag" => opts.to_string_tag = Some(input.parse::<LitStr>()?.value()),
                _ => return Err(syn::Error::new(key.span(), "unknown option")),
            }
            if !input.is_empty() {
                let _: Token![,] = input.parse()?;
            }
        }
        // Validate: js_proto and extends are mutually exclusive
        if opts.js_proto.is_some() && opts.extends.is_some() {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "`js_proto` and `extends` are mutually exclusive",
            ));
        }
        Ok(opts)
    }
}

// ============================================================================
// #[jsclass] attribute macro
// ============================================================================

/// Attribute macro that derives `ClassDef` for a struct and generates a
/// stack newtype for ergonomic use.
///
/// Given `struct Foo { ... }`, this macro:
/// 1. Renames the data struct to `__FooInner` (hidden, implements `ClassDef`)
/// 2. Generates `Foo<'s>` — a `#[repr(transparent)]` newtype over `Handle<'s, *mut JSObject>`
///    matching the mozjs builtin pattern (like `Date<'s>`, `Array<'s>`)
/// 3. Generates `type FooRef = HeapRef<__FooInner>` for heap references
///
/// # Usage
///
/// ```rust,ignore
/// #[jsclass]
/// struct MyClass {
///     data: String,
/// }
/// // Generates:
/// //   __MyClassInner { data: String } — the data struct (ClassDef)
/// //   MyClass<'s>                     — stack newtype (Handle wrapper)
/// //   type MyClassRef                 — HeapRef<__MyClassInner>
/// ```
#[proc_macro_attribute]
pub fn jsclass(attr: TokenStream, item: TokenStream) -> TokenStream {
    let opts = parse_macro_input!(attr as AttrOpts);
    let input = parse_macro_input!(item as ItemStruct);
    process_class_def(opts, input, ClassConfig::JSCLASS)
}

/// Attribute macro for WebIDL interface definitions.
///
/// Identical to `#[jsclass]` but with WebIDL-specific defaults:
/// - `Symbol.toStringTag` is automatically set to the class name
///   (unless explicitly overridden via `to_string_tag = "..."`)
/// - `pub const` items in `#[jsmethods]` are installed on **both** the
///   constructor and the prototype (per WebIDL §3.7.3)
///
/// # Usage
///
/// ```rust,ignore
/// #[webidl_interface]
/// struct DOMException {
///     name: String,
///     message: String,
/// }
/// ```
#[proc_macro_attribute]
pub fn webidl_interface(attr: TokenStream, item: TokenStream) -> TokenStream {
    let opts = parse_macro_input!(attr as AttrOpts);
    let input = parse_macro_input!(item as ItemStruct);
    process_class_def(opts, input, ClassConfig::WEBIDL_INTERFACE)
}

// ============================================================================
// Class definition configuration
// ============================================================================

/// Controls codegen differences between `#[jsclass]` and `#[webidl_interface]`.
struct ClassConfig {
    /// When `true` and no explicit `to_string_tag` is set, automatically
    /// use the JS class name as `Symbol.toStringTag`.
    auto_to_string_tag: bool,
    /// When `true`, generate `const CONSTANTS_ON_PROTOTYPE: bool = true;`
    /// so that `pub const` items are installed on both constructor and
    /// prototype (per WebIDL §3.7.3).
    constants_on_prototype: bool,
}

impl ClassConfig {
    /// Configuration for plain `#[jsclass]`: no auto-tag, constants on
    /// constructor only.
    const JSCLASS: Self = Self {
        auto_to_string_tag: false,
        constants_on_prototype: false,
    };

    /// Configuration for `#[webidl_interface]`: auto Symbol.toStringTag,
    /// constants on both constructor and prototype.
    const WEBIDL_INTERFACE: Self = Self {
        auto_to_string_tag: true,
        constants_on_prototype: true,
    };
}

/// Shared implementation for `#[jsclass]` and `#[webidl_interface]`.
///
/// Processes the attributed struct and generates all ClassDef machinery,
/// stack newtypes, and heap reference wrappers.
fn process_class_def(opts: AttrOpts, mut input: ItemStruct, config: ClassConfig) -> TokenStream {
    let struct_name = input.ident.clone();
    let inner_name = format_ident!("__{}Inner", struct_name);
    let ref_alias = format_ident!("{}Ref", struct_name);
    let js_name = opts
        .name
        .unwrap_or_else(|| struct_name.to_string().to_upper_camel_case());

    // Generate identifiers for the static JSClass and JSClassOps
    let class_ops_static = format_ident!("__{}_CLASS_OPS", struct_name.to_string().to_uppercase());
    let class_static = format_ident!("__{}_CLASS", struct_name.to_string().to_uppercase());
    // Null-terminated byte string for the C class name
    let js_name_bytes = format!("{js_name}\0");
    let js_name_cstr_literal = proc_macro2::Literal::byte_string(js_name_bytes.as_bytes());

    // If extends is set, compute the inner parent name and rewrite the parent field type
    let opts_extends_ident = opts.extends.clone();
    let inner_parent = opts.extends.as_ref().map(|p| format_ident!("__{}Inner", p));

    if let Some(ref inner_parent_name) = inner_parent {
        // Rewrite the `parent` field's type from `Parent` to `__ParentInner`
        if let Fields::Named(ref mut fields) = input.fields {
            for field in &mut fields.named {
                if field.ident.as_ref().map(|i| i == "parent").unwrap_or(false) {
                    field.ty = syn::parse_quote! { #inner_parent_name };
                }
            }
        }
    }

    // Rename the struct to __FooInner and make it pub (since it's referenced
    // in the public API of the generated stack newtype and HeapRef wrapper).
    input.ident = inner_name.clone();
    input.vis = syn::Visibility::Public(syn::token::Pub::default());

    // Generate parent_prototype / register_inheritance / ensure_parent_registered
    // methods if extends or js_proto is set.
    let parent_classdef_methods = if let Some(ref inner_parent_name) = inner_parent {
        quote! {
            fn parent_prototype(scope: &::js::gc::scope::Scope<'_>) -> *mut ::js::native::JSObject {
                ::core_runtime::class::get_prototype_for::<#inner_parent_name>(scope)
                    .unwrap_or(::std::ptr::null_mut())
            }

            fn register_inheritance() {
                ::core_runtime::class::register_parent_info::<Self>();
            }

            fn ensure_parent_registered(
                scope: &::js::gc::scope::Scope<'_>,
                global: ::js::object::Object<'_>,
            ) {
                unsafe {
                    // SAFETY: register_class is safe to call if scope and global are valid.
                    ::core_runtime::class::register_class::<#inner_parent_name>(scope, global);
                }
            }
        }
    } else if let Some(ref proto_name) = opts.js_proto {
        // js_proto = "Error" → use the built-in JS prototype via JSProtoKey.
        let proto_key = format_ident!("JSProto_{}", proto_name);
        quote! {
            fn parent_prototype(scope: &::js::gc::scope::Scope<'_>) -> *mut ::js::native::JSObject {
                ::js::class::get_class_prototype(scope, ::js::class_spec::JSProtoKey::#proto_key)
                    .map(|h| h.get())
                    .unwrap_or(::std::ptr::null_mut())
            }
        }
    } else {
        quote! {}
    };

    // Generate TO_STRING_TAG const override.
    // Explicit `to_string_tag = "..."` always wins. Otherwise, when
    // `config.auto_to_string_tag` is true (webidl_interface), default
    // to the JS class name.
    let effective_tag = opts
        .to_string_tag
        .as_deref()
        .or({
            if config.auto_to_string_tag {
                Some(js_name.as_str())
            } else {
                None
            }
        })
        .map(|t| t.to_owned());
    let to_string_tag_const = if let Some(ref tag) = effective_tag {
        quote! {
            const TO_STRING_TAG: &'static str = #tag;
        }
    } else {
        quote! {}
    };

    // Generate CONSTANTS_ON_PROTOTYPE override for webidl_interface.
    let constants_on_prototype_const = if config.constants_on_prototype {
        quote! {
            const CONSTANTS_ON_PROTOTYPE: bool = true;
        }
    } else {
        quote! {}
    };

    // Generate HAS_ERROR_DATA const when js_proto = "Error".
    let has_error_data_const = if opts.js_proto.as_deref() == Some("Error") {
        quote! {
            const HAS_ERROR_DATA: bool = true;
        }
    } else {
        quote! {}
    };

    let output = quote! {
        #[doc(hidden)]
        #[derive(::core_runtime::Traceable)]
        #input

        // Static JSClassOps for this type — unique per ClassDef.
        #[doc(hidden)]
        static #class_ops_static: ::js::class_spec::JSClassOps = ::js::class_spec::JSClassOps {
            addProperty: None,
            delProperty: None,
            enumerate: None,
            newEnumerate: None,
            resolve: None,
            mayResolve: None,
            finalize: Some(::core_runtime::class::generic_class_finalize::<#inner_name>),
            call: None,
            construct: None,
            trace: Some(::core_runtime::class::generic_class_trace::<#inner_name>),
        };

        // Static JSClass for this type — its address serves as the type tag.
        #[doc(hidden)]
        static #class_static: ::js::class_spec::JSClass = {
            // Ensure at least MIN_CLASS_RESERVED_SLOTS for private data (slot 0).
            // Use a const block so the max() is evaluated at compile time.
            const SLOTS: u32 = if <#inner_name as ::core_runtime::class::ClassDef>::RESERVED_SLOTS
                > ::core_runtime::class::MIN_CLASS_RESERVED_SLOTS
            {
                <#inner_name as ::core_runtime::class::ClassDef>::RESERVED_SLOTS
            } else {
                ::core_runtime::class::MIN_CLASS_RESERVED_SLOTS
            };

            ::js::class_spec::JSClass {
                name: #js_name_cstr_literal as *const u8 as *const ::std::ffi::c_char,
                flags: ::js::class_spec::JSCLASS_FOREGROUND_FINALIZE
                    | ((SLOTS & ::js::class_spec::JSCLASS_RESERVED_SLOTS_MASK)
                        << ::js::class_spec::JSCLASS_RESERVED_SLOTS_SHIFT),
                cOps: &#class_ops_static as *const ::js::class_spec::JSClassOps,
                spec: ::std::ptr::null(),
                ext: ::std::ptr::null(),
                oOps: ::std::ptr::null(),
            }
        };

        // Generated ClassDef impl using autoref specialization.
        // The constructor and method registration delegate to companion types
        // that are populated by #[jsmethods].
        impl ::core_runtime::class::ClassDef for #inner_name {
            const NAME: &'static str = #js_name;

            fn class() -> &'static ::js::class_spec::JSClass {
                &#class_static
            }

            fn constructor(
                scope: &::js::gc::scope::Scope<'_>,
                args: &::js::native::CallArgs,
            ) -> Result<Self, ()> {
                use ::core_runtime::class::__ConstructorRegistrar;
                let reg = ::core_runtime::class::__CtorReg::<Self>::new();
                (&reg).construct(scope, args)
            }

            fn register_class_methods(
                builder: ::core_runtime::class::ClassBuilder<Self>,
            ) -> ::core_runtime::class::ClassBuilder<Self> {
                use ::core_runtime::class::__MethodRegistrar;
                let reg = ::core_runtime::class::__MethodReg::<Self>::new();
                (&reg).register(builder)
            }

            fn register_static_methods(
                builder: ::core_runtime::class::ClassBuilder<Self>,
            ) -> ::core_runtime::class::ClassBuilder<Self> {
                use ::core_runtime::class::__StaticMethodRegistrar;
                let reg = ::core_runtime::class::__StaticMethodReg::<Self>::new();
                (&reg).register(builder)
            }

            fn destructor(&mut self) {
                use ::core_runtime::class::__DestructorRegistrar;
                let reg = ::core_runtime::class::__DtorReg::<Self>::new();
                (&reg).destruct(self);
            }

            fn register_constants(
                builder: ::core_runtime::class::ClassBuilder<Self>,
            ) -> ::core_runtime::class::ClassBuilder<Self> {
                use ::core_runtime::class::__ConstantRegistrar;
                let reg = ::core_runtime::class::__ConstantReg::<Self>::new();
                (&reg).register(builder)
            }

            #parent_classdef_methods
            #to_string_tag_const
            #has_error_data_const
            #constants_on_prototype_const
        }

        // Reflexive DerivedFrom: every class derives from itself
        impl ::core_runtime::class::DerivedFrom<#inner_name> for #inner_name {}

        // ================================================================
        // Stack newtype: Foo<'s> — a transparent Handle wrapper
        // ================================================================

        /// Stack newtype wrapping a rooted JS object handle.
        ///
        /// This type follows the same pattern as `crate::js::date::Date<'s>`,
        /// `crate::js::promise::Promise<'s>`, etc. It is `Copy + Clone` and
        /// dereferences to `Object<'s>`.
        #[repr(transparent)]
        #[derive(Clone, Copy, Debug)]
        pub struct #struct_name<'s>(::js::native::GCHandle<'s, *mut ::js::native::JSObject>);

        impl<'s> #struct_name<'s> {
            /// Get the underlying `HandleObject`.
            #[inline]
            pub fn handle(&self) -> ::js::native::GCHandle<'s, *mut ::js::native::JSObject> {
                self.0
            }

            /// Get the raw `*mut JSObject` pointer.
            #[inline]
            pub fn as_raw(self) -> *mut ::js::native::JSObject {
                self.0.get()
            }

            /// Create from a rooted handle. Does not verify the object type.
            ///
            /// # Safety
            ///
            /// The handle must point to a JS object that was created via
            /// the class registration for this type.
            #[inline]
            pub unsafe fn from_handle(
                h: ::js::native::GCHandle<'s, *mut ::js::native::JSObject>,
            ) -> Self {
                #struct_name(h)
            }

            /// Root a raw pointer and wrap it, returning `None` if null or not
            /// an instance of this class.
            ///
            /// # Safety
            ///
            /// `ptr` must be either null or a valid pointer to a live JS object.
            pub unsafe fn from_raw(
                scope: &'s ::js::gc::scope::Scope<'_>,
                ptr: *mut ::js::native::JSObject,
            ) -> Option<Self> {
                let nn = ::std::ptr::NonNull::new(ptr)?;
                // Verify type tag via JSClass pointer
                let concrete_tag = ::core_runtime::class::get_class_tag(ptr);
                let target_tag = ::core_runtime::class::class_tag::<#inner_name>();
                if !::core_runtime::class::is_derived_from_type(concrete_tag, target_tag) {
                    return None;
                }
                Some(#struct_name(scope.root_object(nn)))
            }

            /// Get an immutable reference to the Rust data stored in this object.
            ///
            /// # Safety
            ///
            /// The handle must point to a live JS object with private data of
            /// the expected inner type.
            #[inline]
            pub unsafe fn data(&self) -> &#inner_name {
                ::core_runtime::class::get_private_or_ancestor::<#inner_name>(self.as_raw())
                    .expect("object does not have expected private data")
            }

            /// Get a mutable reference to the Rust data stored in this object.
            ///
            /// # Safety
            ///
            /// Same as [`data`](Self::data), plus no other references to the
            /// data may exist simultaneously.
            #[inline]
            #[allow(clippy::mut_from_ref)]
            pub unsafe fn data_mut(&self) -> &mut #inner_name {
                ::core_runtime::class::get_private_or_ancestor_mut::<#inner_name>(self.as_raw())
                    .expect("object does not have expected private data")
            }

            /// Type-checked cast from any `Object`.
            ///
            /// Returns `Some(Self)` if `obj` was created as this class (or a
            /// subclass), `None` otherwise.  This is the primary downcast
            /// mechanism for stack newtypes.
            pub fn from_object(scope: &'s ::js::gc::scope::Scope<'_>, obj: ::js::object::Object<'s>) -> Option<Self> {
                let ptr = obj.as_raw();
                let concrete_tag = unsafe { ::core_runtime::class::get_class_tag(ptr) };
                let target_tag = ::core_runtime::class::class_tag::<#inner_name>();
                if !::core_runtime::class::is_derived_from_type(concrete_tag, target_tag) {
                    return None;
                }
                // SAFETY: we verified the type tag, and the pointer came from
                // a live Object handle.
                let nn = ::std::ptr::NonNull::new(ptr).unwrap();
                Some(#struct_name(scope.root_object(nn)))
            }
        }

        impl<'s> ::core_runtime::class::StackNewtype<'s> for #struct_name<'s> {
            type Inner = #inner_name;

            unsafe fn from_handle_unchecked(
                h: ::js::native::GCHandle<'s, *mut ::js::native::JSObject>,
            ) -> Self {
                #struct_name(h)
            }

            fn js_handle(self) -> ::js::native::GCHandle<'s, *mut ::js::native::JSObject> {
                self.0
            }
        }

        impl<'s> ::std::ops::Deref for #struct_name<'s> {
            type Target = ::js::object::Object<'s>;
            fn deref(&self) -> &Self::Target {
                // SAFETY: Both Foo<'s> and Object<'s> are #[repr(transparent)]
                // wrappers over Handle<'s, *mut JSObject>.
                unsafe { ::std::mem::transmute(self) }
            }
        }

        // ================================================================
        // FooRef — heap reference newtype with get() method
        // ================================================================

        /// Heap reference to a [`#struct_name`] JS object.
        ///
        /// Use [`get`](Self::get) to root the object and obtain a stack
        /// newtype for method calls.
        #[js::must_root]
        pub struct #ref_alias(::core_runtime::class::HeapRef<#inner_name>);

        impl #ref_alias {
            /// Root the heap-stored JS object and return the stack newtype.
            ///
            /// Returns `None` if the underlying JS object is null.
            pub fn get<'s>(&self, scope: &'s ::js::gc::scope::Scope<'_>) -> Option<#struct_name<'s>> {
                let obj = self.0.get_jsobject();
                let nn = ::std::ptr::NonNull::new(obj)?;
                Some(#struct_name(scope.root_object(nn)))
            }

            /// Create from an `Object` handle.
            ///
            /// **For use by generated code only.** `FooRef` stores a `HeapRef`
            /// which must live inside a `Traceable` struct — never on the
            /// stack. Use the stack newtype (e.g. `Foo<'s>`) for locals.
            ///
            /// # Safety
            ///
            /// `obj` must be a JS object created via the class for this type.
            #[doc(hidden)]
            pub unsafe fn from_object<'s>(obj: ::js::object::Object<'s>) -> Self {
                #ref_alias(::core_runtime::class::HeapRef::from_object(obj))
            }

            /// Create from a raw JS object pointer.
            ///
            /// **For use by generated code only.** `FooRef` stores a `HeapRef`
            /// which must live inside a `Traceable` struct — never on the
            /// stack. Use the stack newtype (e.g. `Foo<'s>`) for locals.
            ///
            /// # Safety
            ///
            /// `obj` must be a valid, non-null JS object pointer with private
            /// data of the expected inner type.
            #[doc(hidden)]
            pub unsafe fn from_raw(obj: *mut ::js::native::JSObject) -> Self {
                #ref_alias(::core_runtime::class::HeapRef::from_raw(obj))
            }

            /// Get the raw JS object pointer.
            pub fn get_jsobject(&self) -> *mut ::js::native::JSObject {
                self.0.get_jsobject()
            }
        }

        impl From<::core_runtime::class::HeapRef<#inner_name>> for #ref_alias {
            #[js::allow_unrooted]
            fn from(hr: ::core_runtime::class::HeapRef<#inner_name>) -> Self {
                #ref_alias(hr)
            }
        }

        impl ::std::ops::Deref for #ref_alias {
            type Target = ::core_runtime::class::HeapRef<#inner_name>;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl ::std::ops::DerefMut for #ref_alias {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }

        unsafe impl ::js::heap::Trace for #ref_alias {
            #[inline]
            unsafe fn trace(&self, trc: *mut ::js::native::JSTracer) {
                self.0.trace(trc);
            }
        }
    };

    // If extends is specified, append inheritance impls
    let output = if let Some(ref inner_parent_name) = inner_parent {
        let parent_name = opts_extends_ident.as_ref().unwrap();
        let parent_ref = format_ident!("{}Ref", parent_name);
        quote! {
            #output

            impl ::core_runtime::class::HasParent for #inner_name {
                type Parent = #inner_parent_name;
                fn as_parent(&self) -> &#inner_parent_name { &self.parent }
                fn as_parent_mut(&mut self) -> &mut #inner_parent_name { &mut self.parent }
            }

            impl ::core_runtime::class::DerivedFrom<#inner_parent_name> for #inner_name {}

            // Upcast on the stack newtype: Child<'s> -> Parent<'s>
            impl<'s> #struct_name<'s> {
                /// Upcast to the parent class stack newtype.
                ///
                /// The returned handle wraps the same JS object — only the
                /// Rust view changes.  Always succeeds because the child
                /// is guaranteed to derive from the parent.
                #[inline]
                pub fn upcast(self) -> #parent_name<'s> {
                    // SAFETY: DerivedFrom guarantees the object IS the parent type.
                    unsafe { #parent_name::from_handle(self.handle()) }
                }
            }

            impl #ref_alias {
                /// Upcast this ref to the parent class ref type.
                #[js::allow_unrooted]
                pub fn upcast(&self) -> #parent_ref {
                    #parent_ref::from(self.0.upcast())
                }
            }
        }
    } else {
        output
    };

    output.into()
}

// ============================================================================
// #[jsmethods] attribute macro
// ============================================================================

/// Classification of a method in the impl block.
enum MethodKind {
    Constructor,
    Destructor,
    Method {
        js_name: String,
        nargs: usize,
    },
    StaticMethod {
        js_name: String,
        nargs: usize,
    },
    /// Property getter — becomes a JSPropertySpec accessor.
    Getter {
        js_name: String,
    },
    /// Property setter — becomes a JSPropertySpec accessor.
    Setter {
        js_name: String,
    },
    /// Combined property (getter + setter via a single annotation).
    Property {
        js_name: String,
    },
}

/// How the return value of a method should be handled.
enum ReturnStyle {
    /// No return value (or returns `()`)
    Void,
    /// Returns a value that implements `ToJSValConvertible`
    Value,
    /// Returns `Result<(), impl Display>` — error becomes JS exception
    ResultVoid,
    /// Returns `Result<T, impl Display>` — Ok value set as return, Err becomes exception
    ResultValue,
    /// Raw method returning `Result<(), ()>` with manual exception handling
    Raw,
    /// Returns `JSPromise` — creates a JS Promise and spawns the async future
    Promise,
    /// Returns `Self` (or the class type) from a method/static_method —
    /// the result is wrapped into a new JS object via `create_instance`.
    InstanceValue,
}

/// Info about a parsed method.
struct MethodInfo {
    kind: MethodKind,
    fn_item: ImplItemFn,
    /// Parameter names and types (excluding self/cx/args)
    params: Vec<(Ident, Type)>,
    /// How the return value should be handled
    return_style: ReturnStyle,
    /// Whether the method takes &self
    has_self: bool,
    /// Whether the method takes &mut self
    has_mut_self: bool,
    /// Whether the method takes cx: &mut JSContext
    has_cx: bool,
    /// Whether the method has raw cx/args params for low-level access
    is_raw: bool,
    /// Whether the method has a variadic rest parameter (last param)
    has_rest_args: bool,
    /// Name of the rest parameter, if any
    rest_arg_name: Option<Ident>,
    /// Inner type of RestArgs<T>, or None for bare RestArgs (defaults to Value)
    rest_inner_type: Option<Type>,
}

/// Attribute macro for an `impl` block that generates JSNative wrappers.
///
/// The impl block is written on the user-visible type name (e.g. `impl Foo`),
/// but is rewritten to target the inner data struct (`impl __FooInner`).
/// Forwarding methods and constructors are generated on the stack newtype
/// `Foo<'s>`.
///
/// # Usage
///
/// ```rust,ignore
/// #[jsmethods]
/// impl MyClass {
///     #[constructor]
///     fn new(data: String) -> Self {
///         Self { data }
///     }
///
///     #[method(name = "toString")]
///     fn to_string(&self) -> String {
///         format!("MyClass({})", self.data)
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn jsmethods(attr: TokenStream, item: TokenStream) -> TokenStream {
    let _opts = parse_macro_input!(attr as AttrOpts);
    let mut input = parse_macro_input!(item as ItemImpl);

    let self_ty = &input.self_ty;

    // Extract the type name for generating function names
    let type_name = match self_ty.as_ref() {
        Type::Path(tp) => tp
            .path
            .segments
            .last()
            .map(|s| s.ident.clone())
            .expect("Expected a named type"),
        _ => panic!("#[jsmethods] requires a named type"),
    };

    // Compute the inner data struct name
    let inner_name = format_ident!("__{}Inner", type_name);

    let mut methods: Vec<MethodInfo> = Vec::new();
    let mut ctor_original_name: Option<Ident> = None;
    let mut constant_builder_calls: Vec<proc_macro2::TokenStream> = Vec::new();

    // Parse each item and classify it
    for item in &mut input.items {
        // Handle `pub const NAME: Type = value;` items — generate constant builder calls.
        if let ImplItem::Const(const_item) = item {
            if matches!(const_item.vis, Visibility::Public(_)) {
                let const_name = const_item.ident.to_string();
                let const_name_bytes = format!("{const_name}\0");
                let const_name_cstr =
                    proc_macro2::Literal::byte_string(const_name_bytes.as_bytes());
                let const_ident = &const_item.ident;

                constant_builder_calls.push(quote! {
                    .constant(
                        unsafe { ::std::ffi::CStr::from_bytes_with_nul_unchecked(#const_name_cstr) },
                        #inner_name::#const_ident as i32,
                    )
                });
            }
            continue;
        }

        if let ImplItem::Fn(fn_item) = item {
            let mut kind = None;
            let mut custom_rename = None;

            // Check for our attributes
            fn_item.attrs.retain(|attr| {
                if attr.path().is_ident("constructor") {
                    kind = Some(MethodKind::Constructor);
                    false
                } else if attr.path().is_ident("method") {
                    // Parse optional (name = "...")
                    if let Ok(opts) = attr.parse_args::<AttrOpts>() {
                        custom_rename = opts.name;
                    }
                    kind = Some(MethodKind::Method {
                        js_name: String::new(), // filled below
                        nargs: 0,
                    });
                    false
                } else if attr.path().is_ident("static_method") {
                    // Parse optional (name = "...")
                    if let Ok(opts) = attr.parse_args::<AttrOpts>() {
                        custom_rename = opts.name;
                    }
                    kind = Some(MethodKind::StaticMethod {
                        js_name: String::new(), // filled below
                        nargs: 0,
                    });
                    false
                } else if attr.path().is_ident("getter") {
                    // Parse optional (name = "...")
                    if let Ok(opts) = attr.parse_args::<AttrOpts>() {
                        custom_rename = opts.name;
                    }
                    kind = Some(MethodKind::Getter {
                        js_name: String::new(), // filled below
                    });
                    false
                } else if attr.path().is_ident("setter") {
                    // Parse optional (name = "...")
                    if let Ok(opts) = attr.parse_args::<AttrOpts>() {
                        custom_rename = opts.name;
                    }
                    kind = Some(MethodKind::Setter {
                        js_name: String::new(), // filled below
                    });
                    false
                } else if attr.path().is_ident("property") {
                    // Parse optional (name = "...")
                    if let Ok(opts) = attr.parse_args::<AttrOpts>() {
                        custom_rename = opts.name;
                    }
                    kind = Some(MethodKind::Property {
                        js_name: String::new(), // filled below
                    });
                    false
                } else if attr.path().is_ident("destructor") {
                    kind = Some(MethodKind::Destructor);
                    false
                } else {
                    true // keep other attrs
                }
            });

            let kind = match kind {
                Some(k) => k,
                None => continue, // Skip methods without our attrs
            };

            let info = parse_method_info(fn_item.clone(), kind, custom_rename, &type_name);

            if matches!(info.kind, MethodKind::Constructor) {
                ctor_original_name = Some(fn_item.sig.ident.clone());
            }

            // Rewrite RestArgs<T> in the function signature to use the
            // fully-qualified type path so the impl block compiles.
            if info.has_rest_args {
                let inner_ty = info
                    .rest_inner_type
                    .clone()
                    .unwrap_or_else(|| syn::parse_quote!(::js::native::Value));
                for arg in fn_item.sig.inputs.iter_mut() {
                    if let FnArg::Typed(pat_ty) = arg {
                        if is_rest_args_type(&pat_ty.ty) {
                            *pat_ty.ty = syn::parse_quote! {
                                ::core_runtime::class::RestArgs<#inner_ty>
                            };
                        }
                    }
                }
            }

            methods.push(info);
        }
    }

    // Rewrite the impl block's self type to __FooInner
    *input.self_ty = syn::parse_quote! { #inner_name };

    // Suppress clippy warnings for generated impl (e.g. inherent to_string methods)
    input
        .attrs
        .push(syn::parse_quote! { #[allow(clippy::inherent_to_string)] });

    // Generate JSNative wrappers for non-constructor methods
    let mut native_fns = Vec::new();
    let mut builder_calls = Vec::new();
    let mut static_builder_calls = Vec::new();
    let mut constructor_body = None;
    let mut destructor_fn_name = None;

    // Collect property accessors indexed by JS name for pairing
    struct PropertyEntry {
        js_name: String,
        getter_native: Option<Ident>,
        setter_native: Option<Ident>,
    }
    let mut property_map: Vec<PropertyEntry> = Vec::new();

    fn find_or_create_property<'a>(
        map: &'a mut Vec<PropertyEntry>,
        js_name: &str,
    ) -> &'a mut PropertyEntry {
        if let Some(pos) = map.iter().position(|e| e.js_name == js_name) {
            &mut map[pos]
        } else {
            map.push(PropertyEntry {
                js_name: js_name.to_string(),
                getter_native: None,
                setter_native: None,
            });
            map.last_mut().unwrap()
        }
    }

    for method in &methods {
        match &method.kind {
            MethodKind::Constructor => {
                constructor_body = Some(gen_constructor_body(method, &inner_name));
            }
            MethodKind::Destructor => {
                destructor_fn_name = Some(method.fn_item.sig.ident.clone());
            }
            MethodKind::Method { js_name, nargs } => {
                let (native_fn, builder_call) =
                    gen_method_native(method, &inner_name, js_name, *nargs);
                native_fns.push(native_fn);
                builder_calls.push(builder_call);
            }
            MethodKind::StaticMethod { js_name, nargs } => {
                let (native_fn, builder_call) =
                    gen_method_native(method, &inner_name, js_name, *nargs);
                native_fns.push(native_fn);
                static_builder_calls.push(builder_call);
            }
            MethodKind::Getter { js_name } => {
                let native_fn = gen_accessor_native(method, &inner_name, js_name, true);
                let native_name =
                    format_ident!("__getter_{inner_name}_{}", method.fn_item.sig.ident);
                native_fns.push(native_fn);
                let entry = find_or_create_property(&mut property_map, js_name);
                entry.getter_native = Some(native_name);
            }
            MethodKind::Setter { js_name } => {
                let native_fn = gen_accessor_native(method, &inner_name, js_name, false);
                let native_name =
                    format_ident!("__setter_{inner_name}_{}", method.fn_item.sig.ident);
                native_fns.push(native_fn);
                let entry = find_or_create_property(&mut property_map, js_name);
                entry.setter_native = Some(native_name);
            }
            MethodKind::Property { js_name } => {
                // A #[property] annotation means this is a getter; look for a
                // matching setter (`set_<name>`) method in the impl block.
                let native_fn = gen_accessor_native(method, &inner_name, js_name, true);
                let native_name =
                    format_ident!("__getter_{inner_name}_{}", method.fn_item.sig.ident);
                native_fns.push(native_fn);
                let entry = find_or_create_property(&mut property_map, js_name);
                entry.getter_native = Some(native_name);
            }
        }
    }

    // Generate .property() builder calls for all accessor entries
    for entry in &property_map {
        let js_name = &entry.js_name;
        let js_name_bytes = format!("{js_name}\0");
        let js_name_cstr = proc_macro2::Literal::byte_string(js_name_bytes.as_bytes());

        let getter = match &entry.getter_native {
            Some(name) => quote! { Some(#name) },
            None => quote! { None },
        };
        let setter = match &entry.setter_native {
            Some(name) => quote! { Some(#name) },
            None => quote! { None },
        };

        builder_calls.push(quote! {
            .property(
                unsafe { ::std::ffi::CStr::from_bytes_with_nul_unchecked(#js_name_cstr) },
                #getter,
                #setter,
            )
        });
    }

    // Generate the ConstructorRegistrar impl (on __FooInner)
    let ctor_impl = if let Some(body) = constructor_body {
        quote! {
            impl ::core_runtime::class::__ConstructorRegistrar<#inner_name> for ::core_runtime::class::__CtorReg<#inner_name> {
                fn construct(
                    &self,
                    scope: &::js::gc::scope::Scope<'_>,
                    args: &::js::native::CallArgs,
                ) -> Result<#inner_name, ()> {
                    unsafe { #body }
                }
            }
        }
    } else {
        quote! {
            impl ::core_runtime::class::__ConstructorRegistrar<#inner_name> for ::core_runtime::class::__CtorReg<#inner_name> {
                fn construct(
                    &self,
                    _scope: &::js::gc::scope::Scope<'_>,
                    _args: &::js::native::CallArgs,
                ) -> Result<#inner_name, ()> {
                    panic!("{} builtin can't be instantiated directly", stringify!(#type_name));
                }
            }
        }
    };

    // Generate the MethodRegistrar impl (on __FooInner)
    let method_impl = quote! {
        impl ::core_runtime::class::__MethodRegistrar<#inner_name> for ::core_runtime::class::__MethodReg<#inner_name> {
            fn register(
                &self,
                builder: ::core_runtime::class::ClassBuilder<#inner_name>,
            ) -> ::core_runtime::class::ClassBuilder<#inner_name> {
                builder #(#builder_calls)*
            }
        }
    };

    // Generate the StaticMethodRegistrar impl (only if static methods exist)
    let static_method_impl = if !static_builder_calls.is_empty() {
        quote! {
            impl ::core_runtime::class::__StaticMethodRegistrar<#inner_name> for ::core_runtime::class::__StaticMethodReg<#inner_name> {
                fn register(
                    &self,
                    builder: ::core_runtime::class::ClassBuilder<#inner_name>,
                ) -> ::core_runtime::class::ClassBuilder<#inner_name> {
                    builder #(#static_builder_calls)*
                }
            }
        }
    } else {
        quote! {}
    };

    // Generate the ConstantRegistrar impl (only if constants exist)
    let constant_impl = if !constant_builder_calls.is_empty() {
        quote! {
            impl ::core_runtime::class::__ConstantRegistrar<#inner_name> for ::core_runtime::class::__ConstantReg<#inner_name> {
                fn register(
                    &self,
                    builder: ::core_runtime::class::ClassBuilder<#inner_name>,
                ) -> ::core_runtime::class::ClassBuilder<#inner_name> {
                    builder #(#constant_builder_calls)*
                }
            }
        }
    } else {
        quote! {}
    };

    // Generate the DestructorRegistrar impl
    let dtor_impl = if let Some(fn_name) = destructor_fn_name {
        quote! {
            impl ::core_runtime::class::__DestructorRegistrar<#inner_name> for ::core_runtime::class::__DtorReg<#inner_name> {
                fn destruct(&self, this: &mut #inner_name) {
                    #inner_name::#fn_name(this);
                }
            }
        }
    } else {
        quote! {}
    };

    // Generate `impl<'s> Foo<'s>` containing new() and add_to_global()
    let ctor_new_impl = if let Some(ref ctor_fn_name) = ctor_original_name {
        let ctor_method = methods
            .iter()
            .find(|m| matches!(m.kind, MethodKind::Constructor));
        if let Some(method) = ctor_method {
            // Skip generating the stack newtype `new()` when the constructor
            // uses the raw `&CallArgs` pattern (only available inside JSNative
            // wrappers). Such constructors are only callable from JS via `new`.
            if method.is_raw {
                quote! {
                    impl<'s> #type_name<'s> {
                        /// Register this class on a global object, making it available
                        /// as a constructor in JavaScript.
                        pub fn add_to_global(scope: &'s ::js::gc::scope::Scope<'_>, global: ::js::object::Object<'s>) {
                            unsafe { ::core_runtime::class::register_class::<#inner_name>(scope, global); }
                        }
                    }
                }
            } else {
                let param_decls: Vec<_> = method
                    .params
                    .iter()
                    .map(|(name, ty)| quote! { #name: #ty })
                    .collect();
                let param_names: Vec<_> = method
                    .params
                    .iter()
                    .map(|(name, _)| quote! { #name })
                    .collect();

                let call = if method.has_cx {
                    quote! { #inner_name::#ctor_fn_name(scope, #(#param_names),*) }
                } else {
                    quote! { #inner_name::#ctor_fn_name(#(#param_names),*) }
                };

                quote! {
                    impl<'s> #type_name<'s> {
                        /// Construct a new instance and return the stack newtype.
                        pub fn new(scope: &'s ::js::gc::scope::Scope<'_>, #(#param_decls),*)
                            -> #type_name<'s>
                        {
                            unsafe {
                                let instance = #call;
                                let obj = ::core_runtime::class::create_instance::<#inner_name>(scope, instance)
                                    .expect(concat!("Class ", stringify!(#type_name), " not registered"));
                                // Re-root through scope to get Handle<'s, _> with the scope lifetime,
                                // since Object::handle() narrows the lifetime to the borrow.
                                let nn = ::std::ptr::NonNull::new(obj.as_raw()).unwrap();
                                #type_name(scope.root_object(nn))
                            }
                        }

                        /// Register this class on a global object, making it available
                        /// as a constructor in JavaScript.
                        pub fn add_to_global(scope: &'s ::js::gc::scope::Scope<'_>, global: ::js::object::Object<'s>) {
                            unsafe { ::core_runtime::class::register_class::<#inner_name>(scope, global); }
                        }
                    }
                }
            }
        } else {
            quote! {}
        }
    } else {
        quote! {}
    };

    // Generate forwarding methods on Foo<'s> for pub instance methods
    let mut newtype_methods: Vec<proc_macro2::TokenStream> = Vec::new();

    for method in &methods {
        match &method.kind {
            MethodKind::Constructor | MethodKind::Destructor | MethodKind::StaticMethod { .. } => {
                continue;
            }
            MethodKind::Getter { .. } | MethodKind::Property { .. } => {
                // Getters are forwarded as simple methods. They always have &self.
                let fn_name = &method.fn_item.sig.ident;
                let name_str = fn_name.to_string();
                if matches!(
                    name_str.as_str(),
                    "data"
                        | "data_mut"
                        | "handle"
                        | "as_raw"
                        | "from_handle"
                        | "from_raw"
                        | "from_object"
                ) {
                    continue;
                }
                let ret_ty = &method.fn_item.sig.output;
                let get_inner = quote! { let inner = unsafe { self.data() }; };
                let cx_param = if method.has_cx {
                    quote! { scope: &::js::gc::scope::Scope<'_>, }
                } else {
                    quote! {}
                };
                let cx_arg = if method.has_cx {
                    quote! { scope, }
                } else {
                    quote! {}
                };
                newtype_methods.push(quote! {
                    pub fn #fn_name(&self, #cx_param) #ret_ty {
                        #get_inner
                        #inner_name::#fn_name(inner, #cx_arg)
                    }
                });
                continue;
            }
            MethodKind::Setter { .. } => {
                // Setters are forwarded with &mut self + the value parameter.
                let fn_name = &method.fn_item.sig.ident;
                let name_str = fn_name.to_string();
                if matches!(
                    name_str.as_str(),
                    "data"
                        | "data_mut"
                        | "handle"
                        | "as_raw"
                        | "from_handle"
                        | "from_raw"
                        | "from_object"
                ) {
                    continue;
                }
                let ret_ty = &method.fn_item.sig.output;
                let param_decls: Vec<_> = method
                    .params
                    .iter()
                    .map(|(name, ty)| quote! { #name: #ty })
                    .collect();
                let param_names: Vec<_> = method
                    .params
                    .iter()
                    .map(|(name, _)| quote! { #name })
                    .collect();
                let get_inner = quote! { let inner = unsafe { self.data_mut() }; };
                let cx_param = if method.has_cx {
                    quote! { scope: &::js::gc::scope::Scope<'_>, }
                } else {
                    quote! {}
                };
                let cx_arg = if method.has_cx {
                    quote! { scope, }
                } else {
                    quote! {}
                };
                newtype_methods.push(quote! {
                    pub fn #fn_name(&self, #cx_param #(#param_decls),*) #ret_ty {
                        #get_inner
                        #inner_name::#fn_name(inner, #cx_arg #(#param_names),*)
                    }
                });
                continue;
            }
            MethodKind::Method { .. } => {
                // Skip raw, rest, and promise methods — they can't be forwarded simply
                if method.is_raw
                    || method.has_rest_args
                    || matches!(method.return_style, ReturnStyle::Promise)
                {
                    continue;
                }
                if !method.has_self && !method.has_mut_self {
                    continue;
                }

                let fn_name = &method.fn_item.sig.ident;

                // Skip methods that conflict with built-in stack newtype methods
                let name_str = fn_name.to_string();
                if matches!(
                    name_str.as_str(),
                    "data"
                        | "data_mut"
                        | "handle"
                        | "as_raw"
                        | "from_handle"
                        | "from_raw"
                        | "from_object"
                ) {
                    continue;
                }
                let ret_ty = &method.fn_item.sig.output;
                let param_decls: Vec<_> = method
                    .params
                    .iter()
                    .map(|(name, ty)| quote! { #name: #ty })
                    .collect();
                let param_names: Vec<_> = method
                    .params
                    .iter()
                    .map(|(name, _)| quote! { #name })
                    .collect();

                let get_inner = if method.has_mut_self {
                    quote! { let inner = unsafe { self.data_mut() }; }
                } else {
                    quote! { let inner = unsafe { self.data() }; }
                };

                // InstanceValue methods return Self wrapped in a new JS object —
                // they need a scope parameter and custom return handling.
                if matches!(method.return_style, ReturnStyle::InstanceValue) {
                    // Always needs a scope to create the JS object
                    let cx_param = quote! { scope: &'s ::js::gc::scope::Scope<'_>, };
                    let cx_arg = if method.has_cx {
                        quote! { scope, }
                    } else {
                        quote! {}
                    };

                    newtype_methods.push(quote! {
                        pub fn #fn_name(&self, #cx_param #(#param_decls),*) -> #type_name<'s> {
                            #get_inner
                            let __data = #inner_name::#fn_name(inner, #cx_arg #(#param_names),*);
                            unsafe {
                                let __obj = ::core_runtime::class::create_instance::<#inner_name>(scope, __data)
                                    .expect(concat!("Class ", stringify!(#type_name), " not registered"));
                                let __nn = ::std::ptr::NonNull::new(__obj.as_raw()).unwrap();
                                #type_name(scope.root_object(__nn))
                            }
                        }
                    });
                    continue;
                }

                // If the method takes a Scope parameter, forward it
                let cx_param = if method.has_cx {
                    quote! { scope: &::js::gc::scope::Scope<'_>, }
                } else {
                    quote! {}
                };
                let cx_arg = if method.has_cx {
                    quote! { scope, }
                } else {
                    quote! {}
                };

                newtype_methods.push(quote! {
                    pub fn #fn_name(&self, #cx_param #(#param_decls),*) #ret_ty {
                        #get_inner
                        #inner_name::#fn_name(inner, #cx_arg #(#param_names),*)
                    }
                });
            }
        }
    }

    let newtype_impl = if !newtype_methods.is_empty() {
        quote! {
            impl<'s> #type_name<'s> {
                #(#newtype_methods)*
            }
        }
    } else {
        quote! {}
    };

    let output = quote! {
        #input

        // Generated JSNative wrapper functions
        #(#native_fns)*

        // Generated constructor registrar
        #ctor_impl

        // Generated method registrar
        #method_impl

        // Generated static method registrar
        #static_method_impl

        // Generated constant registrar
        #constant_impl

        // Generated destructor registrar
        #dtor_impl

        // Generated inherent new() constructor + add_to_global on stack newtype
        #ctor_new_impl

        // Generated forwarding methods on stack newtype
        #newtype_impl
    };

    output.into()
}

// ============================================================================
// Method analysis
// ============================================================================

fn parse_method_info(
    fn_item: ImplItemFn,
    mut kind: MethodKind,
    custom_rename: Option<String>,
    type_name: &Ident,
) -> MethodInfo {
    let method_name = fn_item.sig.ident.to_string();

    // Determine self receiver
    let has_self = fn_item
        .sig
        .inputs
        .first()
        .map(|a| matches!(a, FnArg::Receiver(r) if r.mutability.is_none()))
        .unwrap_or(false);
    let has_mut_self = fn_item
        .sig
        .inputs
        .first()
        .map(|a| matches!(a, FnArg::Receiver(r) if r.mutability.is_some()))
        .unwrap_or(false);

    // Collect non-self parameters, detecting cx and raw params
    let mut params = Vec::new();
    let mut has_cx = false;
    let mut is_raw = false;
    let mut has_rest_args = false;
    let mut rest_arg_name = None;
    let mut rest_inner_type = None;
    let skip_first = if has_self || has_mut_self { 1 } else { 0 };

    for arg in fn_item.sig.inputs.iter().skip(skip_first) {
        if let FnArg::Typed(pat_ty) = arg {
            if is_cx_param_type(&pat_ty.ty) {
                has_cx = true;
                continue;
            }
            if is_callargs_param_type(&pat_ty.ty) {
                is_raw = true;
                continue;
            }
            // Check for RestArgs marker type
            if is_rest_args_type(&pat_ty.ty) {
                if let Pat::Ident(pat_ident) = pat_ty.pat.as_ref() {
                    has_rest_args = true;
                    rest_arg_name = Some(pat_ident.ident.clone());
                    rest_inner_type = extract_rest_args_inner_type(&pat_ty.ty);
                }
                continue;
            }
            if let Pat::Ident(pat_ident) = pat_ty.pat.as_ref() {
                params.push((pat_ident.ident.clone(), (*pat_ty.ty).clone()));
            }
        }
    }

    // Determine return style
    let is_constructor = matches!(kind, MethodKind::Constructor);
    let return_style = classify_return_style(&fn_item.sig.output, Some(type_name), is_constructor);

    // Compute JS name: custom name overrides, otherwise default to camelCase.
    // For setters, derive the property name by stripping "set_" prefix.
    let js_name = custom_rename.unwrap_or_else(|| {
        if matches!(kind, MethodKind::Setter { .. }) {
            let stripped = method_name.strip_prefix("set_").unwrap_or(&method_name);
            stripped.to_lower_camel_case()
        } else {
            method_name.to_lower_camel_case()
        }
    });

    let nargs = params.len();

    match &mut kind {
        MethodKind::Method {
            js_name: n,
            nargs: na,
        } => {
            *n = js_name;
            *na = nargs;
        }
        MethodKind::StaticMethod {
            js_name: n,
            nargs: na,
        } => {
            *n = js_name;
            *na = nargs;
        }
        MethodKind::Getter { js_name: n } => {
            *n = js_name;
        }
        MethodKind::Setter { js_name: n } => {
            *n = js_name;
        }
        MethodKind::Property { js_name: n } => {
            *n = js_name;
        }
        _ => {}
    }

    MethodInfo {
        kind,
        fn_item,
        params,
        return_style,
        has_self,
        has_mut_self,
        has_cx,
        is_raw,
        has_rest_args,
        rest_arg_name,
        rest_inner_type,
    }
}

fn is_cx_param_type(ty: &Type) -> bool {
    let s = quote!(#ty).to_string();
    s.contains("JSContext") || s.contains("Scope")
}

fn is_callargs_param_type(ty: &Type) -> bool {
    let s = quote!(#ty).to_string();
    s.contains("CallArgs")
}

fn is_rest_args_type(ty: &Type) -> bool {
    let s = quote!(#ty).to_string();
    s == "RestArgs"
        || s.starts_with("RestArgs <")
        || s.starts_with("RestArgs<")
        || s.ends_with(":: RestArgs")
        || s.ends_with("::RestArgs")
        || s.contains(":: RestArgs <")
        || s.contains("::RestArgs<")
}

/// Extract the inner type from `RestArgs<T>`. Returns `None` for bare `RestArgs`
/// (which defaults to `Value`).
fn extract_rest_args_inner_type(ty: &Type) -> Option<Type> {
    if let Type::Path(type_path) = ty {
        let last_seg = type_path.path.segments.last()?;
        let ident = last_seg.ident.to_string();
        if ident == "RestArgs" {
            if let syn::PathArguments::AngleBracketed(args) = &last_seg.arguments {
                if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                    return Some(inner.clone());
                }
            }
        }
    }
    None
}

/// Walk a `UseTree` to find the leaf `Ident` (e.g., `super::Vec2` → `Vec2`).
fn extract_use_leaf_ident(tree: &syn::UseTree) -> Option<&Ident> {
    match tree {
        syn::UseTree::Name(name) => Some(&name.ident),
        syn::UseTree::Path(path) => extract_use_leaf_ident(&path.tree),
        _ => None,
    }
}

fn is_promise_type(ty: &Type) -> bool {
    let s = quote!(#ty).to_string();
    s == "JSPromise" || s.ends_with(":: JSPromise") || s.ends_with("::JSPromise")
}

fn is_integer_type(ty: &Type) -> bool {
    let s = quote!(#ty).to_string();
    matches!(
        s.as_str(),
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "isize" | "usize"
    )
}

/// Check if a type string is exactly `Result < () , () >`
fn is_result_unit_unit(ty_str: &str) -> bool {
    let normalized: String = ty_str.chars().filter(|c| !c.is_whitespace()).collect();
    normalized == "Result<(),()>"
}

/// Check if a type string is a `Result<T, E>` type.
/// Returns `Some(true)` if Result has a non-() Ok type, `Some(false)` if Ok is ().
/// Returns `None` if not a Result type.
fn is_result_type(ty_str: &str) -> Option<bool> {
    let normalized: String = ty_str.chars().filter(|c| !c.is_whitespace()).collect();
    if !normalized.starts_with("Result<") {
        return None;
    }
    // Extract the inner part between Result< and >
    let inner = &normalized["Result<".len()..normalized.len() - 1];
    // Find the Ok type (before the first comma at depth 0)
    let mut depth = 0;
    for (i, c) in inner.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                let ok_type = &inner[..i];
                return Some(ok_type != "()");
            }
            _ => {}
        }
    }
    None
}

/// Classify the return type of a function into a `ReturnStyle`.
/// If `type_name` is provided and `is_constructor` is true, `Self` returns become `Void`
/// (constructors handle object creation separately). For non-constructor methods,
/// `Self` returns become `InstanceValue` so the macro auto-wraps them.
fn classify_return_style(
    output: &ReturnType,
    type_name: Option<&Ident>,
    is_constructor: bool,
) -> ReturnStyle {
    match output {
        ReturnType::Default => ReturnStyle::Void,
        ReturnType::Type(_, ty) => {
            let ty_str = quote!(#ty).to_string();
            if let Some(tn) = type_name {
                if ty_str == "Self" || ty_str.contains(&tn.to_string()) {
                    if is_constructor {
                        return ReturnStyle::Void;
                    }
                    return ReturnStyle::InstanceValue;
                }
            }
            if is_promise_type(ty) {
                ReturnStyle::Promise
            } else if is_result_unit_unit(&ty_str) {
                ReturnStyle::Raw
            } else if let Some(has_inner_value) = is_result_type(&ty_str) {
                if has_inner_value {
                    ReturnStyle::ResultValue
                } else {
                    ReturnStyle::ResultVoid
                }
            } else {
                ReturnStyle::Value
            }
        }
    }
}

// ============================================================================
// Code generation
// ============================================================================

/// Generate argument extraction code for a list of typed parameters.
/// When `use_question_mark` is true, extraction errors propagate via `?`;
/// otherwise they `return false` (for use inside JSNative wrappers).
fn gen_arg_extractions(
    params: &[(Ident, Type)],
    args_expr: proc_macro2::TokenStream,
    use_question_mark: bool,
    scope_expr: proc_macro2::TokenStream,
) -> Vec<proc_macro2::TokenStream> {
    params
        .iter()
        .enumerate()
        .map(|(i, (name, ty))| {
            let idx = i as u32;
            let extract = if is_integer_type(ty) {
                quote! {
                    ::core_runtime::class::get_int_arg(#scope_expr, #args_expr, #idx,
                        ::js::conversions::ConversionBehavior::Default)
                }
            } else {
                quote! { ::core_runtime::class::get_arg(#scope_expr, #args_expr, #idx) }
            };
            if use_question_mark {
                quote! { let #name = #extract?; }
            } else {
                quote! {
                    let #name = match #extract {
                        Ok(v) => v,
                        Err(()) => return false,
                    };
                }
            }
        })
        .collect()
}

/// Generate the constructor body that extracts args and calls the Rust constructor fn.
fn gen_constructor_body(info: &MethodInfo, type_name: &Ident) -> proc_macro2::TokenStream {
    let ctor_fn = &info.fn_item.sig.ident;
    let arg_extractions = gen_arg_extractions(&info.params, quote!(args), true, quote!(scope));
    let arg_names: Vec<_> = info.params.iter().map(|(name, _)| quote!(#name)).collect();

    // Build the constructor call, passing scope and/or args if the Rust
    // constructor requested them via `scope: &Scope<'_>` or `args: &CallArgs`.
    let call = if info.is_raw {
        quote! { #type_name::#ctor_fn(scope, args) }
    } else if info.has_cx {
        quote! { #type_name::#ctor_fn(scope, #(#arg_names),*) }
    } else {
        quote! { #type_name::#ctor_fn(#(#arg_names),*) }
    };

    quote! {
        #(#arg_extractions)*
        Ok(#call)
    }
}

/// Generate a JSNative wrapper function and the corresponding ClassBuilder call.
fn gen_method_native(
    info: &MethodInfo,
    type_name: &Ident,
    js_name: &str,
    nargs: usize,
) -> (proc_macro2::TokenStream, proc_macro2::TokenStream) {
    let fn_name = &info.fn_item.sig.ident;
    let native_name = format_ident!("__native_{type_name}_{fn_name}");
    let nargs_u32 = nargs as u32;

    // Create the C string literal for the JS name
    let js_name_bytes = format!("{js_name}\0");
    let js_name_cstr = proc_macro2::Literal::byte_string(js_name_bytes.as_bytes());

    // Use __args internally to avoid shadowing user's rest param names
    let this_extraction = if info.has_self {
        quote! {
            let __self = match ::core_runtime::class::get_this::<#type_name>(&scope, &__args) {
                Ok(v) => v,
                Err(()) => return false,
            };
        }
    } else if info.has_mut_self {
        quote! {
            let __self = match ::core_runtime::class::get_this_mut::<#type_name>(&scope, &__args) {
                Ok(v) => v,
                Err(()) => return false,
            };
        }
    } else {
        quote! {}
    };

    let arg_extractions = gen_arg_extractions(&info.params, quote!(&__args), false, quote!(&scope));
    let call_args: Vec<_> = info.params.iter().map(|(name, _)| quote!(#name)).collect();

    // Generate rest args collection using FromJSValue conversion
    let rest_setup = if info.has_rest_args {
        let rest_name = info.rest_arg_name.as_ref().unwrap();
        let start_idx = info.params.len() as u32;
        let inner_ty = info
            .rest_inner_type
            .clone()
            .unwrap_or_else(|| syn::parse_quote!(::js::native::Value));
        quote! {
            let #rest_name = {
                let mut __rest_vec = ::std::vec::Vec::with_capacity(
                    (argc.saturating_sub(#start_idx)) as usize,
                );
                for __i in #start_idx..argc {
                    let __handle = unsafe {
                        ::js::native::Handle::from_raw(__args.get(__i))
                    };
                    match <#inner_ty as ::core_runtime::class::FromJSValue>::from_js_value(
                        &scope,
                        __handle.get(),
                    ) {
                        Ok(__v) => __rest_vec.push(__v),
                        Err(()) => return false,
                    }
                }
                ::core_runtime::class::RestArgs::new(__rest_vec)
            };
        }
    } else {
        quote! {}
    };

    // Build rest arg value for method call
    let rest_arg_expr: Vec<proc_macro2::TokenStream> = if info.has_rest_args {
        let rest_name = info.rest_arg_name.as_ref().unwrap();
        vec![quote! { #rest_name }]
    } else {
        vec![]
    };

    let call = if info.has_self || info.has_mut_self {
        if info.is_raw {
            quote! { #type_name::#fn_name(__self, &scope, &__args) }
        } else if info.has_cx {
            let all_args: Vec<_> = call_args.iter().chain(rest_arg_expr.iter()).collect();
            quote! { #type_name::#fn_name(__self, &scope, #(#all_args),*) }
        } else {
            let all_args: Vec<_> = call_args.iter().chain(rest_arg_expr.iter()).collect();
            quote! { #type_name::#fn_name(__self, #(#all_args),*) }
        }
    } else if info.is_raw {
        quote! { #type_name::#fn_name(&scope, &__args) }
    } else if info.has_cx {
        let all_args: Vec<_> = call_args.iter().chain(rest_arg_expr.iter()).collect();
        quote! { #type_name::#fn_name(&scope, #(#all_args),*) }
    } else {
        let all_args: Vec<_> = call_args.iter().chain(rest_arg_expr.iter()).collect();
        quote! { #type_name::#fn_name(#(#all_args),*) }
    };

    let body = match &info.return_style {
        ReturnStyle::Raw => quote! {
            match #call {
                Ok(()) => true,
                Err(()) => false,
            }
        },
        ReturnStyle::Value => quote! {
            let __result = #call;
            ::core_runtime::class::set_return(&scope, &__args, &__result);
            true
        },
        ReturnStyle::Void => quote! {
            #call;
            ::core_runtime::class::set_return(&scope, &__args, &::js::value::undefined());
            true
        },
        ReturnStyle::ResultVoid => quote! {
            match #call {
                Ok(()) => {
                    ::core_runtime::class::set_return(&scope, &__args, &::js::value::undefined());
                    true
                }
                Err(__e) => {
                    ::core_runtime::class::ThrowException::throw(__e, &scope);
                    false
                }
            }
        },
        ReturnStyle::ResultValue => quote! {
            match #call {
                Ok(__v) => {
                    ::core_runtime::class::set_return(&scope, &__args, &__v);
                    true
                }
                Err(__e) => {
                    ::core_runtime::class::ThrowException::throw(__e, &scope);
                    false
                }
            }
        },
        ReturnStyle::Promise => quote! {
            // Create a bare JS Promise (no executor)
            let __promise = match ::js::promise::Promise::new_pending(&scope) {
                Ok(p) => p,
                Err(_) => return false,
            };
            // Return the promise object to JS immediately
            __args.rval().set(unsafe { ::js::value::from_object(__promise.as_raw()) });
            // Call the user's method to get the JSPromise (containing the future)
            let __js_promise = #call;
            // Spawn the future, which will resolve/reject the promise later
            ::core_runtime::class::__spawn_promise(__promise.as_raw(), __js_promise);
            true
        },
        ReturnStyle::InstanceValue => quote! {
            let __instance = #call;
            let __obj = match ::core_runtime::class::create_instance::<#type_name>(&scope, __instance) {
                Ok(o) => o,
                Err(_) => return false,
            };
            __args.rval().set(unsafe { ::js::value::from_object(__obj.as_raw()) });
            true
        },
    };

    let native_fn = quote! {
        #[allow(non_snake_case)]
        unsafe extern "C" fn #native_name(
            raw_cx: *mut ::js::native::RawJSContext,
            argc: u32,
            vp: *mut ::js::native::Value,
        ) -> bool {
            let mut __cx = unsafe { ::js::native::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(raw_cx)) };
            let scope = unsafe { ::js::gc::scope::RootScope::from_current_realm(&mut __cx) };
            let __args = ::js::native::CallArgs::from_vp(vp, argc);
            #this_extraction
            #(#arg_extractions)*
            #rest_setup
            #body
        }
    };

    // Generate: .method(c"jsName", nargs, Some(native_fn))
    // We need a &'static CStr. Use an unsafe trick with a byte string literal.
    let builder_call = quote! {
        .method(
            unsafe { ::std::ffi::CStr::from_bytes_with_nul_unchecked(#js_name_cstr) },
            #nargs_u32,
            Some(#native_name),
        )
    };

    (native_fn, builder_call)
}

/// Generate a JSNative wrapper for a property getter or setter.
///
/// - Getter: `fn name(&self) -> T` — reads `this`, calls method, sets return value.
/// - Setter: `fn set_name(&mut self, val: T)` — reads `this` mutably, reads `args[0]`, calls method.
fn gen_accessor_native(
    info: &MethodInfo,
    type_name: &Ident,
    _js_name: &str,
    is_getter: bool,
) -> proc_macro2::TokenStream {
    let fn_name = &info.fn_item.sig.ident;
    let native_name = if is_getter {
        format_ident!("__getter_{type_name}_{fn_name}")
    } else {
        format_ident!("__setter_{type_name}_{fn_name}")
    };

    let this_extraction = if is_getter {
        // Getter: &self
        quote! {
            let __self = match ::core_runtime::class::get_this::<#type_name>(&scope, &__args) {
                Ok(v) => v,
                Err(()) => return false,
            };
        }
    } else {
        // Setter: &mut self
        quote! {
            let __self = match ::core_runtime::class::get_this_mut::<#type_name>(&scope, &__args) {
                Ok(v) => v,
                Err(()) => return false,
            };
        }
    };

    let body = if is_getter {
        // Getter: call method, set return value
        let call = if info.has_cx {
            quote! { #type_name::#fn_name(__self, &scope) }
        } else {
            quote! { #type_name::#fn_name(__self) }
        };

        match &info.return_style {
            ReturnStyle::Value => quote! {
                let __result = #call;
                ::core_runtime::class::set_return(&scope, &__args, &__result);
                true
            },
            ReturnStyle::ResultValue => quote! {
                match #call {
                    Ok(__v) => {
                        ::core_runtime::class::set_return(&scope, &__args, &__v);
                        true
                    }
                    Err(__e) => {
                        ::core_runtime::class::ThrowException::throw(__e, &scope);
                        false
                    }
                }
            },
            _ => quote! {
                let __result = #call;
                ::core_runtime::class::set_return(&scope, &__args, &__result);
                true
            },
        }
    } else {
        // Setter: extract arg[0], call method
        let arg_extractions =
            gen_arg_extractions(&info.params, quote!(&__args), false, quote!(&scope));

        let call_args: Vec<_> = info.params.iter().map(|(name, _)| quote!(#name)).collect();
        let call = if info.has_cx {
            quote! { #type_name::#fn_name(__self, &scope, #(#call_args),*) }
        } else {
            quote! { #type_name::#fn_name(__self, #(#call_args),*) }
        };

        match &info.return_style {
            ReturnStyle::ResultVoid => quote! {
                #(#arg_extractions)*
                match #call {
                    Ok(()) => {
                        __args.rval().set(::js::value::undefined());
                        true
                    }
                    Err(__e) => {
                        ::core_runtime::class::ThrowException::throw(__e, &scope);
                        false
                    }
                }
            },
            _ => quote! {
                #(#arg_extractions)*
                #call;
                __args.rval().set(::js::value::undefined());
                true
            },
        }
    };

    quote! {
        #[allow(non_snake_case)]
        unsafe extern "C" fn #native_name(
            raw_cx: *mut ::js::native::RawJSContext,
            argc: u32,
            vp: *mut ::js::native::Value,
        ) -> bool {
            let mut __cx = unsafe { ::js::native::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(raw_cx)) };
            let scope = unsafe { ::js::gc::scope::RootScope::from_current_realm(&mut __cx) };
            let __args = ::js::native::CallArgs::from_vp(vp, argc);
            #this_extraction
            #body
        }
    }
}

// ============================================================================
// #[derive(Traceable)] proc macro
// ============================================================================

/// Derive macro that generates an `unsafe impl Traceable` for a struct.
///
/// Each field is traced by calling `self.field.trace(trc)` unless annotated
/// with `#[no_trace]`, in which case it is skipped.
///
/// # Usage
///
/// ```rust,ignore
/// #[derive(Traceable)]
/// struct MyStruct {
///     js_val: Heap<*mut JSObject>,
///     #[no_trace]
///     plain_data: String,
/// }
/// ```
#[proc_macro_derive(Traceable, attributes(no_trace))]
pub fn derive_traceable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let trace_body = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => {
                let field_traces: Vec<_> = fields
                    .named
                    .iter()
                    .filter(|f| !has_no_trace_attr(f))
                    .map(|f| {
                        let field_name = f.ident.as_ref().unwrap();
                        quote! { self.#field_name.trace(trc); }
                    })
                    .collect();
                quote! { #(#field_traces)* }
            }
            Fields::Unnamed(fields) => {
                let field_traces: Vec<_> = fields
                    .unnamed
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| !has_no_trace_attr(f))
                    .map(|(i, _)| {
                        let idx = syn::Index::from(i);
                        quote! { self.#idx.trace(trc); }
                    })
                    .collect();
                quote! { #(#field_traces)* }
            }
            Fields::Unit => quote! {},
        },
        Data::Enum(_) => {
            panic!("#[derive(Traceable)] is not supported for enums");
        }
        Data::Union(_) => {
            panic!("#[derive(Traceable)] is not supported for unions");
        }
    };

    let output = quote! {
        unsafe impl #impl_generics ::js::heap::Trace for #name #ty_generics #where_clause {
            #[inline]
            unsafe fn trace(&self, trc: *mut ::js::native::JSTracer) {
                #trace_body
            }
        }
    };

    output.into()
}

fn has_no_trace_attr(field: &syn::Field) -> bool {
    field
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("no_trace"))
}

// ============================================================================
// #[jsmodule] attribute macro
// ============================================================================

/// Attribute macro that transforms a `mod` block into a native ES module.
///
/// Public functions become callable JS exports (and remain callable from Rust).
/// Public `const` items become value exports.
///
/// # Usage
///
/// ```rust,ignore
/// #[::core_runtime::jsmodule]
/// mod my_math {
///     pub const PI: f64 = 3.14159;
///     pub fn add(a: f64, b: f64) -> f64 { a + b }
/// }
///
/// // Register:
/// core_runtime::module::register_module::<my_math::js_module>(cx, global);
///
/// // Call from Rust:
/// assert_eq!(my_math::add(1.0, 2.0), 3.0);
/// ```
#[proc_macro_attribute]
pub fn jsmodule(attr: TokenStream, item: TokenStream) -> TokenStream {
    let opts = parse_macro_input!(attr as AttrOpts);
    let input = parse_macro_input!(item as syn::ItemMod);

    let mod_name = &input.ident;
    let mod_vis = &input.vis;
    let js_module_name = opts.name.unwrap_or_else(|| mod_name.to_string());

    let items = match &input.content {
        Some((_, items)) => items,
        None => {
            return syn::Error::new_spanned(&input, "#[jsmodule] requires an inline mod block")
                .to_compile_error()
                .into();
        }
    };

    // Collect public functions and constants
    let mut fn_exports: Vec<ModuleFnExport> = Vec::new();
    let mut const_exports: Vec<ModuleConstExport> = Vec::new();
    let mut original_items: Vec<proc_macro2::TokenStream> = Vec::new();

    for item in items {
        match item {
            syn::Item::Fn(fn_item) if matches!(fn_item.vis, syn::Visibility::Public(_)) => {
                let fn_name = &fn_item.sig.ident;
                let rust_name = fn_name.to_string();
                let js_name = rust_name.to_lower_camel_case();

                // Collect parameter info, filtering out cx and CallArgs params
                let mut params: Vec<(Ident, Type)> = Vec::new();
                let mut has_cx = false;
                let mut is_raw = false;
                for arg in &fn_item.sig.inputs {
                    if let FnArg::Typed(pat_ty) = arg {
                        if is_cx_param_type(&pat_ty.ty) {
                            has_cx = true;
                            continue;
                        }
                        if is_callargs_param_type(&pat_ty.ty) {
                            is_raw = true;
                            continue;
                        }
                        if let Pat::Ident(pat_ident) = pat_ty.pat.as_ref() {
                            params.push((pat_ident.ident.clone(), (*pat_ty.ty).clone()));
                        }
                    }
                }

                let return_style = classify_return_style(&fn_item.sig.output, None, false);

                fn_exports.push(ModuleFnExport {
                    fn_name: fn_name.clone(),
                    js_name,
                    params,
                    return_style,
                    has_cx,
                    is_raw,
                });

                original_items.push(quote! { #fn_item });
            }
            syn::Item::Const(const_item)
                if matches!(const_item.vis, syn::Visibility::Public(_)) =>
            {
                let const_name = &const_item.ident;
                let rust_name = const_name.to_string();
                let js_name = rust_name.to_lower_camel_case();
                let is_ref_type = matches!(&*const_item.ty, Type::Reference(_));

                const_exports.push(ModuleConstExport {
                    const_name: const_name.clone(),
                    js_name,
                    is_ref_type,
                });

                original_items.push(quote! { #const_item });
            }
            other => {
                original_items.push(quote! { #other });
            }
        }
    }

    // Generate JSNative wrappers for each function export
    let mut native_fns: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut declaration_entries: Vec<proc_macro2::TokenStream> = Vec::new();

    for exp in &fn_exports {
        let fn_name = &exp.fn_name;
        let native_name = format_ident!("__native_module_{}", fn_name);
        let js_name = &exp.js_name;
        let nargs = exp.params.len() as u32;

        let arg_extractions =
            gen_arg_extractions(&exp.params, quote!(&args), false, quote!(&scope));
        let call_args: Vec<_> = exp.params.iter().map(|(name, _)| quote!(#name)).collect();

        let call = if exp.is_raw {
            quote! { super::#mod_name::#fn_name(&scope, &args) }
        } else if exp.has_cx {
            quote! { super::#mod_name::#fn_name(&scope, #(#call_args),*) }
        } else {
            quote! { super::#mod_name::#fn_name(#(#call_args),*) }
        };

        let body = match &exp.return_style {
            ReturnStyle::Void => quote! {
                #call;
                args.rval().set(::js::value::undefined());
                true
            },
            ReturnStyle::Value => quote! {
                let result = #call;
                ::core_runtime::class::set_return(&scope, &args, &result);
                true
            },
            ReturnStyle::ResultVoid => quote! {
                match #call {
                    Ok(()) => {
                        args.rval().set(::js::value::undefined());
                        true
                    }
                    Err(e) => {
                        ::core_runtime::class::ThrowException::throw(e, &scope);
                        false
                    }
                }
            },
            ReturnStyle::ResultValue => quote! {
                match #call {
                    Ok(v) => {
                        ::core_runtime::class::set_return(&scope, &args, &v);
                        true
                    }
                    Err(e) => {
                        ::core_runtime::class::ThrowException::throw(e, &scope);
                        false
                    }
                }
            },
            _ => unreachable!("module functions don't use Raw or Promise return styles"),
        };

        native_fns.push(quote! {
            #[allow(non_snake_case)]
            unsafe extern "C" fn #native_name(
                raw_cx: *mut ::js::native::RawJSContext,
                argc: u32,
                vp: *mut ::js::native::Value,
            ) -> bool {
                let mut __cx = unsafe { ::js::native::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(raw_cx)) };
                let scope = unsafe { ::js::gc::scope::RootScope::from_current_realm(&mut __cx) };
                let args = ::js::native::CallArgs::from_vp(vp, argc);
                #(#arg_extractions)*
                #body
            }
        });

        declaration_entries.push(quote! {
            ::core_runtime::module::ModuleExport::Function {
                js_name: #js_name,
                native: Some(#native_name),
                nargs: #nargs,
            }
        });
    }

    // Generate declaration entries for constants
    for exp in &const_exports {
        let js_name = &exp.js_name;
        declaration_entries.push(quote! {
            ::core_runtime::module::ModuleExport::Value {
                js_name: #js_name,
            }
        });
    }

    // Generate the evaluate function that sets constant values
    let mut value_setters: Vec<proc_macro2::TokenStream> = Vec::new();
    for exp in &const_exports {
        let const_name = &exp.const_name;
        let js_name = &exp.js_name;
        // For reference-type constants (e.g. `&str`), pass directly.
        let value_expr = if exp.is_ref_type {
            quote! { super::#mod_name::#const_name }
        } else {
            quote! { &super::#mod_name::#const_name }
        };
        value_setters.push(quote! {
            if !::core_runtime::module::set_module_export(
                scope, env, #js_name, #value_expr,
            ) {
                return false;
            }
        });
    }

    let js_module_name_str = &js_module_name;

    let output = quote! {
        #mod_vis mod #mod_name {
            #(#original_items)*

            /// Generated struct implementing `NativeModule` for this module.
            pub struct js_module;

            #(#native_fns)*

            impl ::core_runtime::module::NativeModule for js_module {
                const NAME: &'static str = #js_module_name_str;

                fn declarations() -> Vec<::core_runtime::module::ModuleExport> {
                    vec![
                        #(#declaration_entries),*
                    ]
                }

                unsafe fn evaluate(
                    scope: &::js::gc::scope::Scope<'_>,
                    env: ::js::native::HandleObject,
                ) -> bool {
                    #(#value_setters)*
                    true
                }
            }

            /// Register this native module so it can be imported from JS.
            ///
            /// Equivalent to `register_module::<js_module>(scope)`.
            pub unsafe fn register(
                scope: &::js::gc::scope::Scope<'_>,
            ) -> bool {
                ::core_runtime::module::register_module::<js_module>(scope)
            }
        }
    };

    output.into()
}

struct ModuleFnExport {
    fn_name: Ident,
    js_name: String,
    params: Vec<(Ident, Type)>,
    return_style: ReturnStyle,
    has_cx: bool,
    is_raw: bool,
}

struct ModuleConstExport {
    const_name: Ident,
    js_name: String,
    is_ref_type: bool,
}

// ============================================================================
// #[jsglobals] attribute macro
// ============================================================================

/// Attribute macro that transforms a `mod` block into a set of global JS definitions.
///
/// Public functions become callable JS functions on the global object.
/// Public `const` items become properties on the global object.
/// `pub use ClassName;` items register `#[jsclass]` classes on the global.
///
/// # Usage
///
/// ```rust,ignore
/// #[::core_runtime::jsglobals]
/// mod my_globals {
///     pub use super::MyClass; // registers MyClass on the global
///     pub const PI: f64 = 3.14159;
///     pub fn greet(name: String) -> String { format!("Hello, {name}!") }
/// }
///
/// // Install on global:
/// my_globals::add_to_global(&scope, global);
/// ```
#[proc_macro_attribute]
pub fn jsglobals(attr: TokenStream, item: TokenStream) -> TokenStream {
    let opts = parse_macro_input!(attr as AttrOpts);
    let _ = opts; // No options used currently
    let input = parse_macro_input!(item as syn::ItemMod);

    let mod_name = &input.ident;
    let mod_vis = &input.vis;

    let items = match &input.content {
        Some((_, items)) => items,
        None => {
            return syn::Error::new_spanned(&input, "#[jsglobals] requires an inline mod block")
                .to_compile_error()
                .into();
        }
    };

    // Collect public functions, constants, and class re-exports
    let mut fn_exports: Vec<ModuleFnExport> = Vec::new();
    let mut const_exports: Vec<ModuleConstExport> = Vec::new();
    let mut class_reexports: Vec<Ident> = Vec::new();
    let mut original_items: Vec<proc_macro2::TokenStream> = Vec::new();

    for item in items {
        match item {
            // `pub use SomeClass;` or `pub use super::SomeClass;` — register a class on the global
            syn::Item::Use(use_item) if matches!(use_item.vis, syn::Visibility::Public(_)) => {
                if let Some(ident) = extract_use_leaf_ident(&use_item.tree) {
                    class_reexports.push(ident.clone());
                }
                // Keep the use item in the module output for Rust visibility
                original_items.push(quote! { #use_item });
            }
            syn::Item::Fn(fn_item) if matches!(fn_item.vis, syn::Visibility::Public(_)) => {
                let fn_name = &fn_item.sig.ident;
                let rust_name = fn_name.to_string();
                let js_name = rust_name.to_lower_camel_case();

                let mut params: Vec<(Ident, Type)> = Vec::new();
                let mut has_cx = false;
                let mut is_raw = false;
                for arg in &fn_item.sig.inputs {
                    if let FnArg::Typed(pat_ty) = arg {
                        if is_cx_param_type(&pat_ty.ty) {
                            has_cx = true;
                            continue;
                        }
                        if is_callargs_param_type(&pat_ty.ty) {
                            is_raw = true;
                            continue;
                        }
                        if let Pat::Ident(pat_ident) = pat_ty.pat.as_ref() {
                            params.push((pat_ident.ident.clone(), (*pat_ty.ty).clone()));
                        }
                    }
                }

                let return_style = classify_return_style(&fn_item.sig.output, None, false);

                fn_exports.push(ModuleFnExport {
                    fn_name: fn_name.clone(),
                    js_name,
                    params,
                    return_style,
                    has_cx,
                    is_raw,
                });

                original_items.push(quote! { #fn_item });
            }
            syn::Item::Const(const_item)
                if matches!(const_item.vis, syn::Visibility::Public(_)) =>
            {
                let const_name = &const_item.ident;
                let rust_name = const_name.to_string();
                let js_name = rust_name.to_lower_camel_case();
                let is_ref_type = matches!(&*const_item.ty, Type::Reference(_));

                const_exports.push(ModuleConstExport {
                    const_name: const_name.clone(),
                    js_name,
                    is_ref_type,
                });

                original_items.push(quote! { #const_item });
            }
            other => {
                original_items.push(quote! { #other });
            }
        }
    }

    // Generate JSNative wrappers for each function export
    let mut native_fns: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut install_calls: Vec<proc_macro2::TokenStream> = Vec::new();

    for exp in &fn_exports {
        let fn_name = &exp.fn_name;
        let native_name = format_ident!("__native_global_{}", fn_name);
        let js_name = &exp.js_name;
        let nargs = exp.params.len() as u32;

        let arg_extractions =
            gen_arg_extractions(&exp.params, quote!(&args), false, quote!(&scope));
        let call_args: Vec<_> = exp.params.iter().map(|(name, _)| quote!(#name)).collect();

        let call = if exp.is_raw {
            quote! { super::#mod_name::#fn_name(&scope, &args) }
        } else if exp.has_cx {
            quote! { super::#mod_name::#fn_name(&scope, #(#call_args),*) }
        } else {
            quote! { super::#mod_name::#fn_name(#(#call_args),*) }
        };

        let body = match &exp.return_style {
            ReturnStyle::Void => quote! {
                #call;
                args.rval().set(::js::value::undefined());
                true
            },
            ReturnStyle::Value => quote! {
                let result = #call;
                ::core_runtime::class::set_return(&scope, &args, &result);
                true
            },
            ReturnStyle::ResultVoid => quote! {
                match #call {
                    Ok(()) => {
                        args.rval().set(::js::value::undefined());
                        true
                    }
                    Err(e) => {
                        ::core_runtime::class::ThrowException::throw(e, &scope);
                        false
                    }
                }
            },
            ReturnStyle::ResultValue => quote! {
                match #call {
                    Ok(v) => {
                        ::core_runtime::class::set_return(&scope, &args, &v);
                        true
                    }
                    Err(e) => {
                        ::core_runtime::class::ThrowException::throw(e, &scope);
                        false
                    }
                }
            },
            _ => unreachable!("global functions don't use Raw or Promise return styles"),
        };

        native_fns.push(quote! {
            #[allow(non_snake_case)]
            unsafe extern "C" fn #native_name(
                raw_cx: *mut ::js::native::RawJSContext,
                argc: u32,
                vp: *mut ::js::native::Value,
            ) -> bool {
                let mut __cx = unsafe { ::js::native::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(raw_cx)) };
                let scope = unsafe { ::js::gc::scope::RootScope::from_current_realm(&mut __cx) };
                let args = ::js::native::CallArgs::from_vp(vp, argc);
                #(#arg_extractions)*
                #body
            }
        });

        let js_name_bytes = format!("{js_name}\0");
        let js_name_cstr = proc_macro2::Literal::byte_string(js_name_bytes.as_bytes());

        install_calls.push(quote! {
            ::js::function::define_function(
                scope,
                global.handle(),
                unsafe { ::std::ffi::CStr::from_bytes_with_nul_unchecked(#js_name_cstr) },
                Some(#native_name),
                #nargs,
                ::js::class_spec::JSPROP_ENUMERATE as u32
            ).unwrap();
        });
    }

    // Generate install calls for constants
    for exp in &const_exports {
        let const_name = &exp.const_name;
        let js_name = &exp.js_name;
        let js_name_bytes = format!("{js_name}\0");
        let js_name_cstr = proc_macro2::Literal::byte_string(js_name_bytes.as_bytes());

        // For reference-type constants (e.g. `&str`), pass the value directly
        // since it's already a reference. For value types, add `&`.
        let value_expr = if exp.is_ref_type {
            quote! { super::#mod_name::#const_name }
        } else {
            quote! { &super::#mod_name::#const_name }
        };

        install_calls.push(quote! {
            ::js::object::define_property(
                scope,
                global.handle(),
                unsafe { ::std::ffi::CStr::from_bytes_with_nul_unchecked(#js_name_cstr) },
                #value_expr,
                ::js::class_spec::JSPROP_ENUMERATE as u32
            ).unwrap();
        });
    }

    // Generate class registration calls — classes are accessed via the
    // `pub use` items that bring them into this module's scope.
    let class_install_calls: Vec<proc_macro2::TokenStream> = class_reexports
        .iter()
        .map(|class_name| {
            quote! {
                #class_name::add_to_global(scope, global);
            }
        })
        .collect();

    let output = quote! {
        #[allow(unused_imports)]
        #mod_vis mod #mod_name {
            #(#original_items)*

            #(#native_fns)*

            /// Install all global functions, constants, and classes onto the given global object.
            pub unsafe fn add_to_global<'s>(
                scope: &'s ::js::gc::scope::Scope<'_>,
                global: ::js::object::Object<'s>,
            ) {
                #(#class_install_calls)*
                #(#install_calls)*
            }
        }
    };

    output.into()
}

// ============================================================================
// #[jsnamespace] / #[webidl_namespace] attribute macros
// ============================================================================

/// Controls codegen differences between `#[jsnamespace]` and `#[webidl_namespace]`.
struct NamespaceConfig {
    /// When `true`, automatically set `Symbol.toStringTag` to the namespace
    /// name (per WebIDL §3.13).
    auto_to_string_tag: bool,
}

impl NamespaceConfig {
    /// Configuration for plain `#[jsnamespace]`: no Symbol.toStringTag.
    const JSNAMESPACE: Self = Self {
        auto_to_string_tag: false,
    };

    /// Configuration for `#[webidl_namespace]`: auto Symbol.toStringTag.
    const WEBIDL_NAMESPACE: Self = Self {
        auto_to_string_tag: true,
    };
}

/// Attribute macro that transforms a `mod` block into a namespace object
/// installed on the global.
///
/// Public functions become methods on the namespace object.
/// No constructor, no prototype chain.
///
/// # Usage
///
/// ```rust,ignore
/// #[jsnamespace(name = "console")]
/// mod console_ns {
///     pub fn log(scope: &Scope<'_>, args: RestArgs) {
///         // ...
///     }
/// }
///
/// // Install on global:
/// console_ns::add_to_global(&scope, global);
/// ```
#[proc_macro_attribute]
pub fn jsnamespace(attr: TokenStream, item: TokenStream) -> TokenStream {
    let opts = parse_macro_input!(attr as AttrOpts);
    let input = parse_macro_input!(item as syn::ItemMod);
    process_namespace(opts, input, NamespaceConfig::JSNAMESPACE)
}

/// Attribute macro for WebIDL namespace definitions.
///
/// Identical to `#[jsnamespace]` but with WebIDL §3.13 semantics:
/// - `Symbol.toStringTag` is automatically set to the namespace name
///
/// # Usage
///
/// ```rust,ignore
/// #[webidl_namespace(name = "CSS")]
/// mod css_ns {
///     pub fn escape(value: String) -> String {
///         // ...
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn webidl_namespace(attr: TokenStream, item: TokenStream) -> TokenStream {
    let opts = parse_macro_input!(attr as AttrOpts);
    let input = parse_macro_input!(item as syn::ItemMod);
    process_namespace(opts, input, NamespaceConfig::WEBIDL_NAMESPACE)
}

/// Shared implementation for `#[jsnamespace]` and `#[webidl_namespace]`.
fn process_namespace(opts: AttrOpts, input: syn::ItemMod, config: NamespaceConfig) -> TokenStream {
    let mod_name = &input.ident;
    let mod_vis = &input.vis;
    let js_ns_name = opts
        .name
        .unwrap_or_else(|| mod_name.to_string().to_lower_camel_case());

    let items = match &input.content {
        Some((_, items)) => items,
        None => {
            return syn::Error::new_spanned(&input, "#[jsnamespace] requires an inline mod block")
                .to_compile_error()
                .into();
        }
    };

    // Collect public functions
    let mut fn_exports: Vec<ModuleFnExport> = Vec::new();
    let mut original_items: Vec<proc_macro2::TokenStream> = Vec::new();

    for item in items {
        match item {
            syn::Item::Fn(fn_item) if matches!(fn_item.vis, syn::Visibility::Public(_)) => {
                let fn_name = &fn_item.sig.ident;
                let rust_name = fn_name.to_string();
                let js_name = rust_name.to_lower_camel_case();

                let mut params: Vec<(Ident, Type)> = Vec::new();
                let mut has_cx = false;
                let mut is_raw = false;
                for arg in &fn_item.sig.inputs {
                    if let FnArg::Typed(pat_ty) = arg {
                        if is_cx_param_type(&pat_ty.ty) {
                            has_cx = true;
                            continue;
                        }
                        if is_callargs_param_type(&pat_ty.ty) {
                            is_raw = true;
                            continue;
                        }
                        if let Pat::Ident(pat_ident) = pat_ty.pat.as_ref() {
                            params.push((pat_ident.ident.clone(), (*pat_ty.ty).clone()));
                        }
                    }
                }

                let return_style = classify_return_style(&fn_item.sig.output, None, false);

                fn_exports.push(ModuleFnExport {
                    fn_name: fn_name.clone(),
                    js_name,
                    params,
                    return_style,
                    has_cx,
                    is_raw,
                });

                original_items.push(quote! { #fn_item });
            }
            other => {
                original_items.push(quote! { #other });
            }
        }
    }

    // Generate JSNative wrappers and install calls for each function
    let mut native_fns: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut install_calls: Vec<proc_macro2::TokenStream> = Vec::new();

    for exp in &fn_exports {
        let fn_name = &exp.fn_name;
        let native_name = format_ident!("__native_ns_{}", fn_name);
        let js_name = &exp.js_name;
        let nargs = exp.params.len() as u32;

        let arg_extractions =
            gen_arg_extractions(&exp.params, quote!(&args), false, quote!(&scope));
        let call_args: Vec<_> = exp.params.iter().map(|(name, _)| quote!(#name)).collect();

        let call = if exp.is_raw {
            quote! { super::#mod_name::#fn_name(&scope, &args) }
        } else if exp.has_cx {
            quote! { super::#mod_name::#fn_name(&scope, #(#call_args),*) }
        } else {
            quote! { super::#mod_name::#fn_name(#(#call_args),*) }
        };

        let body = match &exp.return_style {
            ReturnStyle::Void => quote! {
                #call;
                args.rval().set(::js::value::undefined());
                true
            },
            ReturnStyle::Value => quote! {
                let result = #call;
                ::core_runtime::class::set_return(&scope, &args, &result);
                true
            },
            ReturnStyle::ResultVoid => quote! {
                match #call {
                    Ok(()) => {
                        args.rval().set(::js::value::undefined());
                        true
                    }
                    Err(e) => {
                        ::core_runtime::class::ThrowException::throw(e, &scope);
                        false
                    }
                }
            },
            ReturnStyle::ResultValue => quote! {
                match #call {
                    Ok(v) => {
                        ::core_runtime::class::set_return(&scope, &args, &v);
                        true
                    }
                    Err(e) => {
                        ::core_runtime::class::ThrowException::throw(e, &scope);
                        false
                    }
                }
            },
            _ => unreachable!("namespace functions don't use Raw or Promise return styles"),
        };

        native_fns.push(quote! {
            #[allow(non_snake_case)]
            unsafe extern "C" fn #native_name(
                raw_cx: *mut ::js::native::RawJSContext,
                argc: u32,
                vp: *mut ::js::native::Value,
            ) -> bool {
                let mut __cx = unsafe { ::js::native::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(raw_cx)) };
                let scope = unsafe { ::js::gc::scope::RootScope::from_current_realm(&mut __cx) };
                let args = ::js::native::CallArgs::from_vp(vp, argc);
                #(#arg_extractions)*
                #body
            }
        });

        let js_name_bytes = format!("{js_name}\0");
        let js_name_cstr = proc_macro2::Literal::byte_string(js_name_bytes.as_bytes());

        install_calls.push(quote! {
            ::js::function::define_function(
                scope,
                ns_handle,
                unsafe { ::std::ffi::CStr::from_bytes_with_nul_unchecked(#js_name_cstr) },
                Some(#native_name),
                #nargs,
                ::js::class_spec::JSPROP_ENUMERATE as u32
            ).unwrap();
        });
    }

    // Generate Symbol.toStringTag installation for webidl_namespace
    let to_string_tag_install = if config.auto_to_string_tag {
        quote! {
            ::core_runtime::class::define_to_string_tag(scope, ns_handle, #js_ns_name);
        }
    } else {
        quote! {}
    };

    let js_ns_name_bytes = format!("{js_ns_name}\0");
    let js_ns_name_cstr = proc_macro2::Literal::byte_string(js_ns_name_bytes.as_bytes());

    let output = quote! {
        #[allow(unused_imports)]
        #mod_vis mod #mod_name {
            #(#original_items)*

            #(#native_fns)*

            /// Install this namespace onto the given global object.
            pub unsafe fn add_to_global<'s>(
                scope: &'s ::js::gc::scope::Scope<'_>,
                global: ::js::object::Object<'s>,
            ) {
                // Create a plain object for the namespace.
                let ns_obj = ::js::object::Object::new_plain(scope)
                    .expect("failed to create namespace object");
                let ns_handle = ns_obj.handle();

                // Install functions on the namespace object.
                #(#install_calls)*

                // Install Symbol.toStringTag if applicable.
                #to_string_tag_install

                // Set the namespace on the global.
                let ns_name = unsafe {
                    ::std::ffi::CStr::from_bytes_with_nul_unchecked(#js_ns_name_cstr)
                };
                let ns_val = unsafe { ::js::value::from_object(ns_obj.as_raw()) };
                let ns_val_handle = scope.root_value(ns_val);
                global.set_property(scope, ns_name, ns_val_handle)
                    .expect("failed to set namespace on global");
            }
        }
    };

    output.into()
}

#[proc_macro_attribute]
pub fn must_root(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item: proc_macro2::TokenStream = item.into();
    quote! {
        #[cfg_attr(crown, crown::unrooted_must_root_lint::must_root)]
        #item
    }
    .into()
}

#[proc_macro_attribute]
pub fn allow_unrooted(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item: proc_macro2::TokenStream = item.into();
    quote! {
        #[cfg_attr(crown, allow(crown::unrooted_must_root))]
        #item
    }
    .into()
}

#[proc_macro_attribute]
pub fn allow_unrooted_interior(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item: proc_macro2::TokenStream = item.into();
    quote! {
        #[cfg_attr(crown, crown::unrooted_must_root_lint::allow_unrooted_interior)]
        #item
    }
    .into()
}

#[proc_macro_attribute]
pub fn allow_unrooted_interior_in_rc(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item: proc_macro2::TokenStream = item.into();
    quote! {
        #[cfg_attr(crown, crown::unrooted_must_root_lint::allow_unrooted_interior_in_rc)]
        #item
    }
    .into()
}
