// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://www.w3.org/TR/hr-time-3/#sec-performance/>
//!
//! Implements [Performance] API from the High Resolution Time specification
//! The time origin is the moment the runtime was initialized with the Performance Global

use std::time::Instant;

use core_runtime::{webidl_interface, webidl_methods};
use js::error::{ExnThrown, TypeError};
use js::gc::scope::Scope;
use js::Object;

use crate::events::event_target::{EventTarget, EventTargetImpl};

struct TimeOriginSnapshot {
    /// The monotonic clock instant when the time origin was captured.
    instant: Instant,
    /// ECMA-262 timestamp (ms since Unix epoch) at the time of the snapshot.
    /// Used to compute `timeOrigin` that is stable against system clock changes.
    epoch_ms: f64,
}

static TIME_ORIGIN: std::sync::OnceLock<TimeOriginSnapshot> = std::sync::OnceLock::new();

fn init_time_origin() {
    let _ = TIME_ORIGIN.get_or_init(|| {
        let instant = Instant::now();
        let epoch_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock is before Unix epoch")
            .as_secs_f64()
            * 1_000.;
        TimeOriginSnapshot { instant, epoch_ms }
    });
}

fn time_origin() -> &'static TimeOriginSnapshot {
    init_time_origin();
    TIME_ORIGIN.get().unwrap()
}

/// The `Performance` interface.
///
/// <https://www.w3.org/TR/hr-time-3/#sec-performance>
#[webidl_interface(extends = EventTarget)]
pub struct Performance {
    parent: EventTarget,
}

#[webidl_methods]
impl Performance {
    /// Performance has no public constructor; it's exposed only as a
    /// singleton on `globalThis.performance`.
    #[constructor]
    fn new(&self, _scope: &Scope<'_>) -> Result<(), TypeError> {
        Err(TypeError("Illegal constructor".into()))
    }

    /// <https://www.w3.org/TR/hr-time-3/#now-method>
    ///
    /// Return the number of milliseconds since the time origin, as a `double`.
    ///
    /// The IDL defines the return as `DOMHighResTimeStamp` which is a
    /// `double` in JS (64-bit float).
    #[method]
    fn now(&self) -> f64 {
        time_origin().instant.elapsed().as_secs_f64() * 1_000.
    }

    /// <https://www.w3.org/TR/hr-time-3/#timeorigin-attribute>
    ///
    /// Return the time origin as an ECMA-262 timestamp (ms since Unix epoch).
    /// This is a fixed value: the wall-clock time (in ms since Unix epoch)
    /// at which the runtime's time origin was established.
    #[getter]
    fn time_origin(&self) -> f64 {
        time_origin().epoch_ms
    }

    /// <https://www.w3.org/TR/hr-time-3/#tojson-method>
    ///
    /// Return a plain object with `timeOrigin` property.
    #[allow(clippy::wrong_self_convention)]
    #[method(name = "toJSON")]
    fn to_json<'a>(&self, scope: &'a Scope<'a>) -> Result<Object<'a>, ExnThrown> {
        let obj = Object::new_plain(scope)?;
        let time_origin_val = self.time_origin();
        obj.set_property(scope, c"timeOrigin", time_origin_val)?;
        Ok(obj)
    }
}

/// Register the `Performance` class on `global` and install the singleton
/// `performance` instance as a property on it.
pub fn add_to_global<'s>(scope: &'s Scope<'_>, global: Object<'s>) {
    // TODO: We should make this more robust for cases like
    // runtime snapshotting and resuming (wizer)
    // We could possibly capture initialization duration and then
    // at resumption set time origin to current time minus initialization duration
    init_time_origin();

    Performance::add_to_global(scope, global);

    let performance = unsafe {
        js::class::create_instance_with::<PerformanceImpl>(scope, |_| PerformanceImpl {
            parent: EventTargetImpl::default(),
        })
    }
    .expect("failed to allocate Performance singleton");

    global
        .set_property(scope, c"performance", performance)
        .expect("failed to define globalThis.performance");
}

#[cfg(test)]
mod tests {
    use core_runtime::{runtime, test_util::eval_with_setup};

    fn eval(code: &str) -> String {
        eval_with_setup(
            || {
                runtime::register_global_initializer(super::add_to_global);
            },
            code,
        )
    }

    #[test]
    fn performance_to_string_tag() {
        assert_eq!(
            eval("Object.prototype.toString.call(performance)"),
            "[object Performance]"
        );
    }

    #[test]
    fn to_json_returns_object() {
        assert!(eval(
            "(() => {
                const json = performance.toJSON();
                return typeof json === 'object' &&
                       typeof json.timeOrigin === 'number';
            })()"
        )
        .parse::<bool>()
        .unwrap());
    }
}
