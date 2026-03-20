/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
//@rustc-env:RUSTC_BOOTSTRAP=1

#![allow(dead_code)]
#![allow(non_snake_case)]

// Mock the mozjs_sys type paths.
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
        #[derive(Default)]
        pub struct Heap<T> {
            inner: T,
        }
    }
}

use mozjs_sys::jsapi::JS::Value;

// Vec<Value> inside an allow_unrooted_interior struct is also incorrect.
#[crown::unrooted_must_root_lint::allow_unrooted_interior]
struct BadQueueEntry {
    values: Vec<Value>, //~ ERROR: bare JS::Value in a GC-traced struct must be wrapped in Heap<Value>
}

fn main() {}

