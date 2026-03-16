// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Symbol creation and well-known symbol access.
//!
//! ES2015 Symbols are unique, immutable property keys. This module provides
//! access to creating new symbols, looking up the global symbol registry, and
//! retrieving well-known symbols like `Symbol.iterator`.

use std::ptr::NonNull;

use crate::gc::scope::Scope;
use mozjs::gc::{Handle, HandleString, HandleSymbol};
use mozjs::jsapi::{JSString, PropertyKey, Symbol, SymbolCode};
use mozjs::rust::wrappers2;

use super::error::ExnThrown;

/// Create a new unique symbol with the given description string.
pub fn new_symbol<'s>(
    scope: &'s Scope<'_>,
    description: HandleString,
) -> Result<Handle<'s, *mut Symbol>, ExnThrown> {
    let sym = unsafe { wrappers2::NewSymbol(scope.cx_mut(), description) };
    NonNull::new(sym)
        .map(|p| scope.root_symbol(p))
        .ok_or(ExnThrown)
}

/// Look up a symbol in the global symbol registry (`Symbol.for(key)`).
///
/// If a symbol with the given key already exists, it is returned; otherwise a
/// new one is created and registered.
pub fn get_symbol_for<'s>(
    scope: &'s Scope<'_>,
    key: HandleString,
) -> Result<Handle<'s, *mut Symbol>, ExnThrown> {
    let sym = unsafe { wrappers2::GetSymbolFor(scope.cx_mut(), key) };
    NonNull::new(sym)
        .map(|p| scope.root_symbol(p))
        .ok_or(ExnThrown)
}

/// Get the description string of a symbol, if it has one.
pub fn get_description(symbol: HandleSymbol) -> Option<NonNull<JSString>> {
    NonNull::new(unsafe { wrappers2::GetSymbolDescription(symbol) })
}

/// Get the `SymbolCode` of a symbol (identifies well-known symbols).
pub fn get_code(symbol: HandleSymbol) -> SymbolCode {
    unsafe { wrappers2::GetSymbolCode(symbol) }
}

/// Get a well-known symbol by its `SymbolCode`.
///
/// Well-known symbols include `Symbol.iterator`, `Symbol.toPrimitive`,
/// `Symbol.hasInstance`, etc.
///
/// Well-known symbols are always present in a valid runtime, so this
/// roots the result in the scope.
pub fn get_well_known<'s>(scope: &'s Scope<'_>, which: SymbolCode) -> Handle<'s, *mut Symbol> {
    let ptr = unsafe { wrappers2::GetWellKnownSymbol(scope.cx(), which) };
    // SAFETY: Well-known symbols are always present in a valid runtime.
    let nn = unsafe { NonNull::new_unchecked(ptr) };
    scope.root_symbol(nn)
}

/// Get the `PropertyKey` for a well-known symbol.
///
/// This is useful for property lookups using symbol keys.
pub fn get_well_known_key(scope: &Scope<'_>, which: SymbolCode) -> PropertyKey {
    unsafe { wrappers2::GetWellKnownSymbolKey(scope.cx(), which) }
}
