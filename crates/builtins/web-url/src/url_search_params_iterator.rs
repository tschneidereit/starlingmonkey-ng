// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Iterator for URLSearchParams (entries, keys, values).
//!
//! Implements the WebIDL `iterable<USVString, USVString>` declaration for
//! URLSearchParams.  Creates iterator objects that follow the standard
//! iterator protocol (`next()` returning `{value, done}`).

use crate::url_search_params::{URLSearchParams, URLSearchParamsImpl};

use core_runtime::{webidl_interface, webidl_methods};
use js::class::get_prototype_for;
use js::conversion::ToJSVal;
use js::error::ExnThrown;
use js::gc::handle::Heap;
use js::gc::scope::Scope;

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
#[webidl_interface(name = "URLSearchParams Iterator")]
pub struct URLSearchParamsIterator {
    /// Reference to the URLSearchParams being iterated.
    pub(crate) params: Heap<URLSearchParamsImpl>,
    /// Current position in the list.
    pub(crate) index: usize,
    /// What kind of values to yield.
    #[no_trace]
    pub(crate) kind: IteratorKind,
}

#[webidl_methods]
impl URLSearchParamsIterator {
    #[constructor]
    fn new(&self, _scope: &Scope<'_>) -> Result<(), js::error::TypeError> {
        Err(js::error::TypeError("Illegal constructor".into()))
    }

    /// <https://webidl.spec.whatwg.org/#es-iterator-prototype-next>
    #[method]
    fn next<'a>(&self, scope: &'a Scope<'a>) -> Result<js::Object<'a>, ExnThrown> {
        let result = js::Object::new(scope, None)?;
        let index = self.data().index;
        let kind = self.data().kind;
        let params: URLSearchParams = self.data().params.get(scope);

        if let Some((name, value)) = params.data().list.get(index).cloned() {
            let js_value = match kind {
                IteratorKind::Entries => {
                    let arr = js::Array::new(scope, 2)?;
                    let name_val = name.as_str().to_jsval(scope).map_err(|e| e.throw(scope))?;
                    let val_val = value.as_str().to_jsval(scope).map_err(|e| e.throw(scope))?;
                    arr.set_element(scope, 0, name_val)?;
                    arr.set_element(scope, 1, val_val)?;
                    scope.root_value(arr.as_value())
                }
                IteratorKind::Keys => name.as_str().to_jsval(scope).map_err(|e| e.throw(scope))?,
                IteratorKind::Values => {
                    value.as_str().to_jsval(scope).map_err(|e| e.throw(scope))?
                }
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
}

/// Define `Symbol.iterator` on the URLSearchParamsIterator prototype
/// (returns `this`, per the iterable protocol).  Called after class
/// registration.
pub fn install_symbol_iterator(scope: &Scope<'_>) {
    let proto = unsafe {
        js::Object::from_raw(
            scope,
            get_prototype_for::<URLSearchParamsIteratorImpl>(scope)
                .expect("URLSearchParamsIterator class not registered"),
        )
        .expect("URLSearchParamsIterator prototype is null")
    };

    // Symbol.iterator on an iterator returns `this`.
    let return_this = js::Function::new_callback(
        scope,
        c"[Symbol.iterator]",
        0,
        |_scope, args, _payload| Ok(args.this()),
        js::value::undefined(),
    )
    .expect("failed to create Symbol.iterator function");

    let iter_key = js::symbol::get_well_known_key(scope, js::native::SymbolCode::iterator);
    let iter_id = scope.root_id(iter_key);
    let fn_val = scope.root_value(return_this.as_value());
    proto
        .set_property_by_id(scope, iter_id, fn_val)
        .expect("failed to define Symbol.iterator on iterator prototype");
}
