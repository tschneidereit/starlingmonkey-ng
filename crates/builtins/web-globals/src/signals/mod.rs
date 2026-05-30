// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! DOM Abort Signal interfaces.
//!
//! Implements [`AbortController`] and [`AbortSignal`] from the
//! [WHATWG DOM Living Standard](https://dom.spec.whatwg.org/).

pub mod abort_controller;
pub mod abort_signal;
pub mod algorithms;

pub use abort_signal::{AbortSignal, AbortSignalImpl};

use js::gc::scope::Scope;
use js::Object;

pub fn add_to_global(scope: &Scope<'_>, global: Object<'_>) {
    abort_controller::AbortController::add_to_global(scope, global);
    abort_signal::AbortSignal::add_to_global(scope, global);
}
