/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
//@rustc-env:RUSTC_BOOTSTRAP=1

#![allow(dead_code)]
#![allow(non_snake_case)]

// Mock the mozjs_sys::jsapi::JS::Value type path that the lint checks for.
mod mozjs_sys {
    pub mod jsapi {
        pub mod JS {
            #[derive(Default)]
            pub struct Value {
                pub as_bits: u64,
            }
        }
    }
    pub mod jsgc {
        /// Mock Heap<T> wrapper (the correct type for heap-resident GC values).
        #[derive(Default)]
        pub struct Heap<T> {
            inner: T,
        }
    }
}

use mozjs_sys::jsgc::Heap;
use mozjs_sys::jsapi::JS::Value;

// A struct marked allow_unrooted_interior using Heap<Value> — correct.
#[crown::unrooted_must_root_lint::allow_unrooted_interior]
struct GoodStream {
    state: u32,
    stored_error: Heap<Value>,
}

// A struct NOT marked allow_unrooted_interior with bare Value — not our concern.
struct PlainStruct {
    val: Value,
}

fn main() {}

