// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Iterator for URLSearchParams (entries, keys, values).
//!
//! Implements the WebIDL `iterable<USVString, USVString>` declaration for
//! URLSearchParams.  Creates iterator objects that follow the standard
//! iterator protocol (`next()` returning `{value, done}`).

use crate::url_search_params::{URLSearchParams, URLSearchParamsImpl};

use core_runtime::{webidl_interface, webidl_methods};
use js::class::{get_iterator_prototype, get_prototype_for};
use js::conversion::ToJSVal;
use js::error::ExnThrown;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::Object;

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
}

/// Chain the `URLSearchParamsIterator` prototype under `%IteratorPrototype%` to satisfy
/// the interface's `iterable<>` declaration.
pub fn install_symbol_iterator(scope: &Scope<'_>) {
    let proto = unsafe {
        Object::from_raw(
            scope,
            get_prototype_for::<URLSearchParamsIteratorImpl>(scope)
                .expect("URLSearchParamsIterator class not registered"),
        )
        .expect("URLSearchParamsIterator prototype is null")
    };

    if let Ok(iterator_proto) = get_iterator_prototype(scope) {
        let _ = proto.set_prototype(scope, iterator_proto.handle());
    }
}
