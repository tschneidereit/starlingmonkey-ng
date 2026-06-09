// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://url.spec.whatwg.org/>

use crate::url_search_params_iterator::{
    IteratorKind, URLSearchParamsIterator, URLSearchParamsIteratorImpl,
};
use core_runtime::{webidl_interface, webidl_methods, webidl_union};
use js::class::get_prototype_for;
use js::conversion::{Record, ToJSVal};
use js::error::{throw_type_error, ExnThrown, TypeError};
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::prelude::HandleValue;

/// WebIDL: `(sequence<sequence<USVString>> or record<USVString, USVString> or USVString)`
/// — the union accepted by the [`URLSearchParams` constructor].
///
/// [`URLSearchParams` constructor]: https://url.spec.whatwg.org/#dom-urlsearchparams-urlsearchparams
#[webidl_union]
pub enum URLSearchParamsInit {
    Pairs(Vec<Vec<String>>),
    Record(Record<String>),
    Str(String),
}

/// <https://url.spec.whatwg.org/#interface-urlsearchparams>
#[webidl_interface]
pub struct URLSearchParams {
    pub(crate) list: Vec<(String, String)>,
    pub(crate) url_object: Option<Heap<crate::url::URLImpl>>,
}

#[webidl_methods]
impl URLSearchParams<'_> {
    /// <https://url.spec.whatwg.org/#dom-urlsearchparams-urlsearchparams>
    #[constructor]
    fn new(&self, init: Option<URLSearchParamsInit>) -> Result<(), TypeError> {
        // Step 2: `Initialize` `this` with _init_.
        let list = match init {
            None => Vec::new(),
            Some(URLSearchParamsInit::Pairs(pairs)) => {
                let mut list = Vec::with_capacity(pairs.len());
                for pair in pairs {
                    // Each inner sequence must contain exactly two items.
                    if pair.len() != 2 {
                        return Err(TypeError(
                            "Each URLSearchParams sequence element must contain exactly two items"
                                .into(),
                        ));
                    }
                    let mut iter = pair.into_iter();
                    let name = iter.next().unwrap();
                    let value = iter.next().unwrap();
                    list.push((name, value));
                }
                list
            }
            Some(URLSearchParamsInit::Record(record)) => record.into_iter().collect(),
            Some(URLSearchParamsInit::Str(mut s)) => {
                // Step 1: If _init_ is a string and starts with U+003F (?), then remove
                //         the first code point from _init_.
                if s.starts_with('?') {
                    s.remove(0);
                }
                form_urlencoded::parse(s.as_bytes()).into_owned().collect()
            }
        };

        self.data_mut().list = list;
        Ok(())
    }

    /// <https://url.spec.whatwg.org/#dom-urlsearchparams-size>
    #[getter]
    fn size(&self) -> u32 {
        // Step 1: Return this’s list’s size.
        self.data().list.len() as u32
    }

    /// <https://url.spec.whatwg.org/#dom-urlsearchparams-append>
    #[method]
    fn append(&self, scope: &Scope<'_>, name: String, value: String) -> Result<(), ExnThrown> {
        // Step 1: `Append` (_name_, _value_) to `this`’s `list`.
        self.data_mut().list.push((name, value));
        // Step 2: `Update` `this`.
        self.update(scope)
    }

    /// <https://url.spec.whatwg.org/#dom-urlsearchparams-delete>
    #[method]
    fn delete(
        &self,
        scope: &Scope<'_>,
        name: String,
        value: Option<String>,
    ) -> Result<(), ExnThrown> {
        // Step 1: If _value_ is given, then `remove` all `tuples` whose name is _name_ and value is
        //         _value_ from `this`’s `list`.
        // Step 2: Otherwise, `remove` all `tuples` whose name is _name_ from `this`’s `list`.
        match value {
            Some(value) => {
                self.data_mut().list.retain(|(tuple_name, tuple_value)| {
                    tuple_name != &name || tuple_value != &value
                });
            }
            None => {
                self.data_mut()
                    .list
                    .retain(|(tuple_name, _)| tuple_name != &name);
            }
        }

        // Step 3: `Update` `this`.
        self.update(scope)
    }

    /// <https://url.spec.whatwg.org/#dom-urlsearchparams-get>
    #[method]
    fn get(&self, scope: &Scope<'_>, name: String) -> Result<Option<String>, ExnThrown> {
        // Step 1: Return the value of the first tuple whose name is name in this’s list, if there
        //         is such a tuple; otherwise null.
        let _ = scope;
        Ok(self
            .data()
            .list
            .iter()
            .find(|(tuple_name, _)| tuple_name == &name)
            .map(|(_, value)| value.clone()))
    }

    /// <https://url.spec.whatwg.org/#dom-urlsearchparams-getall>
    #[method]
    fn get_all(&self, scope: &Scope<'_>, name: String) -> Result<Vec<String>, ExnThrown> {
        // Step 1: Return the values of all tuples whose name is name in this’s list, in list
        //         order; otherwise the empty sequence.
        let _ = scope;
        Ok(self
            .data()
            .list
            .iter()
            .filter(|(tuple_name, _)| tuple_name == &name)
            .map(|(_, value)| value.clone())
            .collect())
    }

    /// <https://url.spec.whatwg.org/#dom-urlsearchparams-has>
    #[method]
    fn has(
        &self,
        scope: &Scope<'_>,
        name: String,
        value: Option<String>,
    ) -> Result<bool, ExnThrown> {
        // Step 1: If _value_ is given and there is a `tuple` whose name is _name_ and value is
        //         _value_ in `this`’s `list`, then return true.
        // Step 2: If _value_ is not given and there is a `tuple` whose name is _name_ in `this`’s
        //         `list`, then return true.
        let _ = scope;
        if let Some(value) = value {
            return Ok(self
                .data()
                .list
                .iter()
                .any(|(tuple_name, tuple_value)| tuple_name == &name && tuple_value == &value));
        }

        if self
            .data()
            .list
            .iter()
            .any(|(tuple_name, _)| tuple_name == &name)
        {
            return Ok(true);
        }

        // Step 3: Return false.
        Ok(false)
    }

    /// <https://url.spec.whatwg.org/#dom-urlsearchparams-set>
    #[method]
    fn set(&self, scope: &Scope<'_>, name: String, value: String) -> Result<(), ExnThrown> {
        // Step 1: If `this`’s `list` `contains` any `tuples` whose name is _name_, then set the
        //         value of the first such `tuple` to _value_ and `remove` the others.
        // Step 2: Otherwise, `append` (_name_, _value_) to `this`’s `list`.
        if let Some(first_index) = self
            .data()
            .list
            .iter()
            .position(|(tuple_name, _)| tuple_name == &name)
        {
            self.data_mut().list[first_index].1 = value.clone();
            let mut seen_first = false;
            self.data_mut().list.retain(|(tuple_name, _)| {
                if tuple_name != &name {
                    return true;
                }
                if !seen_first {
                    seen_first = true;
                    true
                } else {
                    false
                }
            });
        } else {
            self.data_mut().list.push((name, value));
        }

        // Step 3: `Update` `this`.
        self.update(scope)
    }

    /// <https://url.spec.whatwg.org/#dom-urlsearchparams-sort>
    #[method]
    fn sort(&self, scope: &Scope<'_>) -> Result<(), ExnThrown> {
        // Step 1: Set `this`’s `list` to the result of `sorting in ascending order` `this`’s
        //         `list`, with _a_ being less than _b_ if _a_’s name is `code unit less than`
        //         _b_’s name.
        self.data_mut().list.sort_by(|(a_name, _), (b_name, _)| {
            if a_name.is_ascii() {
                a_name.cmp(b_name)
            } else {
                a_name.encode_utf16().cmp(b_name.encode_utf16())
            }
        });

        // Step 2: `Update` `this`.
        self.update(scope)
    }

    #[method]
    fn for_each(
        &self,
        scope: &Scope<'_>,
        callback: HandleValue,
        this_arg: Option<HandleValue>,
    ) -> Result<(), ExnThrown> {
        if !callback.is_object() {
            return Err(throw_type_error(scope, c"callback must be a function"));
        }

        let mut index = 0usize;
        while index < self.data().list.len() {
            let (name, value) = self.data().list[index].clone();
            let value_js = value.as_str().to_jsval(scope).map_err(|e| e.throw(scope))?;
            let name_js = name.as_str().to_jsval(scope).map_err(|e| e.throw(scope))?;
            let self_js = scope.root_value(self.as_value());

            js::Function::call(
                scope,
                this_arg.unwrap_or(HandleValue::undefined()),
                callback,
                &[value_js, name_js, self_js],
            )?;
            index += 1;
        }

        Ok(())
    }

    #[allow(clippy::wrong_self_convention)]
    #[method]
    fn to_string(&self, _scope: &Scope<'_>) -> Result<String, ExnThrown> {
        let mut serializer = form_urlencoded::Serializer::new(String::new());
        for (name, value) in &self.data().list {
            serializer.append_pair(name, value);
        }
        Ok(serializer.finish())
    }

    #[method]
    fn entries<'r>(&self, scope: &'r Scope<'_>) -> Result<URLSearchParamsIterator<'r>, ExnThrown> {
        self.create_iterator(scope, IteratorKind::Entries)
    }

    #[method]
    fn keys<'r>(&self, scope: &'r Scope<'_>) -> Result<URLSearchParamsIterator<'r>, ExnThrown> {
        self.create_iterator(scope, IteratorKind::Keys)
    }

    #[method]
    fn values<'r>(&self, scope: &'r Scope<'_>) -> Result<URLSearchParamsIterator<'r>, ExnThrown> {
        self.create_iterator(scope, IteratorKind::Values)
    }
}

/// Define `Symbol.iterator` on URLSearchParams.prototype.
pub fn install_symbol_iterator(scope: &Scope<'_>) {
    let proto = unsafe {
        js::Object::from_raw(
            scope,
            get_prototype_for::<URLSearchParamsImpl>(scope)
                .expect("URLSearchParams class not registered"),
        )
        .expect("URLSearchParams prototype is null")
    };

    let iterator_fn = js::Function::new_callback(
        scope,
        c"[Symbol.iterator]",
        0,
        |scope, args, _payload| {
            let this_obj = js::Object::from_value(scope, args.this())
                .map_err(|_| throw_type_error(scope, c"Invalid URLSearchParams receiver"))?;
            js::Function::call_by_name(scope, this_obj.handle(), c"entries", &[])
                .map(|value| *value)
        },
        js::value::undefined(),
    )
    .expect("failed to create URLSearchParams @@iterator function");

    let iter_key = js::symbol::get_well_known_key(scope, js::native::SymbolCode::iterator);
    let iter_id = scope.root_id(iter_key);
    let fn_val = scope.root_value(iterator_fn.as_value());
    proto
        .set_property_by_id(scope, iter_id, fn_val)
        .expect("failed to define Symbol.iterator on URLSearchParams prototype");
}

impl URLSearchParams<'_> {
    pub(crate) fn update(&self, scope: &Scope<'_>) -> Result<(), ExnThrown> {
        let Some(url_object_heap) = self.data().url_object.as_ref() else {
            return Ok(());
        };

        let url_object = url_object_heap.get(scope);

        let mut serializer = form_urlencoded::Serializer::new(String::new());
        for (name, value) in &self.data().list {
            serializer.append_pair(name, value);
        }
        let serialized = serializer.finish();
        let serialized_query = if serialized.is_empty() {
            None
        } else {
            Some(serialized)
        };

        if let Some(url) = url_object.data_mut().url.as_mut() {
            // Per the URL spec's opaque-path state, a trailing U+0020 SPACE before a query
            // delimiter is stored literally, but must be percent-encoded as %20 when the query
            // is removed and that space becomes a true trailing character.
            if serialized_query.is_none() && url.cannot_be_a_base() {
                let path = url.path().to_string();
                if path.ends_with(' ') {
                    let trimmed = path.trim_end_matches(' ');
                    let trailing_spaces = path.len() - trimmed.len();
                    let rebuilt = format!("{}{}%20", trimmed, " ".repeat(trailing_spaces - 1));
                    url.set_path(&rebuilt);
                }
            }
            url.set_query(serialized_query.as_deref());
        }
        Ok(())
    }

    fn create_iterator<'r>(
        &self,
        scope: &'r Scope<'_>,
        kind: IteratorKind,
    ) -> Result<URLSearchParamsIterator<'r>, ExnThrown> {
        let iter_object = unsafe {
            js::class::create_instance_with::<URLSearchParamsIteratorImpl>(scope, |_| {
                URLSearchParamsIteratorImpl {
                    params: (*self).into(),
                    index: 0,
                    kind,
                }
            })
        }?;
        iter_object
            .cast::<URLSearchParamsIterator>()
            .map_err(|_| throw_type_error(scope, c"Failed to create URLSearchParams iterator"))
    }
}
