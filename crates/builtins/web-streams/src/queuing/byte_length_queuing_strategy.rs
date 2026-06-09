// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/>

use super::QueuingStrategyInit;
use core_runtime::{webidl_interface, webidl_methods};
use js::error::ExnThrown;
use js::gc::scope::Scope;
use js::native::Value;
use js::prelude::{CallbackArgs, HandleValue};
use js::{value, Function, Object};

/// <https://streams.spec.whatwg.org/#blqs-class>
#[webidl_interface]
pub struct ByteLengthQueuingStrategy {
    /// <https://streams.spec.whatwg.org/#blqs-internal-slots>
    #[no_trace]
    high_water_mark: f64,
}

/// The `byte length queuing strategy size function`: returns `chunk.byteLength`.
///
/// <https://streams.spec.whatwg.org/#byte-length-queuing-strategy-size-function>
fn byte_length_queuing_strategy_size(
    scope: &Scope<'_>,
    args: CallbackArgs<'_>,
    _payload: HandleValue<'_>,
) -> Result<Value, ExnThrown> {
    // Return ? `GetV`(_chunk_, "`byteLength`").
    let chunk = args.get(0);
    let obj = Object::from_value_coerce(scope, chunk).map_err(|_| ExnThrown)?;
    let byte_length = obj
        .get_property(scope, c"byteLength")
        .map_err(|_| ExnThrown)?;
    Ok(byte_length.get())
}

#[webidl_methods]
impl ByteLengthQueuingStrategy {
    /// <https://streams.spec.whatwg.org/#blqs-constructor>
    #[constructor]
    fn new(init: QueuingStrategyInit) -> Self {
        // Step 1: Set `this`.`[[highWaterMark]]` to _init_["``highWaterMark``"].
        ByteLengthQueuingStrategyImpl {
            high_water_mark: init.high_water_mark,
        }
    }

    /// <https://streams.spec.whatwg.org/#blqs-high-water-mark>
    #[getter]
    fn high_water_mark(&self) -> f64 {
        // Step 1: Return `this`.`[[highWaterMark]]`.
        self.data().high_water_mark
    }

    /// <https://streams.spec.whatwg.org/#blqs-size>
    #[getter]
    fn size<'r>(&self, scope: &'r Scope<'_>) -> Result<Function<'r>, ExnThrown> {
        // Step 1: Return `this`’s `relevant global object`’s `byte length queuing strategy size
        //         function`.
        // The size function is a per-global singleton (its identity is observable via `===`).
        js::class::get_or_init_shared_function(
            scope,
            byte_length_queuing_strategy_size as *const () as usize,
            |scope| {
                let undef = scope.root_value(value::undefined());
                Function::new_callback(scope, c"size", 1, byte_length_queuing_strategy_size, undef)
            },
        )
    }
}
