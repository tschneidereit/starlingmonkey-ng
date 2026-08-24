// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Implementation of the `FetchEvent` and `ExtendableEvent` interfaces from the
//! [Service Workers](https://w3c.github.io/ServiceWorker/) specification.

pub mod extendable_event;
pub mod fetch_event;

use js::gc::scope::Scope;
use js::Object;

/// Register `ExtendableEvent` and `FetchEvent`.
pub fn add_to_global(scope: &Scope, global: Object) {
    extendable_event::add_to_global(scope, global);
    fetch_event::FetchEvent::add_to_global(scope, global);
}
