// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Iterator for URLSearchParams (entries, keys, values).
//!
//! Implements the WebIDL `iterable<USVString, USVString>` declaration for
//! URLSearchParams.  Creates iterator objects that follow the standard
//! iterator protocol (`next()` returning `{value, done}`).

use crate::url_search_params::{URLSearchParams, URLSearchParamsImpl};

use core_runtime::{webidl_interface, webidl_methods};
use js::error::ExnThrown;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::iteration::{create_iter_result, pair_iterator_result, IterationKind};
use js::prelude::HandleValue;

/// <https://webidl.spec.whatwg.org/#dfn-default-iterator-object>
#[webidl_interface(hidden, name = "URLSearchParams Iterator")]
pub struct URLSearchParamsIterator {
    /// Reference to the URLSearchParams being iterated.
    pub(crate) params: Heap<URLSearchParamsImpl>,
    /// Current position in the list.
    pub(crate) index: usize,
    #[no_trace]
    pub(crate) kind: IterationKind,
}

#[webidl_methods]
impl URLSearchParamsIterator {
    fn new(params: URLSearchParams, kind: IterationKind) -> Self {
        Self {
            params: Heap::from(params),
            index: 0,
            kind,
        }
    }

    /// <https://webidl.spec.whatwg.org/#es-iterator-prototype-next>
    #[method]
    fn next<'a>(&self, scope: &'a Scope<'a>) -> Result<js::Object<'a>, ExnThrown> {
        // Steps 1-5: Implemented in the `#[method]` receiver check.
        // Step 6: Let _index_ be _object_'s `index`.
        let index = self.data().index;
        // Step 7: Let _kind_ be _object_'s `kind`.
        let kind = self.data().kind;
        // Step 8: Let _values_ be _object_'s `target`'s `value pairs to iterate over`.
        let params = self.data().params.get(scope);
        let params_data = params.data();
        let values = &params_data.list;
        // Step 9: Let _len_ be the length of _values_.
        // Step 10: If _index_ is greater than or equal to _len_, then return
        //          CreateIteratorResultObject(undefined, true).
        // Step 11: Let _pair_ be the entry in _values_ at index _index_.
        let Some((name, value)) = values.get(index) else {
            return create_iter_result(scope, HandleValue::undefined(), true);
        };
        // Step 12: Set _object_'s index to _index_ + 1.
        self.data_mut().index = index + 1;
        // Step 13: Return the `iterator result` for _pair_ and _kind_.
        pair_iterator_result(scope, name.as_str(), value.as_str(), kind)
    }
}
