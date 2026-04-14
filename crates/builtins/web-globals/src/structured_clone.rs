// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! The `structuredClone()` global function.
//!
//! Implements the [structured cloning API] from the HTML specification,
//! which deep-copies a JavaScript value using the structured clone algorithm.
//! Supports an optional `transfer` list for transferring ownership of
//! `ArrayBuffer` and other transferable objects.
//!
//! [structured cloning API]: https://html.spec.whatwg.org/multipage/structured-data.html#dom-structuredclone

/// <https://html.spec.whatwg.org/multipage/structured-data.html#dom-structuredclone>
#[core_runtime::jsglobals]
pub mod structured_clone_globals {
    use js::error::ExnThrown;
    use js::gc::scope::Scope;
    use js::prelude::HandleValue;

    /// <https://html.spec.whatwg.org/multipage/structured-data.html#dom-structuredclone>
    ///
    /// The `structuredClone(value, options)` method steps are:
    ///
    /// 1. Let _serialized_ be ? StructuredSerializeWithTransfer(_value_,
    ///    _options_["`transfer`"]).
    /// 2. Let _deserializeRecord_ be ? StructuredDeserializeWithTransfer(
    ///    _serialized_, `this`'s relevant realm).
    /// 3. Return _deserializeRecord_.[[Deserialized]].
    pub fn structured_clone<'r>(
        scope: &'r Scope<'_>,
        value: HandleValue<'_>,
        options: Option<HandleValue<'_>>,
    ) -> Result<HandleValue<'r>, ExnThrown> {
        // Extract the transfer list from options, if provided.
        let transfer = if let Some(opts) = options {
            if opts.is_object() {
                let opts_obj = js::Object::from_value(scope, *opts).expect("is_object() was true");
                let transfer_val = opts_obj.get_property(scope, c"transfer")?;
                if transfer_val.is_undefined() {
                    None
                } else {
                    Some(transfer_val)
                }
            } else if opts.is_null_or_undefined() {
                None
            } else {
                return Err(js::error::throw_type_error(
                    scope,
                    c"structuredClone: options must be an object",
                ));
            }
        } else {
            None
        };

        match transfer {
            // Steps 1-3 (no transfer): use the simple clone path.
            None => unsafe {
                js::structured_clone::clone(scope, value, std::ptr::null(), std::ptr::null_mut())
            },
            // Steps 1-3 (with transfer): serialize with transfer, then deserialize.
            Some(transfer_val) => {
                js::structured_clone::clone_with_transfer(scope, value, transfer_val)
            }
        }
    }
}
