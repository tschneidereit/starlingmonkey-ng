// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! [`CustomEvent`](https://dom.spec.whatwg.org/#interface-customevent) interface.
//!
//! Extends Event with a `detail` property for application-specific data.

use core_runtime::{webidl_dictionary, webidl_interface, webidl_methods};
use js::error::ExnThrown;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::native::{CallArgs, Value};
use js::prelude::HandleValue;
use js::value;

use crate::events::event::EventInit;

use super::event::{Event, EventImpl};

/// <https://dom.spec.whatwg.org/#interface-customevent>
#[webidl_interface(extends = Event)]
pub struct CustomEvent {
    parent: Event,
    /// <https://dom.spec.whatwg.org/#dom-customevent-detail>
    pub(crate) detail: Heap<Value>,
}

#[webidl_methods]
impl CustomEvent {
    /// <https://dom.spec.whatwg.org/#dom-customevent-customevent>
    #[constructor]
    fn new(event_type: String, event_init_dict: Option<CustomEventInit>) -> Self {
        let opts = event_init_dict.unwrap_or_default();
        let parent_init = EventInit {
            bubbles: opts.bubbles.unwrap_or(false),
            cancelable: opts.cancelable.unwrap_or(false),
            composed: opts.composed.unwrap_or(false),
        };
        CustomEventImpl {
            parent: Event::init(event_type, Some(parent_init)),
            detail: Heap::from(opts.detail.unwrap_or_else(value::null)),
        }
    }

    /// <https://dom.spec.whatwg.org/#dom-customevent-detail>
    #[getter]
    fn detail(&self, _scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        // SAFETY: the value is consumed immediately into rval; no allocation occurs in between.
        args.rval().set(unsafe { self.data().detail.as_raw() });
        Ok(())
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
        if self.data().parent.dispatch_flag {
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
pub struct CustomEventInit {
    pub bubbles: Option<bool>,
    pub cancelable: Option<bool>,
    pub composed: Option<bool>,
    pub detail: Option<Value>,
}
