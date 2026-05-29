// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

pub mod algorithms;
pub mod url;
pub mod url_search_params;
pub mod url_search_params_iterator;

use js::gc::scope::Scope;
use js::Object;

pub fn add_to_global(scope: &Scope<'_>, global: Object<'_>) {
    url::URL::add_to_global(scope, global);
    url_search_params::URLSearchParams::add_to_global(scope, global);
    url_search_params_iterator::URLSearchParamsIterator::add_to_global(scope, global);
    url_search_params::install_symbol_iterator(scope);
    url_search_params_iterator::install_symbol_iterator(scope);
}
