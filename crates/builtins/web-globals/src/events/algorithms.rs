// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Event-related algorithms from <https://dom.spec.whatwg.org>.
//!
//! Contains the listener management and event dispatch algorithms used by
//! `EventTarget` and `Event`.

use std::cell::Cell;

use js::conversion::{ConversionError, FromJSVal};
use js::error::{throw_type_error, ExnThrown};
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::prelude::HandleValue;
use js::{Function, Object};

use super::event::Event;
use super::event_target::{EventListener, EventTarget, EventTargetImpl};
use crate::events::event::EventFlags;
use crate::signals::abort_signal::AbortSignal;
use crate::signals::algorithms as signal_algorithms;

/// The flattened result of `addEventListener`'s options argument.
///
/// <https://dom.spec.whatwg.org/#event-flatten-more>
///
/// Note: while this *looks* like it should be a WebIDL dictionary, the coercion rules are
/// slightly different, which is why *flatten-more* is a thing in the spec and this implementation.
pub(crate) struct ListenerOptions<'s> {
    pub(crate) capture: bool,
    pub(crate) passive: Option<bool>,
    pub(crate) once: bool,
    pub(crate) signal: Option<AbortSignal<'s>>,
}

thread_local! {
    /// Monotonic source of event-listener ids.
    /// Would theoretically overflow after 2^64 listeners, but that's not a realistic concern.
    static NEXT_LISTENER_ID: Cell<u64> = const { Cell::new(0) };
}

fn next_listener_id() -> u64 {
    NEXT_LISTENER_ID.with(|c| {
        let id = c.get();
        c.set(id + 1);
        id
    })
}

/// <https://dom.spec.whatwg.org/#concept-flatten-options>
///
/// (Note: while the spec presents this as a WebIDL spec concept, it's entirely
/// specific to event handling, so this file is indeed the right place for it.)
///
/// To flatten options, run these steps:
pub(crate) fn flatten_options(scope: &Scope<'_>, options: Option<HandleValue<'_>>) -> bool {
    match options {
        None => false,
        Some(val) => {
            // Step 1: If _options_ is a boolean, then return _options_.
            if val.is_boolean() {
                val.to_boolean()
            } else if val.is_object() {
                // Step 2: Return _options_["``capture``"].
                let obj = js::Object::from_value(scope, *val).expect("options should be an object");
                if let Ok(capture_val) = obj.get_property(scope, c"capture") {
                    bool::from_jsval(scope, capture_val, ()).unwrap()
                } else {
                    false
                }
            } else {
                false
            }
        }
    }
}

/// <https://dom.spec.whatwg.org/#event-flatten-more>
///
/// To flatten more options, run these steps.
///
/// Throws a `TypeError` if `options["signal"]` is present but not an
/// `AbortSignal` (which includes `null`), matching the WebIDL conversion of the
/// non-nullable `AbortSignal signal` dictionary member.
pub(crate) fn flatten_more_options<'s>(
    scope: &'s Scope<'_>,
    options: Option<HandleValue<'_>>,
) -> Result<ListenerOptions<'s>, ExnThrown> {
    match options {
        None => Ok(ListenerOptions {
            capture: false,
            passive: None,
            once: false,
            signal: None,
        }),
        Some(val) => {
            if val.is_boolean() {
                // Step 1: Let _capture_ be the result of `flattening` _options_.
                Ok(ListenerOptions {
                    capture: val.to_boolean(),
                    passive: None,
                    once: false,
                    signal: None,
                })
            } else if val.is_object() {
                let obj = Object::from_value(scope, *val).unwrap();

                // Step 1: Let _capture_ be the result of `flattening` _options_.
                let capture = obj
                    .get_property(scope, c"capture")
                    .map(|v| bool::from_jsval(scope, v, ()).unwrap())
                    .unwrap_or(false);

                // Step 2: Let _once_ be false.
                // Step 4: If _options_ is a `dictionary`: Set _once_ to _options_["``once``"].
                let once = obj
                    .get_property(scope, c"once")
                    .map(|v| bool::from_jsval(scope, v, ()).unwrap())
                    .unwrap_or(false);

                // Step 3: Let _passive_ and _signal_ be null.
                // Step 4 (cont.): If _options_["``passive``"] `exists`, then set _passive_ to
                //                 _options_["``passive``"].
                let passive = obj.get_property(scope, c"passive").ok().and_then(|v| {
                    if v.is_undefined() {
                        None
                    } else {
                        Some(bool::from_jsval(scope, v, ()).unwrap())
                    }
                });

                // Step 4 (cont.): If _options_["``signal``"] `exists`, then set _signal_ to
                //                 _options_["``signal``"].
                let signal = read_signal_option(scope, &obj)?;

                // Step 5: Return _capture_, _passive_, _once_, and _signal_.
                Ok(ListenerOptions {
                    capture,
                    passive,
                    once,
                    signal,
                })
            } else {
                Ok(ListenerOptions {
                    capture: false,
                    passive: None,
                    once: false,
                    signal: None,
                })
            }
        }
    }
}

/// Read the `signal` member of an `addEventListener` options dictionary.
///
/// An absent or `undefined` member yields `None`; any present value is
/// converted to `AbortSignal`, throwing a `TypeError` for `null` or any other
/// non-`AbortSignal` value.
fn read_signal_option<'s>(
    scope: &'s Scope<'_>,
    options: &Object<'_>,
) -> Result<Option<AbortSignal<'s>>, ExnThrown> {
    let val = options.get_property(scope, c"signal")?;
    if val.is_undefined() {
        return Ok(None);
    }
    match AbortSignal::from_jsval(scope, val, ()) {
        Ok(signal) => Ok(Some(signal)),
        Err(ConversionError::Failure(_)) => {
            Err(throw_type_error(scope, c"signal must be an AbortSignal"))
        }
        Err(ConversionError::ExnPending) => {
            // Propagate any exception thrown by the conversion (e.g. from a proxy trap).
            Err(ExnThrown)
        }
    }
}

/// <https://dom.spec.whatwg.org/#add-an-event-listener>
///
/// To add an event listener, given an EventTarget object eventTarget and an
/// event listener listener, run these steps:
pub(crate) fn add_an_event_listener(
    target: &EventTarget<'_>,
    event_type: String,
    callback: Function<'_>,
    capture: bool,
    passive: Option<bool>,
    once: bool,
    signal: Option<AbortSignal<'_>>,
) {
    // Step 1: (ServiceWorkerGlobalScope warning — not applicable.)

    // Step 2: If _listener_'s `signal` is non-null and is `aborted`, then return.
    if let Some(signal) = &signal {
        if signal.aborted() {
            return;
        }
    }

    // Step 3: If _listener_'s `callback` is null, then return.
    // (Handled by the caller — only called with a valid callback.)
    // Step 4: If _listener_'s `passive` is null, then set it to the `default passive value`
    //         given _listener_'s `type` and _eventTarget_.
    // (We pass passive through as-is; no default passive value logic yet.)

    // Step 5: If _eventTarget_'s `event listener list` does not `contain` an `event listener`
    //         whose `type` is _listener_'s `type`, `callback` is _listener_'s `callback`, and
    //         `capture` is _listener_'s `capture`, then `append` _listener_ to _eventTarget_'s
    //         `event listener list`.
    let existing_id = target
        .data()
        .event_listener_list
        .iter()
        .find(|l| {
            !l.removed
                && l.event_type == event_type
                && l.capture == capture
                // TODO: implement Eq for Heap and Stack and remove the use of raw pointers.
                && l.callback.eq_stack(&callback)
        })
        .map(|l| l.id);
    let listener_id = match existing_id {
        Some(id) => id,
        None => {
            let id = next_listener_id();
            target.data_mut().event_listener_list.push(EventListener {
                id,
                event_type,
                callback: Heap::from(callback),
                capture,
                passive,
                once,
                removed: false,
            });
            id
        }
    };

    // Step 6: If _listener_'s `signal` is non-null, then `add the following` abort steps to it:
    //         `Remove an event listener` with _eventTarget_ and _listener_.
    if let Some(signal) = signal {
        signal_algorithms::add_listener_removal_algorithm(&signal, target, listener_id);
    }
}

/// <https://dom.spec.whatwg.org/#remove-an-event-listener>
///
/// To remove an event listener, given an EventTarget object eventTarget and an
/// event listener listener, run these steps:
pub(crate) fn remove_an_event_listener(
    target: &mut EventTargetImpl,
    event_type: &str,
    callback: &Function<'_>,
    capture: bool,
) {
    // Step 1: (ServiceWorkerGlobalScope warning — not applicable.)

    // Step 2: Set _listener_'s `removed` to true and `remove` _listener_ from _eventTarget_'s
    //         `event listener list`. Identity is compared via the stored Heap's
    //         live pointer (see `add_an_event_listener`).
    target.event_listener_list.retain(|l| {
        !(l.event_type == event_type && l.capture == capture && l.callback.eq_stack(callback))
    });
}

/// Remove the event listener with the given id from an EventTarget.
///
/// This is the concrete action of the abort algorithm registered by
/// "add an event listener" step 6: it removes a specific listener identified by
/// its id (never reused), regardless of its type, callback, or capture.
pub(crate) fn remove_an_event_listener_by_id(target: &EventTarget<'_>, id: u64) {
    target.data_mut().event_listener_list.retain(|l| l.id != id);
}

/// <https://dom.spec.whatwg.org/#concept-event-dispatch>
///
/// To dispatch an event to a target, with an optional legacy target override
/// flag and an optional legacyOutputDidListenersThrowFlag, run these steps:
///
/// This is a simplified implementation handling only AT_TARGET dispatch
/// (no shadow DOM, no capturing/bubbling phase, no activation behavior).
pub(crate) fn dispatch(scope: &Scope<'_>, event: &Event<'_>, target: &EventTarget<'_>) -> bool {
    // Step 1: Set _event_'s `dispatch flag`.
    event.start_dispatching();

    // Step 2-5: (Legacy target override, activation target, retargeting — not applicable.)

    // Step 6: Simplified — set target and invoke at AT_TARGET only.
    event.data_mut().target = Some(Heap::from(*target));

    invoke_listeners(scope, event, target);

    // Step 7: Set _event_'s ``eventPhase`` attribute to ``NONE``.
    // (Omitted)

    // Step 8: Set _event_'s ``currentTarget`` attribute to null.
    // (Omitted)

    // Step 9: Set _event_'s `path` to the empty list.
    // (Not applicable without shadow DOM.)

    // Step 10: Unset _event_'s `dispatch flag`, `stop propagation flag`, and `stop immediate
    //          propagation flag`.
    event.stop_dispatching();

    // Step 11: (clearTargets — not applicable without shadow DOM.)
    // Step 12: (activation behavior — not applicable.)

    // Step 13: Return false if _event_'s `canceled flag` is set; otherwise true.
    !event.is_canceled()
}

/// Invoke event listeners on a single target.
///
/// Based on the "inner invoke" algorithm:
/// <https://dom.spec.whatwg.org/#concept-event-listener-inner-invoke>
fn invoke_listeners(scope: &Scope<'_>, event: &Event<'_>, target: &EventTarget<'_>) {
    if event.is_propagation_stopped() {
        return;
    }

    // Snapshot the listener list to avoid issues with listeners added or
    // removed during dispatch. We capture only listener `id`s and metadata; the
    // live list is re-consulted by `id` each iteration, so callbacks are always
    // fetched fresh and rooted (no stored GC pointers, no rooting gap).
    struct ListenerSnapshot {
        id: u64,
        passive: Option<bool>,
        once: bool,
        event_type: String,
    }

    let snapshots: Vec<ListenerSnapshot> = target
        .data()
        .event_listener_list
        .iter()
        .filter(|l| !l.removed)
        .map(|l| ListenerSnapshot {
            id: l.id,
            passive: l.passive,
            once: l.once,
            event_type: l.event_type.clone(),
        })
        .collect();

    let event_type = event.data().event_type.clone();

    for snap in &snapshots {
        if snap.event_type != event_type {
            continue;
        }

        // Re-locate the listener in the live list; it may have been removed by
        // a previous callback.
        let Some(pos) = target
            .data()
            .event_listener_list
            .iter()
            .position(|l| l.id == snap.id && !l.removed)
        else {
            continue;
        };

        // If listener's `once` is true, remove it.
        if snap.once {
            target.data_mut().event_listener_list[pos].removed = true;
        }

        // Set in-passive-listener flag if passive.
        if snap.passive == Some(true) {
            event.set_in_passive_listener(true);
        }

        // Call the listener. We root the callback from the Heap at call time.
        // The `target.data()` borrow is released before the call so a listener
        // that mutates the list (e.g. by aborting a signal that removes
        // listeners) does not hit a `RefCell` double borrow.
        {
            let callback: Function<'_> = {
                let listener = &target.data().event_listener_list[pos];
                listener.callback.get(scope)
            };
            let callback_val = scope.root_value(callback.as_value());
            let this_val = scope.root_value(target.as_value());
            let event_val = scope.root_value(event.as_value());

            if Function::call(scope, this_val, callback_val, &[event_val]).is_err() {
                // Per the spec, exceptions from event listeners are "reported"
                // but do not stop event dispatch.
                js::exception::report_and_clear(scope, "event listener");
            }
        }

        event.set_in_passive_listener(false);

        if event.is_immediate_propagation_stopped() {
            break;
        }
    }
}

/// <https://dom.spec.whatwg.org/#concept-event-fire>
///
/// To fire an event named e at target, optionally using an eventConstructor,
/// with a description of how IDL attributes are to be initialized, and a
/// legacy target override flag, run these steps:
pub(crate) fn fire_an_event(
    scope: &Scope<'_>,
    name: &str,
    target: &EventTarget<'_>,
) -> Result<bool, ExnThrown> {
    // Step 1: If _eventConstructor_ is not given, then let _eventConstructor_ be ``Event``.
    // Step 2: Let _event_ be the result of `creating an event` given _eventConstructor_, in the
    //         `relevant realm` of _target_.
    let event = Event::new(scope, name.to_string(), None)?;

    // Step 3: Initialize _event_'s ``type`` attribute to _e_.
    // (Already set by the Event constructor above.)

    // Step 4: Initialize any other IDL attributes of _event_ as described in the invocation of
    //         this algorithm. This also allows for the ``isTrusted`` attribute to be set to false.
    event.data_mut().flags.insert(EventFlags::TRUSTED);

    // Step 5: Return the result of `dispatching` _event_ at _target_, with _legacy target override
    //         flag_ set if set.
    Ok(dispatch(scope, &event, target))
}
