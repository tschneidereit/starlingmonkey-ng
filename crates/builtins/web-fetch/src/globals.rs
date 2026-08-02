// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Global functions and constants from <https://fetch.spec.whatwg.org/>

use core_runtime::jsglobals;

#[jsglobals]
pub mod globals {
    use crate::abort::AbortFetchState;
    use crate::request::{Request, RequestInfo, RequestInit};
    use crate::response::response_from_platform;
    use js::conversion::ToJSVal;
    use js::error::throw_type_error;
    use js::error::ExnThrown;
    use js::gc::handle::RootedHeap;
    use js::gc::scope::Scope;
    use js::prelude::HandleValue;
    use js::promise::{PromiseFuture, PromiseOutcome};
    use js::{Function, Object, Promise};

    /// <https://fetch.spec.whatwg.org/#dom-global-fetch>
    ///
    /// Note: This skips a lot of machinery relevant to browsers, and in particular their security
    /// model, including CORS, cache, and redirect taint.
    pub fn fetch<'r>(
        scope: &'r Scope<'_>,
        input: RequestInfo<'_>,
        init: Option<RequestInit<'_>>,
    ) -> Result<Promise<'r>, ExnThrown> {
        // Step 1: Let _p_ be `a new promise`.
        let p = Promise::new_pending(scope)?;

        // Step 2: Let _requestObject_ be the result of invoking the initial value of `Request` as
        //     constructor with _input_ and _init_ as arguments. If this throws an exception,
        //     `reject` _p_ with it and return _p_.
        let request_object = match Request::new(scope, input, init) {
            Ok(object) => object,
            Err(_) => {
                p.reject_with_pending(scope)?;
                return Ok(p);
            }
        };

        // Step 3: Let _request_ be _requestObject_’s `request`.
        // All `request` state is stored on the object, so just rename it to align with the spec.
        let request = request_object;

        // Step 4: If _requestObject_’s `signal` is `aborted`, then:
        let signal = request.signal(scope);
        if signal.aborted() {
            // Step 4.1: `Abort the `fetch()` call` with _p_, _request_, null, and _requestObject_’s
            //     `signal`’s `abort reason`.
            let reason = signal.reason(scope);
            crate::abort::abort_fetch(scope, &p, &request, None, reason)?;
            // Step 4.2: Return _p_.
            return Ok(p);
        }
        let abort_state = Some(crate::abort::register_fetch_abort(
            scope, signal, p, request,
        )?);

        // Step 5: Let _globalObject_ be _request_’s `client`’s `global object`.
        // Step 6: If _globalObject_ is a `ServiceWorkerGlobalScope` object, then set _request_’s
        //     `service-workers mode` to "`none`".
        // Step 7: Let _responseObject_ be null.
        // Step 8: Let _relevantRealm_ be `this`’s `relevant realm`.
        // (not applicable)

        // Step 9: Let _locallyAborted_ be false.
        // Step 10: Let _controller_ be null.
        // (not applicable)

        // Step 11: `Add the following abort steps` to _requestObject_’s `signal`:
        // Step 11.1: Set _locallyAborted_ to true.
        // Step 11.2: `Assert`: _controller_ is non-null.
        // Step 11.3: `Abort` _controller_ with _requestObject_’s `signal`’s `abort reason`.
        // Step 11.4: `Abort the `fetch()` call` with _p_, _request_, _responseObject_, and
        //     _requestObject_’s `signal`’s `abort reason`.
        // Step 12: Set _controller_ to the result of calling `fetch` given _request_ and
        //     `_processResponse_` given _response_ being these steps:
        // Step 12.1: If _locallyAborted_ is true, then abort these steps.
        // Step 12.2: If _response_’s `aborted flag` is set, then:
        // Step 12.2.1: Let _deserializedError_ be the result of `deserialize a serialized abort
        //     reason` given _controller_’s `serialized abort reason` and _relevantRealm_.
        // Step 12.2.2: `Abort the `fetch()` call` with _p_, _request_, _responseObject_, and
        //     _deserializedError_.
        // Step 12.2.3: Abort these steps.
        // Step 12.3: If _response_ is a `network error`, then `reject` _p_ with a `TypeError` and
        //     abort these steps.
        // Step 12.4: Set _responseObject_ to the result of `creating` a `Response` object, given
        //     _response_, "`immutable`", and _relevantRealm_.
        // Step 12.5: `Resolve` _p_ with _responseObject_.
        //
        // _processResponse_ is applied below to whichever outcome `fetch` produced. The
        // _locallyAborted_ / `serialized abort reason` bookkeeping is not needed: an abort before
        // this point returned at Step 4, and rejecting an already-settled promise is a no-op.
        match crate::algorithms::fetch(scope, &request)? {
            // _processResponse_: Set _responseObject_ to the result of `creating` a `Response`
            // object, given _response_, "`immutable`", and _relevantRealm_ — done by `scheme
            // fetch`'s "`data`" arm — then `Resolve` _p_ with _responseObject_.
            crate::algorithms::FetchOutcome::Response(response_object) => {
                if let Some(state) = &abort_state {
                    state.set_response(&response_object);
                }
                p.resolve(scope, response_object)?;
                return Ok(p);
            }
            // _processResponse_: If _response_ is a `network error`, then `reject` _p_ with a
            // `TypeError` and abort these steps. Throw one to set the pending exception, then
            // take it as the rejection value.
            crate::algorithms::FetchOutcome::NetworkError => {
                throw_type_error(scope, c"Failed to fetch");
                p.reject_with_pending(scope)?;
                return Ok(p);
            }
            // The response is produced by the host transport, so _processResponse_ runs when the
            // future below completes.
            crate::algorithms::FetchOutcome::Network => {}
        }
        // `main fetch` handed off to [`HTTP fetch`](https://fetch.spec.whatwg.org/#concept-http-fetch)
        // and below, which are `transport::send_following_redirects`. On completion,
        // _processResponse_: `response_from_platform` creates the `Response` object with an
        // "`immutable`" headers guard and _p_ resolves with it; a transport error is a `network
        // error`, rejecting _p_ with a `TypeError`.
        let platform_request = request.platform_request(scope);
        let url = request.current_url();
        let redirect_mode = request.redirect_mode();
        let mode = request.request_mode();
        let method = platform_request.method.clone();
        // Root the abort state across the host fetch, so the resolve callback can
        // record the delivered response on it. `on_settled` keeps the abort
        // algorithm registered only when the response is known to it.
        let abort_state = abort_state.map(RootedHeap::new);
        let future = async move {
            match crate::transport::send_following_redirects(
                platform_request,
                redirect_mode,
                mode,
                url,
            )
            .await
            {
                Ok((response, url_list)) => {
                    PromiseOutcome::Resolve(Box::new(move |scope: &Scope<'_>| {
                        let response = response_from_platform(
                            scope,
                            response,
                            url_list,
                            &method,
                            redirect_mode,
                        )?;
                        if let Some(state) = &abort_state {
                            state.get(scope).set_response(&response);
                        }
                        response.to_jsval_throwing(scope)
                    }))
                }
                Err(error) => PromiseOutcome::Reject(format!("Failed to fetch: {error}")),
            }
        };
        p.spawn(PromiseFuture::new(future));

        // Step 13: Return _p_.
        Ok(p)
    }
}
