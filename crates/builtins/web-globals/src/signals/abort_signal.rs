// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! [`AbortSignal`](https://dom.spec.whatwg.org/#interface-abortsignal) interface.
//!
//! An AbortSignal represents a signal that can communicate that an operation
//! should be aborted. It extends EventTarget so it can fire "abort" events.

use core_runtime::{webidl_interface, webidl_methods};
use js::error::ExnThrown;
use js::gc::handle::{Heap, OptionHeapExt};
use js::gc::scope::Scope;
use js::native::{CallArgs, ExceptionStackBehavior, Value};
use js::prelude::HandleValue;
use js::{value, Function, Object};

use super::algorithms;
use crate::events::algorithms as event_algorithms;
use crate::events::event_target::{EventTarget, EventTargetImpl};

/// An algorithm registered to run when the signal is aborted.
///
/// <https://dom.spec.whatwg.org/#abortsignal-abort-algorithms>
///
/// The only abort algorithm this runtime registers removes an event listener
/// added with the signal (per "add an event listener" step 6), so it is stored
/// concretely as the target plus the listener id rather than as a boxed
/// closure. A closure capturing a `Heap` could not be traced (it is opaque to
/// both the GC tracer and crown); keeping the GC pointer in an explicit field
/// makes it traceable.
///
#[js::must_root]
pub(crate) enum AbortAlgorithm {
    /// Remove the event listener identified by `listener_id` from `held` (an
    /// EventTarget) when the signal aborts. Registered by "add an event listener".
    RemoveListener {
        /// The EventTarget whose listener is removed. Traced so it stays alive
        /// until then.
        held: Heap<EventTargetImpl>,
        /// Identity of the listener to remove.
        listener_id: u64,
    },
    /// Call `callback` with the signal's abort reason when the signal aborts.
    /// Registered by callers such as `fetch` that need to react to abort.
    RunSteps(Heap<js::function::Function>),
}

// Safety: `trc` must be the JS engine's `JSTracer` function.
unsafe impl js::heap::Trace for AbortAlgorithm {
    unsafe fn trace(&self, trc: *mut js::native::JSTracer) {
        match self {
            AbortAlgorithm::RemoveListener { held, .. } => held.trace(trc),
            AbortAlgorithm::RunSteps(callback) => callback.trace(trc),
        }
    }
}

/// <https://dom.spec.whatwg.org/#interface-abortsignal>
#[webidl_interface(extends = EventTarget)]
pub struct AbortSignal {
    parent: EventTargetImpl,

    /// <https://dom.spec.whatwg.org/#abortsignal-abort-reason>
    pub(crate) abort_reason: Heap<Value>,

    /// <https://dom.spec.whatwg.org/#abortsignal-dependent>
    #[no_trace]
    pub(crate) dependent: bool,

    /// <https://dom.spec.whatwg.org/#abortsignal-source-signals>
    pub(crate) source_signals: Vec<Heap<AbortSignalImpl>>,

    /// <https://dom.spec.whatwg.org/#abortsignal-dependent-signals>
    pub(crate) dependent_signals: Vec<Heap<AbortSignalImpl>>,

    /// <https://dom.spec.whatwg.org/#abortsignal-abort-algorithms>
    pub(crate) abort_algorithms: Vec<AbortAlgorithm>,

    /// The `onabort` event handler attribute.
    ///
    /// <https://dom.spec.whatwg.org/#dom-abortsignal-onabort>
    pub(crate) onabort_handler: Option<Heap<js::function::Function>>,
}

#[webidl_methods]
impl AbortSignal {
    fn new() -> Self {
        AbortSignalImpl {
            parent: EventTargetImpl::default(),
            abort_reason: Heap::default(),
            dependent: false,
            source_signals: Vec::new(),
            dependent_signals: Vec::new(),
            abort_algorithms: Vec::new(),
            onabort_handler: None,
        }
    }

    /// <https://dom.spec.whatwg.org/#dom-abortsignal-aborted>
    #[getter]
    pub fn aborted(&self) -> bool {
        // Step 1: Return true if this is aborted; otherwise false.
        !self.data().abort_reason.is_undefined()
    }

    /// <https://dom.spec.whatwg.org/#dom-abortsignal-reason>
    #[getter]
    pub fn reason<'r>(&self, scope: &'r Scope<'_>) -> HandleValue<'r> {
        // Step 1: Return this's abort reason.
        self.data().abort_reason.get(scope)
    }

    /// <https://dom.spec.whatwg.org/#dom-abortsignal-throwifaborted>
    #[method]
    fn throw_if_aborted(&self, scope: &Scope<'_>) -> Result<(), ExnThrown> {
        // Step 1: Throw this's abort reason, if this is aborted.
        if !self.data().abort_reason.is_undefined() {
            let reason = self.data().abort_reason.get(scope);
            js::exception::set_pending(scope, reason, ExceptionStackBehavior::DoNotCapture);
            return Err(ExnThrown);
        }
        Ok(())
    }

    /// <https://dom.spec.whatwg.org/#dom-abortsignal-onabort>
    ///
    /// IDL event handler attribute: `attribute EventHandler onabort;`
    #[getter]
    fn onabort(&self, scope: &Scope<'_>, args: &CallArgs) -> Result<(), ExnThrown> {
        match &self.data().onabort_handler {
            Some(heap) => {
                let func = heap.get(scope);
                args.rval().set(func.as_value());
            }
            None => {
                args.rval().set(value::null());
            }
        }
        Ok(())
    }

    /// <https://dom.spec.whatwg.org/#dom-abortsignal-onabort>
    #[setter]
    fn set_onabort(&self, scope: &Scope<'_>, val: HandleValue<'_>) {
        // Remove the old handler listener, if any. Root it before taking the
        // mutable borrow of `self`.
        let old_callback = self.data().onabort_handler.get(scope);
        if let Some(func) = old_callback {
            event_algorithms::remove_an_event_listener(
                &mut self.data_mut().parent,
                "abort",
                &func,
                false,
            );
        }

        // If the new value is a function, store it and add as listener.
        if val.is_null() || val.is_undefined() {
            self.data_mut().onabort_handler = None;
        } else if let Ok(obj) = Object::from_value(scope, *val) {
            if let Ok(func) = obj.cast::<Function<'_>>() {
                // View this signal as the EventTarget it extends to register the
                // listener (the onabort handler never carries its own signal).
                let target = Object::from_value(scope, self.as_value())
                    .expect("AbortSignal is an object")
                    .cast::<EventTarget<'_>>()
                    .expect("AbortSignal is an EventTarget");
                event_algorithms::add_an_event_listener(
                    &target,
                    "abort".to_string(),
                    func,
                    false,
                    None,
                    false,
                    None,
                );
                self.data_mut().onabort_handler = Some(Heap::from(func));
            } else {
                self.data_mut().onabort_handler = None;
            }
        } else {
            self.data_mut().onabort_handler = None;
        }
    }

    /// <https://dom.spec.whatwg.org/#dom-abortsignal-abort>
    #[static_method]
    pub fn abort<'r>(
        scope: &'r Scope<'_>,
        reason: Option<HandleValue<'_>>,
    ) -> Result<AbortSignal<'r>, ExnThrown> {
        // Step 1: Let _signal_ be a new ``AbortSignal`` object.
        let signal = AbortSignal::new(scope)?;

        // Step 2: Set _signal_'s `abort reason` to _reason_ if it is given; otherwise to a new
        //         "``AbortError``" ``DOMException``.
        match reason {
            Some(r) => signal.data_mut().abort_reason.set(r.get()),
            None => {
                let err_val = algorithms::create_abort_error(scope)?;
                signal.data_mut().abort_reason.set(err_val.get());
            }
        }

        // Step 3: Return _signal_.
        Ok(signal)
    }

    /// <https://dom.spec.whatwg.org/#dom-abortsignal-timeout>
    #[static_method]
    pub fn timeout<'r>(
        scope: &'r Scope<'_>,
        milliseconds: f64,
    ) -> Result<AbortSignal<'r>, ExnThrown> {
        // Step 1: Let _signal_ be a new ``AbortSignal`` object.
        let signal = AbortSignal::new(scope)?;

        // Step 2: Let _global_ be _signal_'s `relevant global object`.
        // Step 3: `Run steps after a timeout` given _global_, "`AbortSignal-timeout`",
        //         _milliseconds_, and the following step: `Queue a global task` on the `timer task
        //         source` given _global_ to `signal abort` given _signal_ and a new
        //         "``TimeoutError``" ``DOMException``. For the duration of this timeout, if
        //         _signal_ has any event listeners registered for its ``abort`` event, there must
        //         be a strong reference from _global_ to _signal_.
        algorithms::schedule_abort_timeout(scope, &signal, milliseconds);

        // Step 4: Return _signal_.
        Ok(signal)
    }

    /// <https://dom.spec.whatwg.org/#dom-abortsignal-any>
    #[static_method]
    pub fn any<'r>(
        scope: &'r Scope<'_>,
        // TODO: use a WebIDL sequence for this, after making that usable outside of unions.
        signals: HandleValue<'_>,
    ) -> Result<AbortSignal<'r>, ExnThrown> {
        let signals = read_signal_sequence(scope, signals)?;

        // Step 1: Return the result of creating a dependent abort signal from signals using
        //         AbortSignal and the current realm.
        algorithms::create_dependent_abort_signal(scope, &signals)
    }
}

/// Convert a `sequence<AbortSignal>` argument into a `Vec` of signals.
///
/// WebIDL sequences are defined in terms of the iterator protocol; this only
/// handles array-like inputs (index access via the `length` property), which
/// covers every enabled caller. A non-`AbortSignal` element throws a
/// `TypeError`.
fn read_signal_sequence<'r>(
    scope: &'r Scope<'_>,
    value: HandleValue<'_>,
) -> Result<Vec<AbortSignal<'r>>, ExnThrown> {
    let obj = Object::from_value(scope, *value)
        .map_err(|_| js::error::throw_type_error(scope, c"AbortSignal.any expects a sequence"))?;

    let len_val = obj.get_property(scope, c"length")?;
    let length = if len_val.is_int32() {
        len_val.to_int32().max(0) as u32
    } else if len_val.is_double() {
        len_val.to_double().max(0.0) as u32
    } else {
        0
    };

    let mut signals = Vec::new();
    for i in 0..length {
        let elem = obj.get_element(scope, i)?;
        let elem_obj = Object::from_value(scope, *elem).map_err(|_| {
            js::error::throw_type_error(scope, c"AbortSignal.any: element is not an AbortSignal")
        })?;
        let signal = elem_obj.cast::<AbortSignal<'_>>().map_err(|_| {
            js::error::throw_type_error(scope, c"AbortSignal.any: element is not an AbortSignal")
        })?;
        signals.push(signal);
    }

    Ok(signals)
}
