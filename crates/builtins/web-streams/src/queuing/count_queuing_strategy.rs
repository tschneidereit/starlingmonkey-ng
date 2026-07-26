// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/>

use super::QueuingStrategyInit;
use core_runtime::{webidl_interface, webidl_methods};
use js::error::ExnThrown;
use js::gc::scope::Scope;
use js::native::Value;
use js::prelude::{CallbackArgs, HandleValue};
use js::{value, Function};

/// <https://streams.spec.whatwg.org/#cqs-class>
#[webidl_interface]
pub struct CountQueuingStrategy {
    /// <https://streams.spec.whatwg.org/#cqs-internal-slots>
    #[no_trace]
    high_water_mark: f64,
}

/// The `count queuing strategy size function`: an algorithm that returns 1.
///
/// <https://streams.spec.whatwg.org/#count-queuing-strategy-size-function>
fn count_queuing_strategy_size(
    _scope: &Scope<'_>,
    _args: CallbackArgs<'_>,
    _payload: HandleValue<'_>,
) -> Result<Value, ExnThrown> {
    Ok(value::from_f64(1.0))
}

#[webidl_methods]
impl CountQueuingStrategy {
    /// <https://streams.spec.whatwg.org/#cqs-constructor>
    #[constructor]
    fn new(init: QueuingStrategyInit) -> Self {
        // Step 1: Set `this`.`[[highWaterMark]]` to _init_["``highWaterMark``"].
        CountQueuingStrategyImpl {
            high_water_mark: init.high_water_mark,
        }
    }

    /// <https://streams.spec.whatwg.org/#cqs-high-water-mark>
    #[getter]
    fn high_water_mark(&self) -> f64 {
        // Step 1: Return `this`.`[[highWaterMark]]`.
        self.data().high_water_mark
    }

    /// <https://streams.spec.whatwg.org/#cqs-size>
    #[getter]
    fn size<'r>(&self, scope: &'r Scope<'_>) -> Result<Function<'r>, ExnThrown> {
        // Step 1: Return `this`’s `relevant global object`’s `count queuing strategy size
        //         function`.
        // The size function is a per-global singleton (its identity is observable via `===`).
        js::class::get_or_init_shared_function(
            scope,
            count_queuing_strategy_size as *const () as usize,
            |scope| {
                let undef = HandleValue::undefined();
                Function::new_callback(scope, c"size", 0, count_queuing_strategy_size, undef)
            },
        )
    }
}
