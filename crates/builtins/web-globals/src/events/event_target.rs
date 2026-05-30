// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! [`EventTarget`](https://dom.spec.whatwg.org/#interface-eventtarget) interface.
//!
//! EventTarget is the base interface for all objects that can receive events
//! and have listeners attached. This includes standalone `EventTarget` objects,
//! and types that inherit from it (e.g., AbortSignal).

use core_runtime::webidl_interface;
use core_runtime::webidl_methods;
use js::error::ExnThrown;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::prelude::HandleValue;
use js::Function;
use js::Object;

use super::algorithms;
use super::event::Event;
use crate::dom_exception::DOMExceptionError;

/// An event listener stored in an EventTarget's event listener list.
///
/// <https://dom.spec.whatwg.org/#concept-event-listener>
#[js::allow_unrooted_interior]
pub(crate) struct EventListener {
    /// Identity used to re-locate this listener in the live list during
    /// dispatch (the snapshot stores ids, not pointers — see `algorithms.rs`).
    pub(crate) id: u64,
    pub(crate) event_type: String,
    /// The GC-traced callback function. Listener identity is compared by
    /// reading this Heap's live pointer ([`Heap::as_ptr`]), so it stays
    /// correct across a compacting GC.
    pub(crate) callback: Heap<js::function::Function>,
    pub(crate) capture: bool,
    pub(crate) passive: Option<bool>,
    pub(crate) once: bool,
    pub(crate) removed: bool,
}

// Safety: the only GC pointer is the `callback` Heap; all other fields are
// primitive.
unsafe impl js::heap::Trace for EventListener {
    unsafe fn trace(&self, trc: *mut js::native::JSTracer) {
        self.callback.trace(trc);
    }
}

/// <https://dom.spec.whatwg.org/#interface-eventtarget>
#[webidl_interface]
pub struct EventTarget {
    /// <https://dom.spec.whatwg.org/#eventtarget-event-listener-list>
    pub(crate) event_listener_list: Vec<EventListener>,
}

#[webidl_methods]
impl EventTarget {
    /// <https://dom.spec.whatwg.org/#dom-eventtarget-eventtarget>
    #[constructor]
    fn new() -> Self {
        // Step 1: Do nothing.
        EventTargetImpl {
            event_listener_list: Vec::new(),
        }
    }

    /// <https://dom.spec.whatwg.org/#dom-eventtarget-addeventlistener>
    #[method]
    fn add_event_listener(
        &self,
        scope: &Scope<'_>,
        event_type: String,
        callback: HandleValue<'_>,
        options: Option<HandleValue<'_>>,
    ) -> Result<(), ExnThrown> {
        // Step 1: Let _capture_, _passive_, _once_, and _signal_ be the result of `flattening more`
        //         _options_. This converts the `signal` member and may throw a TypeError, which
        //         must happen even when `callback` is null.
        let options = algorithms::flatten_more_options(scope, options)?;

        if callback.is_null() || callback.is_undefined() {
            return Ok(());
        }
        let Ok(obj) = Object::from_value(scope, *callback) else {
            return Ok(());
        };
        let Ok(callback) = obj.cast::<Function<'_>>() else {
            return Ok(());
        };

        // Step 2: `Add an event listener` with `this` and an `event listener` whose `type` is
        //         _type_, `callback` is _callback_, `capture` is _capture_, `passive` is _passive_,
        //         `once` is _once_, and `signal` is _signal_.
        algorithms::add_an_event_listener(
            self,
            event_type,
            callback,
            options.capture,
            options.passive,
            options.once,
            options.signal,
        );
        Ok(())
    }

    /// <https://dom.spec.whatwg.org/#dom-eventtarget-removeeventlistener>
    #[method]
    fn remove_event_listener(
        &self,
        scope: &Scope<'_>,
        event_type: String,
        callback: HandleValue<'_>,
        options: Option<HandleValue<'_>>,
    ) {
        // Step 1: Let _capture_ be the result of `flattening` _options_.
        let capture = algorithms::flatten_options(scope, options);

        if callback.is_null() || callback.is_undefined() {
            return;
        }
        let Ok(obj) = Object::from_value(scope, *callback) else {
            return;
        };
        let Ok(callback) = obj.cast::<Function<'_>>() else {
            return;
        };

        // Step 2: If `this`'s `event listener list` `contains` an `event listener` whose `type`
        //         is _type_, `callback` is _callback_, and `capture` is _capture_, then `remove an
        //         event listener` with `this` and that `event listener`.
        algorithms::remove_an_event_listener(self.data_mut(), &event_type, &callback, capture);
    }

    /// <https://dom.spec.whatwg.org/#dom-eventtarget-dispatchevent>
    #[method]
    fn dispatch_event(
        &self,
        scope: &Scope<'_>,
        event: Event<'_>,
    ) -> Result<bool, DOMExceptionError> {
        // Step 1: If _event_'s `dispatch flag` is set, or if its `initialized flag` is not set,
        //         then `throw` an "``InvalidStateError``" ``DOMException``.
        let event_data = event.data();
        if event_data.dispatch_flag || !event_data.initialized_flag {
            return Err(DOMExceptionError::new(
                "InvalidStateError",
                "The event is already being dispatched or has not been initialized",
            ));
        }
        let _ = event_data;

        // Step 2: Initialize _event_'s ``isTrusted`` attribute to false.
        event.data_mut().is_trusted = false;

        // Step 3: Return the result of `dispatching` _event_ to `this`.
        Ok(algorithms::dispatch(scope, &event, self))
    }
}
