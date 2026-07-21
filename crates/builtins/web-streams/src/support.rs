// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Non-spec plumbing shared by the stream algorithms: invoking the JS-callable
//! algorithms stored on controllers, attaching native promise reactions, and
//! building iterator-result objects.

use std::ffi::CStr;

use js::error::ExnThrown;
use js::function::Callback;
use js::gc::scope::Scope;
use js::prelude::HandleValue;
use js::prelude::ToJSVal;
use js::value;
use js::Function;
use js::{Object, Promise};

/// Invoke a stored promise-returning stream algorithm (a controller's pull or
/// cancel algorithm) following WebIDL "invoke a Promise-returning operation":
///
/// - an absent algorithm (stored as `undefined`) returns a promise resolved
///   with undefined;
/// - a synchronous throw becomes a rejected promise;
/// - otherwise the call result is coerced to a promise.
///
/// `receiver` is the `this` value (the underlying source for the
/// from-underlying-source path, or `undefined` for native algorithms).
pub(crate) fn invoke_promise_algorithm<'r>(
    scope: &'r Scope<'_>,
    algorithm: HandleValue<'r>,
    receiver: HandleValue<'r>,
    args: &[HandleValue<'r>],
) -> Promise<'r> {
    if algorithm.is_undefined() {
        let undef = scope.root_value(value::undefined());
        return Promise::new_resolved_with_value(scope, undef)
            .expect("failed to create resolved promise");
    }
    match Function::call(scope, receiver, algorithm, args) {
        Ok(result) => {
            if receiver.is_undefined() {
                // A native internal algorithm (tee, `Create*`, the transform
                // source/sink natives): its result already *is* the algorithm's
                // promise, which the controller reacts to directly. Coerce as-is
                // (no extra adoption tick), matching the spec's `uponPromise`.
                Promise::call_original_resolve(scope, result).expect("Promise.resolve failed")
            } else {
                // A WebIDL operation invocation (a user source/sink/transformer
                // method, called with the dictionary as `this`). WebIDL's
                // `Promise<T>` conversion of the result is `NewPromiseCapability`
                // + `Resolve` ("a promise resolved with"), which mints a *new*
                // promise rather than returning a promise result as-is. When the
                // method returns a promise (e.g. an async transformer method),
                // adopting it adds a microtask tick — observable in cancel and
                // erroring orderings.
                Promise::new_resolved_with_value(scope, result)
                    .expect("failed to create resolved promise")
            }
        }
        Err(_) => Promise::new_rejected_with_pending_error(scope)
            .expect("failed to create rejected promise"),
    }
}

/// Invoke a non-promise-returning stream algorithm (a controller's start
/// algorithm, or a strategy size algorithm) with `this` = `receiver`. Returns
/// the raw call result; a synchronous throw propagates as `Err`. An absent
/// algorithm (`undefined`) yields `undefined`.
pub(crate) fn invoke_algorithm<'r>(
    scope: &'r Scope<'_>,
    algorithm: HandleValue<'r>,
    receiver: HandleValue<'r>,
    args: &[impl ToJSVal<'r>],
) -> Result<HandleValue<'r>, ExnThrown> {
    if algorithm.is_undefined() {
        return Ok(scope.root_value(value::undefined()));
    }
    Function::call(scope, receiver, algorithm, args)
}

/// Attach native fulfillment/rejection reactions to `promise`, each carrying a
/// single payload value (typically the controller or reader the reaction
/// operates on). Mirrors the spec's "upon fulfillment / upon rejection of
/// promise" and "react to promise" phrasing.
///
/// These are internal reactions: the dependent promise is discarded and never
/// surfaced to author code, so it must not participate in unhandled-rejection
/// tracking. A fulfillment-only reaction on a promise that later rejects (e.g.
/// pipeTo's forward-close reaction on `reader.[[closedPromise]]`) would
/// otherwise produce a spurious unhandled rejection. Attaching the reaction also
/// marks `promise` itself handled, which is correct — the stream is consuming
/// it.
pub(crate) fn react(
    scope: &Scope<'_>,
    promise: &Promise<'_>,
    on_fulfilled: Option<(Callback, HandleValue<'_>)>,
    on_rejected: Option<(Callback, HandleValue<'_>)>,
) -> Result<(), ExnThrown> {
    let fulfilled = match on_fulfilled {
        Some((cb, payload)) => Some(Function::new_callback(scope, c"", 1, cb, payload)?),
        None => None,
    };
    let rejected = match on_rejected {
        Some((cb, payload)) => Some(Function::new_callback(scope, c"", 1, cb, payload)?),
        None => None,
    };
    promise.add_reactions_ignoring_unhandled_rejection(
        scope,
        fulfilled.map(|f| *f),
        rejected.map(|f| *f),
    )
}

/// Resolve a WebIDL callback dictionary member to the value stored as the
/// corresponding algorithm: `undefined` when the member is absent (the invoker
/// treats that as the default/constant algorithm), or the callable itself.
///
/// The callback dictionary members are held as plain [`Object`]s (so a bound
/// function or callable proxy is accepted), so the "callback function" type's
/// callability check is not applied during dictionary conversion — it is applied
/// here: a present member that is not callable is a `TypeError` (`message`).
pub(crate) fn callback_member<'r>(
    scope: &'r Scope<'_>,
    member: Option<&Object<'_>>,
    message: &CStr,
) -> Result<HandleValue<'r>, ExnThrown> {
    match member {
        None => Ok(scope.root_value(value::undefined())),
        Some(obj) if obj.is_callable() => Ok(scope.root_value(obj.as_value())),
        Some(_) => Err(js::error::throw_type_error(scope, message)),
    }
}

/// Build an ordinary `{ value, done }` iterator-result object (the spec's
/// `CreateIterResultObject`), returned as a value.
pub(crate) fn create_iter_result<'r>(
    scope: &'r Scope<'_>,
    chunk: HandleValue<'_>,
    done: bool,
) -> Result<HandleValue<'r>, ExnThrown> {
    let obj = Object::new_plain(scope)?;
    obj.set_property(scope, c"value", chunk)?;
    let done_val = scope.root_value(value::from_bool(done));
    obj.set_property(scope, c"done", done_val)?;
    Ok(scope.root_value(obj.as_value()))
}
