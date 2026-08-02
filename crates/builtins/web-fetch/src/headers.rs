// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://fetch.spec.whatwg.org/>

use core_runtime::{webidl_interface, webidl_methods, webidl_union};
use js::class::{get_iterator_prototype, get_prototype_for};
use js::conversion::{Record, ToJSVal};
use js::error::{throw_type_error, ExnThrown};
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::prelude::HandleValue;
use js::{Function, Object};

use crate::algorithms;
use crate::algorithms::sort_and_combine_a_header_list;
use crate::byte_string::ByteString;

/// A header list: an ordered list of (name, value) pairs.
///
/// <https://fetch.spec.whatwg.org/#concept-header-list>
///
/// Header names are ASCII tokens, so byte-case-insensitive comparison is ASCII
/// case-insensitive.
///
/// Values are stored as the strings the WebIDL `ByteString` conversion produced,
/// whose code units are all ≤ 0xFF, each standing for one byte. The conversion
/// to and from wire bytes is `platform::http::isomorphic_encode`/`_decode`, so
/// `é` (U+00E9) is the single byte 0xE9 — not its UTF-8 encoding.
// TODO: consider interning all header names, along with lower-cased versions of them.
// Entries would then become `(interned(name), interned(lower-cased(name)), value)`.
// This should ideally support returning `&'static str` references to the names.
// If that's too much of a DoS vector, Cow might work instead, with the most common
// names being interned.
pub(crate) type HeaderList = Vec<(String, String)>;

/// A headers guard.
///
/// <https://fetch.spec.whatwg.org/#concept-headers-guard>
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Guard {
    /// `"none"`
    #[default]
    None,
    /// `"immutable"`
    Immutable,
    /// `"request"`
    Request,
    /// `"request-no-cors"`
    RequestNoCors,
    /// `"response"`
    Response,
}

/// WebIDL `typedef (sequence<sequence<ByteString>> or record<ByteString, ByteString>) HeadersInit`
#[webidl_union]
pub enum HeadersInit {
    Sequence(Vec<Vec<ByteString>>),
    Record(Record<ByteString, ByteString>),
}

/// <https://fetch.spec.whatwg.org/#headers-class>
#[webidl_interface]
pub struct Headers {
    /// <https://fetch.spec.whatwg.org/#concept-headers-header-list>
    /// (a header list), which is initially empty. The spec allows this to be a pointer to
    /// another object's header list (e.g. a request's); here the `Headers` object owns its
    /// list and `Request`/`Response` reference the `Headers` object.
    pub(crate) header_list: HeaderList,
    /// <https://fetch.spec.whatwg.org/#concept-headers-guard>
    /// which is a headers guard. A headers guard is "immutable", "request", "request-no-cors",
    /// "response" or "none".
    #[no_trace]
    pub(crate) guard: Guard,
}

#[webidl_methods]
impl Headers {
    /// <https://fetch.spec.whatwg.org/#dom-headers>
    #[constructor]
    fn new(&self, scope: &Scope<'_>, init: Option<HeadersInit>) -> Result<(), ExnThrown> {
        // Step 1: Set `this`’s `guard` to "`none`".
        self.data_mut().guard = Guard::None;
        // Step 2: If _init_ is given, then `fill` `this` with _init_.
        if let Some(init) = init {
            algorithms::fill_headers(scope, self, init)?;
        }
        Ok(())
    }

    /// Create a `Headers` instance from a header list and guard.
    pub fn from_list(list: HeaderList, guard: Guard) -> Self {
        Self {
            header_list: list,
            guard,
        }
    }

    /// <https://fetch.spec.whatwg.org/#dom-headers-append>
    #[method]
    pub fn append(
        &self,
        scope: &Scope<'_>,
        name: ByteString,
        value: ByteString,
    ) -> Result<(), ExnThrown> {
        // Step 1: Append (name, value) to this.
        algorithms::append_to_headers(scope, self, name.into_string(), value.into_string())
    }

    /// <https://fetch.spec.whatwg.org/#dom-headers-delete>
    #[method]
    pub fn delete(&self, scope: &Scope<'_>, name: ByteString) -> Result<(), ExnThrown> {
        let name = name.as_str();
        // Step 1: If `validating` (_name_, `) for `this` returns false, then return. Passing a
        //     dummy `header value` ought not to have any negative repercussions.
        if !algorithms::validate_header_name(scope, self, name, "")? {
            return Ok(());
        }
        // Step 2: If `this`’s `guard` is "`request-no-cors`", _name_ is not a `no-CORS-safelisted
        //     request-header name`, and _name_ is not a `privileged no-CORS request-header name`,
        //     then return.
        if self.data().guard == Guard::RequestNoCors
            && !algorithms::is_no_cors_safelisted_request_header_name(name)
            && !algorithms::is_privileged_no_cors_request_header_name(name)
        {
            return Ok(());
        }

        // Step 3: If `this`’s `header list` `does not contain` _name_, then return.
        if !algorithms::contains(&self.data().header_list, name) {
            return Ok(());
        }

        // Step 4: `Delete` _name_ from `this`’s `header list`.
        algorithms::delete_header(&mut self.data_mut().header_list, name);

        // Step 5: If `this`’s `guard` is "`request-no-cors`", then `remove privileged no-CORS
        //     request-headers` from `this`.
        // Note: at first glance, it seems very random for no-CORS headers to be removed here.
        // The rationale is that they persist as long as content doesn't apply *any* modifications
        // to the `Headers` object.
        // That also means that the early return in step 3 can't be omitted, because while the
        // check happens implicitly in step 4, the early return doesn't.
        if self.data().guard == Guard::RequestNoCors {
            algorithms::remove_privileged_no_cors_request_headers(self);
        }
        Ok(())
    }

    /// <https://fetch.spec.whatwg.org/#dom-headers-get>
    #[method]
    pub fn get(&self, scope: &Scope<'_>, name: ByteString) -> Result<Option<String>, ExnThrown> {
        let name = name.as_str();
        // Step 1: If _name_ is not a `header name`, then `throw` a `TypeError`.
        if !algorithms::is_header_name(name) {
            return Err(throw_type_error(scope, c"Invalid header name"));
        }
        // Step 2: Return the result of `getting` _name_ from `this`’s `header list`.
        let data = self.data();
        Ok(algorithms::get_header_name(&data.header_list, name).map(std::borrow::Cow::into_owned))
    }

    /// <https://fetch.spec.whatwg.org/#dom-headers-getsetcookie>
    #[method]
    pub fn get_set_cookie(&self, scope: &Scope<'_>) -> Result<Vec<String>, ExnThrown> {
        let _ = scope;
        // Step 1: If `this`’s `header list` `does not contain` `Set-Cookie`, then return « ».
        // Step 2: Return the `values` of all `headers` in `this`’s `header list` whose `name` is a
        //     `byte-case-insensitive` match for `Set-Cookie`, in order.
        // An empty header list yields an empty sequence, subsuming step 1.
        Ok(self
            .data()
            .header_list
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
            .map(|(_, value)| value.clone())
            .collect())
    }

    /// <https://fetch.spec.whatwg.org/#dom-headers-has>
    #[method]
    pub fn has(&self, scope: &Scope<'_>, name: ByteString) -> Result<bool, ExnThrown> {
        let name = name.as_str();
        // Step 1: If _name_ is not a `header name`, then `throw` a `TypeError`.
        if !algorithms::is_header_name(name) {
            return Err(throw_type_error(scope, c"Invalid header name"));
        }
        // Step 2: Return true if `this`’s `header list` `contains` _name_; otherwise false.
        Ok(algorithms::contains(&self.data().header_list, name))
    }

    /// <https://fetch.spec.whatwg.org/#dom-headers-set>
    #[method]
    pub fn set(
        &self,
        scope: &Scope<'_>,
        name: ByteString,
        value: ByteString,
    ) -> Result<(), ExnThrown> {
        let name = name.into_string();
        // Step 1: `Normalize` _value_.
        let value = algorithms::normalize_byte_sequence(value.into_string());
        // Step 2: If `validating` (_name_, _value_) for `this` returns false, then return.
        if !algorithms::validate_header_name(scope, self, &name, &value)? {
            return Ok(());
        }
        // Step 3: If `this`’s `guard` is "`request-no-cors`" and (_name_, _value_) is not a
        //     `no-CORS-safelisted request-header`, then return.
        if self.data().guard == Guard::RequestNoCors
            && !algorithms::is_no_cors_safelisted_request_header(&name, &value)
        {
            return Ok(());
        }
        // Step 4: `Set` (_name_, _value_) in `this`’s `header list`.
        algorithms::set_a_header(&mut self.data_mut().header_list, name, value);
        // Step 5: If `this`’s `guard` is "`request-no-cors`", then `remove privileged no-CORS
        //     request-headers` from `this`.
        if self.data().guard == Guard::RequestNoCors {
            algorithms::remove_privileged_no_cors_request_headers(self);
        }
        Ok(())
    }

    /// <https://webidl.spec.whatwg.org/#es-iterable> — `entries()` yields `[name, value]`
    /// pairs over the result of `sort and combine` of this's header list.
    #[method]
    fn entries<'r>(&self, scope: &'r Scope<'_>) -> Result<HeadersIterator<'r>, ExnThrown> {
        self.create_iterator(scope, IteratorKind::Entries)
    }

    /// <https://webidl.spec.whatwg.org/#es-iterable> — `keys()` yields header names.
    #[method]
    fn keys<'r>(&self, scope: &'r Scope<'_>) -> Result<HeadersIterator<'r>, ExnThrown> {
        self.create_iterator(scope, IteratorKind::Keys)
    }

    /// <https://webidl.spec.whatwg.org/#es-iterable> — `values()` yields header values.
    #[method]
    fn values<'r>(&self, scope: &'r Scope<'_>) -> Result<HeadersIterator<'r>, ExnThrown> {
        self.create_iterator(scope, IteratorKind::Values)
    }

    /// <https://webidl.spec.whatwg.org/#es-forEach> — invoke `callback` once per (value, name)
    /// over the sorted-and-combined header list.
    #[method(name = "forEach")]
    fn for_each(
        &self,
        scope: &Scope<'_>,
        callback: HandleValue,
        this_arg: Option<HandleValue>,
    ) -> Result<(), ExnThrown> {
        // WebIDL converts the argument to a callback function, which requires it to be callable —
        // a non-callable object is a TypeError, not something to be called and fail later.
        let is_callable =
            Object::from_value(scope, callback.get()).is_ok_and(|object| object.is_callable());
        if !is_callable {
            return Err(throw_type_error(
                scope,
                c"forEach callback must be a function",
            ));
        }
        // WebIDL's `forEach` re-reads "the list of value pairs to iterate over" on every turn, so
        // a callback that appends to or deletes from these headers changes what the rest of the
        // iteration sees. Recompute rather than iterating a snapshot taken up front.
        let mut index = 0;
        loop {
            let combined = algorithms::sort_and_combine_a_header_list(&self.data().header_list);
            let Some((name, value)) = combined.get(index) else {
                break;
            };
            let value_js = value.as_str().to_jsval_throwing(scope)?;
            let name_js = name.as_str().to_jsval_throwing(scope)?;
            let self_js = scope.root_value(self.as_value());
            Function::call(
                scope,
                this_arg.unwrap_or(HandleValue::undefined()),
                callback,
                &[value_js, name_js, self_js],
            )?;
            index += 1;
        }
        Ok(())
    }
}

impl Headers<'_> {
    /// Create a `HeadersIterator` over this object's headers for the given kind.
    fn create_iterator<'r>(
        &self,
        scope: &'r Scope<'_>,
        kind: IteratorKind,
    ) -> Result<HeadersIterator<'r>, ExnThrown> {
        HeadersIterator::new(scope, *self, kind)
    }

    /// Define `Symbol.iterator` on `Headers.prototype` (an alias of `entries`).
    fn install_symbol_iterator(scope: &Scope<'_>) {
        js::class::add_symbol_alias::<HeadersImpl>(
            scope,
            c"entries",
            js::native::SymbolCode::iterator,
        );
    }
}

/// Which kind of values the iterator produces.
#[derive(Clone, Copy, Default)]
pub enum IteratorKind {
    /// Yields `[name, value]` pairs.
    #[default]
    Entries,
    /// Yields names only.
    Keys,
    /// Yields values only.
    Values,
}

/// <https://webidl.spec.whatwg.org/#es-iterable>
#[webidl_interface(hidden, name = "Headers Iterator")]
pub struct HeadersIterator {
    /// The `Headers` object being iterated.
    pub(crate) headers: Heap<HeadersImpl>,
    /// Current position in the sorted-and-combined list.
    pub(crate) index: usize,
    /// What kind of values to yield.
    #[no_trace]
    pub(crate) kind: IteratorKind,
}

#[webidl_methods]
impl HeadersIterator {
    fn new(headers: Headers, kind: IteratorKind) -> Self {
        Self {
            headers: Heap::from(headers),
            index: 0,
            kind,
        }
    }

    /// <https://webidl.spec.whatwg.org/#es-iterator-prototype-next>
    #[method]
    fn next<'a>(&self, scope: &'a Scope<'a>) -> Result<Object<'a>, ExnThrown> {
        let result = Object::new(scope, None)?;
        let index = self.data().index;
        let kind = self.data().kind;
        let headers = self.data().headers.get(scope);
        let combined = sort_and_combine_a_header_list(&headers.data().header_list);

        if let Some((name, value)) = combined.get(index) {
            let js_value = match kind {
                IteratorKind::Entries => {
                    let arr = js::Array::new(scope, 2)?;
                    let name_val = name.as_str().to_jsval_throwing(scope)?;
                    let val_val = value.as_str().to_jsval_throwing(scope)?;
                    arr.set_element(scope, 0, name_val)?;
                    arr.set_element(scope, 1, val_val)?;
                    scope.root_value(arr.as_value())
                }
                IteratorKind::Keys => name.as_str().to_jsval_throwing(scope)?,
                IteratorKind::Values => value.as_str().to_jsval_throwing(scope)?,
            };
            self.data_mut().index = index + 1;
            result.set_property(scope, c"value", js_value)?;
            result.set_property(scope, c"done", false)?;
        } else {
            result.set_property(scope, c"value", js::value::undefined())?;
            result.set_property(scope, c"done", true)?;
        }

        Ok(result)
    }

    /// Chain the `HeadersIterator` prototype under `%IteratorPrototype%` to satisfy
    /// the interface's `iterable<>` declaration.
    fn install_symbol_iterator(scope: &Scope<'_>) {
        let proto = unsafe {
            Object::from_raw(
                scope,
                get_prototype_for::<HeadersIteratorImpl>(scope)
                    .expect("HeadersIterator class not registered"),
            )
            .expect("HeadersIterator prototype is null")
        };

        if let Ok(iterator_proto) = get_iterator_prototype(scope) {
            let _ = proto.set_prototype(scope, iterator_proto.handle());
        }
    }
}

pub(crate) fn add_to_global(scope: &Scope, global: Object) {
    Headers::add_to_global(scope, global);
    Headers::install_symbol_iterator(scope);
    HeadersIterator::add_to_global(scope, global);
    HeadersIterator::install_symbol_iterator(scope);
}
