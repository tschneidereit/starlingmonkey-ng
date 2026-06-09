// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/>
use super::byte_stream_controller::ReadableByteStreamControllerImpl;
use core_runtime::{webidl_interface, webidl_methods};
use js::error::ExnThrown;
use js::gc::handle::Heap;
use js::gc::scope::Scope;
use js::native::Value;
use js::prelude::HandleValue;

/// <https://streams.spec.whatwg.org/#rs-byob-request-class>
#[webidl_interface(no_ctor)]
pub struct ReadableStreamBYOBRequest {
    /// <https://streams.spec.whatwg.org/#ReadableStreamBYOBRequest-controller>
    /// The parent ReadableByteStreamController instance, or null once the BYOB
    /// request has been invalidated.
    pub(crate) controller: Option<Heap<ReadableByteStreamControllerImpl>>,
    /// <https://streams.spec.whatwg.org/#ReadableStreamBYOBRequest-view>
    /// A typed array representing the destination region to which the controller can write generated
    /// data, or null after the BYOB request has been invalidated.
    pub(crate) view: Heap<Value>,
}

#[webidl_methods]
impl ReadableStreamBYOBRequest {
    /// <https://streams.spec.whatwg.org/#dom-ReadableStreamBYOBRequest-constructor>
    #[constructor]
    fn new() -> Self {
        ReadableStreamBYOBRequestImpl::default()
    }

    /// <https://streams.spec.whatwg.org/#rs-byob-request-view>
    #[getter]
    fn view<'r>(&self, scope: &'r Scope<'_>) -> Option<HandleValue<'r>> {
        // WebIDL: Uint8Array
        // Step 1: Return `this`.`[[view]]`.
        //         The slot holds a `Uint8Array` or null (after invalidation); a null is surfaced as
        //         the WebIDL nullable's `None`.
        if self.data().view.is_undefined() {
            return None;
        }
        let view = self.data().view.get(scope);
        if view.is_null() {
            None
        } else {
            Some(view)
        }
    }

    /// <https://streams.spec.whatwg.org/#rs-byob-request-respond>
    #[method]
    fn respond(&self, scope: &Scope<'_>, bytes_written: u64) -> Result<(), ExnThrown> {
        // Step 1: If `this`.`[[controller]]` is undefined, throw a ``TypeError`` exception.
        let controller = match self.data().controller.as_ref() {
            Some(c) => c.get(scope),
            None => {
                return Err(js::error::throw_type_error(
                    scope,
                    c"respond() called on an invalidated BYOB request",
                ))
            }
        };
        // Step 2: If ! `IsDetachedBuffer`(`this`.`[[view]]`.[[ArrayBuffer]]) is true, throw a
        //         ``TypeError`` exception.
        let view = js::Object::from_value(scope, self.data().view.get(scope).get())
            .ok()
            .and_then(js::ArrayBufferView::from_object)
            .expect("BYOB request view is an ArrayBufferView");
        if view.viewed_buffer(scope)?.is_detached() {
            return Err(js::error::throw_type_error(
                scope,
                c"respond() called with a detached view buffer",
            ));
        }
        // Step 3: Assert: `this`.`[[view]]`.[[ByteLength]] > 0.
        debug_assert!(view.byte_length() > 0);
        // Step 4: Assert: `this`.`[[view]]`.[[ViewedArrayBuffer]].[[ByteLength]] > 0.
        debug_assert!(view.viewed_buffer(scope)?.byte_length() > 0);
        // Step 5: Perform ? `ByteStreamControllerRespond`(`this`.`[[controller]]`,
        //         _bytesWritten_).
        super::algorithms::readable_byte_stream_controller_respond(
            scope,
            &controller,
            bytes_written,
        )
    }

    /// <https://streams.spec.whatwg.org/#rs-byob-request-respond-with-new-view>
    #[method]
    fn respond_with_new_view(
        &self,
        scope: &Scope<'_>,
        view: HandleValue<'_>, /* WebIDL: ArrayBufferView */
    ) -> Result<(), ExnThrown> {
        // The WebIDL signature coerces the argument to an `ArrayBufferView`; a non-view value is a
        // ``TypeError`` before the steps below run.
        let view = js::Object::from_value(scope, *view)
            .ok()
            .and_then(js::ArrayBufferView::from_object)
            .ok_or_else(|| {
                js::error::throw_type_error(
                    scope,
                    c"respondWithNewView() argument is not an ArrayBufferView",
                )
            })?;
        // Step 1: If `this`.`[[controller]]` is undefined, throw a ``TypeError`` exception.
        let controller = match self.data().controller.as_ref() {
            Some(c) => c.get(scope),
            None => {
                return Err(js::error::throw_type_error(
                    scope,
                    c"respondWithNewView() called on an invalidated BYOB request",
                ))
            }
        };
        // Step 2: If ! `IsDetachedBuffer`(_view_.[[ViewedArrayBuffer]]) is true, throw a
        //         ``TypeError`` exception.
        if view.viewed_buffer(scope)?.is_detached() {
            return Err(js::error::throw_type_error(
                scope,
                c"respondWithNewView() called with a detached view buffer",
            ));
        }
        // Step 3: Return ?
        //         `ByteStreamControllerRespondWithNewView`(`this`.`[[controller]]`,
        //         _view_).
        super::algorithms::readable_byte_stream_controller_respond_with_new_view(
            scope,
            &controller,
            view,
        )
    }
}
