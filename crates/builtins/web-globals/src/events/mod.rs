// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! DOM Event interfaces.
//!
//! Implements [`Event`], [`EventTarget`], and [`CustomEvent`] from the
//! [WHATWG DOM Living Standard](https://dom.spec.whatwg.org/).

pub mod algorithms;
pub mod custom_event;
pub mod event;
pub mod event_target;

pub use event::Event;
pub use event_target::EventTarget;

use js::gc::scope::Scope;
use js::Object;

pub fn add_to_global(scope: &Scope<'_>, global: Object<'_>) {
    event::Event::add_to_global(scope, global);
    custom_event::CustomEvent::add_to_global(scope, global);
    event_target::EventTarget::add_to_global(scope, global);
}
