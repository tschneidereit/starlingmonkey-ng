/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
//@rustc-env:RUSTC_BOOTSTRAP=1

#[crown::unrooted_must_root_lint::must_root]
struct Foo(i32);

// A non-`must_root` enum holding a `must_root` type must be flagged regardless
// of whether the variant is tuple-like or struct-like. Struct-like variants
// were previously not checked.
enum Bar {
    Tuple(Foo),
    //~^ ERROR: Type must be rooted, use #[js::must_root] on the enum definition to propagate
    Struct { field: Foo },
    //~^ ERROR: Type must be rooted, use #[js::must_root] on the enum definition to propagate
}

fn main() {}
