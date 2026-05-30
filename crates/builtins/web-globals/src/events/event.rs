// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! [`Event`](https://dom.spec.whatwg.org/#interface-event) interface.
//!
//! Implements the core DOM Event type following the WHATWG DOM Living Standard.

use std::time::Instant;

use core_runtime::{webidl_dictionary, webidl_interface, webidl_methods};
use js::gc::handle::Heap;
use js::gc::scope::Scope;

use super::event_target::EventTargetImpl;

/// <https://dom.spec.whatwg.org/#interface-event>
// TODO: convert all these bools into bitflags.
#[webidl_interface]
pub struct Event {
    /// The event type (e.g. "click", "abort").
    #[no_trace]
    pub(crate) event_type: String,
    /// <https://dom.spec.whatwg.org/#dom-event-target>
    pub(crate) target: Option<Heap<EventTargetImpl>>,
    /// <https://dom.spec.whatwg.org/#dom-event-currenttarget>
    pub(crate) current_target: Option<Heap<EventTargetImpl>>,
    /// <https://dom.spec.whatwg.org/#dom-event-eventphase>
    #[no_trace]
    pub(crate) event_phase: u16,
    /// <https://dom.spec.whatwg.org/#dom-event-bubbles>
    #[no_trace]
    pub(crate) bubbles: bool,
    /// <https://dom.spec.whatwg.org/#dom-event-cancelable>
    #[no_trace]
    pub(crate) cancelable: bool,
    /// <https://dom.spec.whatwg.org/#dom-event-composed>
    #[no_trace]
    pub(crate) composed: bool,
    /// <https://dom.spec.whatwg.org/#dom-event-istrusted>
    #[no_trace]
    pub(crate) is_trusted: bool,
    /// <https://dom.spec.whatwg.org/#dom-event-timestamp>
    #[no_trace]
    pub(crate) time_stamp: f64,

    // Internal flags per spec.
    #[no_trace]
    pub(crate) stop_propagation_flag: bool,
    #[no_trace]
    pub(crate) stop_immediate_propagation_flag: bool,
    #[no_trace]
    pub(crate) canceled_flag: bool,
    #[no_trace]
    pub(crate) in_passive_listener_flag: bool,
    #[no_trace]
    pub(crate) dispatch_flag: bool,
    #[no_trace]
    pub(crate) initialized_flag: bool,
}

/// Monotonic reference point for `timeStamp` computation.
///
/// Per the spec, `timeStamp` returns the number of milliseconds since
/// "time origin" — we approximate with process-relative monotonic time.
fn time_origin() -> &'static Instant {
    use std::sync::OnceLock;
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now)
}

/// Returns the current high-resolution timestamp in milliseconds relative to
/// the time origin.
pub(crate) fn current_high_res_time() -> f64 {
    let elapsed = time_origin().elapsed();
    elapsed.as_secs_f64() * 1000.0
}

// Event phase constants — accessible from algorithms and other modules.
// TODO: Instead of these, `Event::CONSTANT` should be used, but the WebIDL macro installs them on `EventImpl`.
pub(crate) const NONE: u16 = 0;
pub(crate) const CAPTURING_PHASE: u16 = 1;
pub(crate) const AT_TARGET: u16 = 2;
pub(crate) const BUBBLING_PHASE: u16 = 3;

#[webidl_methods]
impl Event {
    pub const NONE: u16 = NONE;
    pub const CAPTURING_PHASE: u16 = CAPTURING_PHASE;
    pub const AT_TARGET: u16 = AT_TARGET;
    pub const BUBBLING_PHASE: u16 = BUBBLING_PHASE;

    /// <https://dom.spec.whatwg.org/#concept-event-constructor>
    #[constructor]
    fn new(event_type: String, event_init_dict: Option<EventInit>) -> Self {
        let opts = event_init_dict.unwrap_or_default();
        EventImpl {
            event_type,
            target: None,
            current_target: None,
            event_phase: Self::NONE,
            bubbles: opts.bubbles,
            cancelable: opts.cancelable,
            composed: opts.composed,
            is_trusted: false,
            time_stamp: current_high_res_time(),
            stop_propagation_flag: false,
            stop_immediate_propagation_flag: false,
            canceled_flag: false,
            in_passive_listener_flag: false,
            dispatch_flag: false,
            initialized_flag: true,
        }
    }

    /// <https://dom.spec.whatwg.org/#dom-event-type>
    #[getter(name = "type")]
    fn get_type(&self) -> String {
        self.data().event_type.clone()
    }

    /// <https://dom.spec.whatwg.org/#dom-event-target>
    #[getter]
    fn target<'r>(&self, scope: &'r Scope<'_>) -> Option<super::event_target::EventTarget<'r>> {
        // Step 1: Return this's target.
        self.data().target.as_ref().map(|h| h.get(scope))
    }

    /// <https://dom.spec.whatwg.org/#dom-event-srcelement>
    #[getter]
    fn src_element<'r>(
        &self,
        scope: &'r Scope<'_>,
    ) -> Option<super::event_target::EventTarget<'r>> {
        // Step 1: Return this's target.
        self.data().target.as_ref().map(|h| h.get(scope))
    }

    /// <https://dom.spec.whatwg.org/#dom-event-currenttarget>
    #[getter]
    fn current_target<'r>(
        &self,
        scope: &'r Scope<'_>,
    ) -> Option<super::event_target::EventTarget<'r>> {
        self.data().current_target.as_ref().map(|h| h.get(scope))
    }

    /// <https://dom.spec.whatwg.org/#dom-event-eventphase>
    #[getter]
    fn event_phase(&self) -> u16 {
        self.data().event_phase
    }

    /// <https://dom.spec.whatwg.org/#dom-event-stoppropagation>
    #[method]
    fn stop_propagation(&self) {
        // Step 1: Set this's stop propagation flag.
        self.data_mut().stop_propagation_flag = true;
    }

    /// <https://dom.spec.whatwg.org/#dom-event-cancelbubble>
    #[getter]
    fn cancel_bubble(&self) -> bool {
        // Step 1: Return true if this's stop propagation flag is set; otherwise false.
        self.data().stop_propagation_flag
    }

    /// <https://dom.spec.whatwg.org/#dom-event-cancelbubble>
    #[setter]
    fn set_cancel_bubble(&self, value: bool) {
        // Step 1: Set this's stop propagation flag if the given value is true; otherwise do
        //         nothing.
        if value {
            self.data_mut().stop_propagation_flag = true;
        }
    }

    /// <https://dom.spec.whatwg.org/#dom-event-stopimmediatepropagation>
    #[method]
    fn stop_immediate_propagation(&self) {
        // Step 1: Set this's stop propagation flag and this's stop immediate propagation flag.
        let data = self.data_mut();
        data.stop_propagation_flag = true;
        data.stop_immediate_propagation_flag = true;
    }

    /// <https://dom.spec.whatwg.org/#dom-event-bubbles>
    #[getter]
    fn bubbles(&self) -> bool {
        self.data().bubbles
    }

    /// <https://dom.spec.whatwg.org/#dom-event-cancelable>
    #[getter]
    fn cancelable(&self) -> bool {
        self.data().cancelable
    }

    /// <https://dom.spec.whatwg.org/#dom-event-returnvalue>
    #[getter]
    fn return_value(&self) -> bool {
        // Step 1: Return false if this's canceled flag is set; otherwise true.
        !self.data().canceled_flag
    }

    /// <https://dom.spec.whatwg.org/#dom-event-returnvalue>
    #[setter]
    fn set_return_value(&self, value: bool) {
        // Step 1: Set the canceled flag with this if the given value is false; otherwise do
        //         nothing.
        if !value {
            set_the_canceled_flag(self.data_mut());
        }
    }

    /// <https://dom.spec.whatwg.org/#dom-event-preventdefault>
    #[method]
    fn prevent_default(&self) {
        // Step 1: Set the canceled flag with this.
        set_the_canceled_flag(self.data_mut());
    }

    /// <https://dom.spec.whatwg.org/#dom-event-defaultprevented>
    #[getter]
    fn default_prevented(&self) -> bool {
        // Step 1: Return true if this's canceled flag is set; otherwise false.
        self.data().canceled_flag
    }

    /// <https://dom.spec.whatwg.org/#dom-event-composed>
    #[getter]
    fn composed(&self) -> bool {
        // Step 1: Return true if this's composed flag is set; otherwise false.
        self.data().composed
    }

    /// <https://dom.spec.whatwg.org/#dom-event-istrusted>
    ///
    /// `[LegacyUnforgeable]`: installed as an own, non-configurable accessor on
    /// each instance rather than on the prototype.
    #[getter(unforgeable)]
    fn is_trusted(&self) -> bool {
        self.data().is_trusted
    }

    /// <https://dom.spec.whatwg.org/#dom-event-timestamp>
    #[getter]
    fn time_stamp(&self) -> f64 {
        self.data().time_stamp
    }

    /// <https://dom.spec.whatwg.org/#dom-event-composedpath>
    ///
    /// Without shadow DOM support, this returns an empty array (no path entries
    /// to traverse).
    #[method]
    fn composed_path(&self) -> Vec<()> {
        // Step 1: Let _composedPath_ be an empty `list`.
        // Step 2: Let _path_ be `this`'s `path`.
        // Step 3: If _path_ `is empty`, then return _composedPath_.
        // Steps 4-16 involve shadow DOM path traversal — not applicable.
        // Step 17: Return _composedPath_.
        Vec::new()
    }

    /// <https://dom.spec.whatwg.org/#dom-event-initevent>
    #[method]
    fn init_event(&self, event_type: String, bubbles: Option<bool>, cancelable: Option<bool>) {
        // Step 1: If `this`'s `dispatch flag` is set, then return.
        if self.data().dispatch_flag {
            return;
        }
        // Step 2: `Initialize` `this` with _type_, _bubbles_, and _cancelable_.
        initialize_event(
            self.data_mut(),
            event_type,
            bubbles.unwrap_or(false),
            cancelable.unwrap_or(false),
        );
    }
}

/// <https://dom.spec.whatwg.org/#set-the-canceled-flag>
///
/// To set the canceled flag, if event's cancelable attribute value is true
/// and event's in passive listener flag is unset, then set event's canceled
/// flag.
fn set_the_canceled_flag(data: &mut EventImpl) {
    if data.cancelable && !data.in_passive_listener_flag {
        data.canceled_flag = true;
    }
}

/// <https://dom.spec.whatwg.org/#concept-event-initialize>
///
/// To initialize an event, with type, bubbles, and cancelable, run these steps:
pub(crate) fn initialize_event(
    data: &mut EventImpl,
    event_type: String,
    bubbles: bool,
    cancelable: bool,
) {
    // Step 1: Set _event_'s `initialized flag`.
    data.initialized_flag = true;
    // Step 2: Unset _event_'s `stop propagation flag`, `stop immediate propagation flag`, and
    //         `canceled flag`.
    data.stop_propagation_flag = false;
    data.stop_immediate_propagation_flag = false;
    data.canceled_flag = false;
    // Step 3: Set _event_'s ``isTrusted`` attribute to false.
    data.is_trusted = false;
    // Step 4: Set _event_'s `target` to null.
    data.target = None;
    // Step 5: Set _event_'s ``type`` attribute to _type_.
    data.event_type = event_type;
    // Step 6: Set _event_'s ``bubbles`` attribute to _bubbles_.
    data.bubbles = bubbles;
    // Step 7: Set _event_'s ``cancelable`` attribute to _cancelable_.
    data.cancelable = cancelable;
}

/// <https://dom.spec.whatwg.org/#dictdef-eventinit>
#[derive(Default)]
#[webidl_dictionary]
pub struct EventInit {
    #[webidl(default = false)]
    pub bubbles: bool,
    #[webidl(default = false)]
    pub cancelable: bool,
    #[webidl(default = false)]
    pub composed: bool,
}
