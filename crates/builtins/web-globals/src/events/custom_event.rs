// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! [`CustomEvent`](https://dom.spec.whatwg.org/#interface-customevent) interface.
//!
//! Extends Event with a `detail` property for application-specific data.

use core_runtime::{webidl_dictionary, webidl_interface, webidl_methods};
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::native::Value;
use js::prelude::HandleValue;
use js::value;

use crate::events::event::EventInit;

use super::event::{Event, EventImpl};

/// <https://dom.spec.whatwg.org/#interface-customevent>
#[webidl_interface(extends = Event)]
pub struct CustomEvent {
    parent: Event,
    /// <https://dom.spec.whatwg.org/#dom-customevent-detail>
    detail: Heap<Value>,
}

#[webidl_methods]
impl CustomEvent {
    /// <https://dom.spec.whatwg.org/#dom-customevent-customevent>
    #[constructor]
    fn new(event_type: String, event_init_dict: Option<CustomEventInit<'_>>) -> Self {
        let opts = event_init_dict.unwrap_or_default();
        let parent_init = EventInit {
            bubbles: opts.bubbles,
            cancelable: opts.cancelable,
            composed: opts.composed,
        };
        CustomEventImpl {
            parent: EventImpl::new(event_type, Some(parent_init)),
            detail: Heap::from(opts.detail.map_or(value::null(), |h| h.get())),
        }
    }

    /// <https://dom.spec.whatwg.org/#dom-customevent-detail>
    #[getter]
    fn detail<'r>(&self, scope: &'r Scope<'_>) -> HandleValue<'r> {
        self.data().detail.get(scope)
    }

    /// <https://dom.spec.whatwg.org/#dom-customevent-initcustomevent>
    #[method]
    fn init_custom_event(
        &self,
        event_type: String,
        bubbles: Option<bool>,
        cancelable: Option<bool>,
        detail: Option<HandleValue<'_>>,
    ) {
        // Step 1: If `this`'s `dispatch flag` is set, then return.
        if self.is_dispatching() {
            return;
        }
        // Step 2: `Initialize` `this` with _type_, _bubbles_, and _cancelable_.
        super::event::initialize_event(
            &mut self.data_mut().parent,
            event_type,
            bubbles.unwrap_or(false),
            cancelable.unwrap_or(false),
        );
        // Step 3: Set `this`'s ``detail`` attribute to _detail_.
        if let Some(detail_val) = detail {
            self.data_mut().detail.set(detail_val.get());
        } else {
            self.data_mut().detail.set(value::null());
        }
    }
}

/// <https://dom.spec.whatwg.org/#dictdef-customeventinit>
///
/// Includes inherited members from EventInit: bubbles, cancelable, composed.
#[derive(Default)]
#[webidl_dictionary]
pub struct CustomEventInit<'a> {
    #[webidl(default = false)]
    pub bubbles: bool,
    #[webidl(default = false)]
    pub cancelable: bool,
    #[webidl(default = false)]
    pub composed: bool,
    pub detail: Option<HandleValue<'a>>,
}
