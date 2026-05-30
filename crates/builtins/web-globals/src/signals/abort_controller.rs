// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! [`AbortController`](https://dom.spec.whatwg.org/#interface-abortcontroller) interface.
//!
//! Provides an `AbortController` object that can be used to abort one or more
//! web requests.

use core_runtime::{webidl_interface, webidl_methods};
use js::error::ExnThrown;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::prelude::HandleValue;

use super::abort_signal::{AbortSignal, AbortSignalImpl};
use super::algorithms;

/// <https://dom.spec.whatwg.org/#interface-abortcontroller>
#[webidl_interface]
pub struct AbortController {
    /// <https://dom.spec.whatwg.org/#abortcontroller-signal>
    pub(crate) signal: Heap<AbortSignalImpl>,
}

#[webidl_methods]
impl AbortController {
    /// <https://dom.spec.whatwg.org/#dom-abortcontroller-abortcontroller>
    #[constructor]
    fn new(&self, scope: &Scope<'_>) -> Result<(), ExnThrown> {
        // Step 1: Let _signal_ be a new ``AbortSignal`` object.
        let signal = AbortSignal::new(scope)?;
        // Step 2: Set `this`'s `signal` to _signal_.
        self.data_mut().signal = Heap::from(signal);
        Ok(())
    }

    /// <https://dom.spec.whatwg.org/#dom-abortcontroller-signal>
    #[getter]
    fn signal<'r>(&self, scope: &'r Scope<'_>) -> AbortSignal<'r> {
        // Step 1: Return this's signal.
        self.data().signal.get(scope)
    }

    /// <https://dom.spec.whatwg.org/#dom-abortcontroller-abort>
    #[method]
    fn abort(&self, scope: &Scope<'_>, reason: Option<HandleValue<'_>>) -> Result<(), ExnThrown> {
        // Step 1: Signal abort on this with reason if it is given.
        let signal: AbortSignal<'_> = self.data().signal.get(scope);
        algorithms::signal_abort(scope, &signal, reason)
    }
}
