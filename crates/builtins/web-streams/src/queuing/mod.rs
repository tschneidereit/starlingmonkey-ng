// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! <https://streams.spec.whatwg.org/#queuing-strategies>

mod byte_length_queuing_strategy;
mod count_queuing_strategy;
mod queue_with_sizes;

use core_runtime::webidl_dictionary;

pub use byte_length_queuing_strategy::ByteLengthQueuingStrategy;
pub use count_queuing_strategy::CountQueuingStrategy;
use js::Object;
pub use queue_with_sizes::{QueueWithSizes, ValueWithSize};

/// <https://streams.spec.whatwg.org/#dictdef-queuingstrategy>
#[webidl_dictionary]
pub struct QueuingStrategy<'a> {
    pub high_water_mark: Option<f64>,
    pub size: Option<QueuingStrategySize<'a>>,
}

/// <https://streams.spec.whatwg.org/#dictdef-queuingstrategyinit>
#[webidl_dictionary]
pub struct QueuingStrategyInit {
    pub high_water_mark: f64,
}

/// WebIDL callback `QueuingStrategySize`: (chunk: HandleValue<'_>) -> f64
pub type QueuingStrategySize<'s> = Object<'s>;
