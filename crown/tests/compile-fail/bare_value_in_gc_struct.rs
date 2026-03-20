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

use mozjs_sys::jsapi::JS::Value;
use mozjs_sys::jsgc::Heap;

// A struct marked allow_unrooted_interior with a bare Value field — should error.
#[crown::unrooted_must_root_lint::allow_unrooted_interior]
struct BadStream {
    state: u32,
    stored_error: Value, //~ ERROR: bare JS::Value in a GC-traced struct must be wrapped in Heap<Value>
}

// Option<Value> is also incorrect — Value inside Option has the same tracing problem.
#[crown::unrooted_must_root_lint::allow_unrooted_interior]
struct BadOptionalError {
    maybe_error: Option<Value>, //~ ERROR: bare JS::Value in a GC-traced struct must be wrapped in Heap<Value>
}

fn main() {}




