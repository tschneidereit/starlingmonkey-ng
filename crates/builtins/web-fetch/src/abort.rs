// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! In-flight `fetch` abort.
//!
//! `fetch` registers an abort algorithm on the request's `AbortSignal`. When the
//! signal aborts while the request is in flight, the fetch promise is rejected
//! with the signal's abort reason. No-op when response delivery has completed.

use crate::body_mixin::BodyMixin;
use crate::incoming_body::HostBackedBodyOwner;
use crate::request::Request;
use crate::response::Response;
use core_runtime::jsclass;
use core_runtime::jsmethods;
use js::conversion::FromJSVal;
use js::error::ExnThrown;
use js::function::CallbackArgs;
use js::gc::handle::{Heap, OptionHeapExt};
use js::gc::scope::Scope;
use js::native::Value;
use js::prelude::HandleValue;
use js::{value, Function, Promise};
use web_globals::signals::{AbortSignal, AbortSignalImpl};

/// State for the abort algorithm: the fetch promise to reject when the signal
/// aborts, and (once delivered) the response whose body should be aborted.
///
/// The signal, and the callback registered on it, are kept so the algorithm can
/// deregister itself once a later abort could no longer do anything — see
/// [`AbortFetchState::detach`].
#[jsclass(hidden)]
pub struct AbortFetchState {
    promise: Heap<js::promise::Promise>,
    signal: Option<Heap<AbortSignalImpl>>,
    request: Option<Heap<crate::request::RequestImpl>>,
    response: Option<Heap<crate::response::ResponseImpl>>,
    callback: Option<Heap<js::function::Function>>,
}

#[jsmethods]
impl AbortFetchState {
    fn new(promise: Promise, request: Request, signal: AbortSignal) -> Self {
        Self {
            promise: Heap::from(promise),
            signal: Some(Heap::from(signal)),
            request: Some(Heap::from(request)),
            response: None,
            callback: None,
        }
    }
}

impl AbortFetchState<'_> {
    /// Record the delivered response, so a later abort can also abort its body.
    pub(crate) fn set_response(&self, response: &Response<'_>) {
        debug_assert!(self.data().response.is_none());
        self.data_mut().response = Some(Heap::from(*response));
    }

    /// Deregister the abort algorithm from the signal and drop what it held.
    ///
    /// An `AbortSignal` outlives the fetches made with it, so an algorithm left
    /// registered would accumulate one entry per fetch, each rooting that fetch's
    /// promise, the `Response`, and the `Response`'s host body and connection.
    /// Detaching once the algorithm can no longer have an effect keeps a reused
    /// signal from growing without bound.
    pub(crate) fn detach(&self, scope: &Scope<'_>) {
        let signal = self.data_mut().signal.take_rooted(scope);
        let callback = self.data_mut().callback.take_rooted(scope);
        if let (Some(signal), Some(callback)) = (signal, callback) {
            web_globals::signals::algorithms::remove_abort_algorithm(&signal, &callback);
        }
        self.data_mut().request = None;
        self.data_mut().response = None;
    }
}

/// Whether a delivered response still has something an abort could act on: an
/// unread host body, or a `.body` stream that has not finished.
///
/// Once neither is true, aborting is a no-op, so the algorithm can be detached.
pub(crate) fn response_body_is_abortable(scope: &Scope<'_>, response: &Response<'_>) -> bool {
    response.has_unread_host_body() || response.body_stream_is_unfinished(scope)
}

/// <https://fetch.spec.whatwg.org/#abort-fetch>
/// To abort a fetch() call with a promise, request, responseObject, and an error:
pub(crate) fn abort_fetch(
    scope: &Scope<'_>,
    promise: &Promise<'_>,
    request: &Request<'_>,
    response_object: Option<&Response<'_>>,
    error: HandleValue<'_>,
) -> Result<(), ExnThrown> {
    // Step 1: `Reject` _promise_ with _error_.
    // Note: This is a no-op if _promise_ has already fulfilled.
    promise.reject(scope, error)?;

    // Step 2: If _request_’s `body` is non-null and is `readable`, then `cancel` _request_’s `body`
    //     with _error_.
    // A body has a stream to cancel here once one exists for it: an author's `ReadableStream` body
    // always, and a host-source body once content asked for it (they are materialized lazily, see
    // `BodyMixin::body_value`). What cancelling it means depends on what is behind it:
    //   - An author stream: its `cancel` steps run.
    //   - A proxy for a host stream, e.g. for `fetch(incomingRequest)`: cancel's the proxied
    //     stream, so the host body reached through one is dropped and its connection closed.
    //   - A host-source stream forwarded straight to the transport (`body: response.body`): a no-op
    //     here, but released instead by aborting the controller, which drops the transport future
    //     that now owns it. `on_abort` does that before calling this algorithm.
    if let Some(stream) = HostBackedBodyOwner::body_stream(request, scope) {
        if stream.is_readable() {
            stream.cancel_internal(scope, error);
        }
    }

    // Cancelling a body `disturbs` it, so `bodyUsed` reports true afterwards. A body with
    // no materialized stream has only the flag standing in for that.
    request.set_source_disturbed();

    // Step 3: If _responseObject_ is null, then return.
    let Some(response_object) = response_object else {
        return Ok(());
    };

    // Step 4: Let _response_ be _responseObject_’s `response`.
    // Step 5: If _response_’s `body` is non-null and is `readable`, then `error` _response_’s
    //     `body` with _error_.
    // `abort_body` errors the `.body` stream and stops the host read that feeds it.
    response_object.abort_body(scope, error);
    Ok(())
}

/// <https://fetch.spec.whatwg.org/#dom-global-fetch> step 11.
///
/// Add an abort algorithm to `signal` that rejects `promise` with the abort reason
/// (and aborts the response body if one has been delivered). Returns the state so
/// `fetch` can record the response on it once delivered.
pub(crate) fn register_fetch_abort<'r>(
    scope: &'r Scope<'_>,
    signal: AbortSignal<'_>,
    promise: Promise<'_>,
    request: Request<'_>,
) -> Result<AbortFetchState<'r>, ExnThrown> {
    let state = AbortFetchState::new(scope, promise, request, signal)?;
    let callback = Function::new_callback(scope, c"", 1, on_abort, state)?;
    web_globals::signals::algorithms::add_abort_algorithm(&signal, &callback);
    state.data_mut().callback = Some(Heap::from(callback));

    // Detach once the fetch settles with nothing left to abort — a network error, or a response
    // with no body. A response that still has a body keeps the algorithm registered (aborting
    // mid-read must still error the stream) and detaches when that body is consumed instead.
    //
    // The reactions must not mark the fetch promise's rejection as handled: that would suppress
    // unhandled-rejection reporting for a `fetch()` the caller never catches.
    let settled = Function::new_callback(scope, c"", 1, on_settled, state)?;
    promise.add_reactions_ignoring_unhandled_rejection(scope, Some(*settled), Some(*settled))?;
    Ok(state)
}

/// The fetch promise settled: drop the abort algorithm unless the delivered
/// response still has a body an abort could error.
fn on_settled(
    scope: &Scope<'_>,
    _args: CallbackArgs<'_>,
    payload: HandleValue<'_>,
) -> Result<Value, ExnThrown> {
    let state = AbortFetchState::from_jsval_throwing(scope, payload, ())?;
    if let Some(response) = state.data().response.get(scope) {
        if response_body_is_abortable(scope, &response) {
            // Still abortable: `Response::consume` detaches once the body has been read.
            response.set_abort_state(&state);
            return Ok(value::undefined());
        }
    }
    state.detach(scope);
    Ok(value::undefined())
}

/// <https://fetch.spec.whatwg.org/#dom-global-fetch> steps 11.1 to 11.4.
///
/// The registered abort algorithm.
fn on_abort(
    scope: &Scope<'_>,
    args: CallbackArgs<'_>,
    payload: HandleValue<'_>,
) -> Result<Value, ExnThrown> {
    let state = AbortFetchState::from_jsval_throwing(scope, payload, ())?;
    let promise = state.data().promise.get(scope);
    let reason = args.get(0);

    // Step 11.1: Set _locallyAborted_ to true.
    // (not applicable)
    // Step 11.2: `Assert`: _controller_ is non-null.
    // (implicit)
    // Step 11.3: `Abort` _controller_ with _requestObject_’s `signal`’s `abort reason`.
    // Implemented by dropping the transport future. That future owns the outgoing body, so
    // dropping it frees that body, too.
    js::promise::cancel_pending_future(promise);

    // Step 11.4: `Abort the `fetch()` call` with _p_, _request_, _responseObject_, and
    //     _requestObject_’s `signal`’s `abort reason`.
    let request = state.data().request.get(scope);
    let response = state.data().response.get(scope);
    if let Some(request) = request {
        abort_fetch(scope, &promise, &request, response.as_ref(), reason)?;
    }
    state.detach(scope);

    Ok(value::undefined())
}
