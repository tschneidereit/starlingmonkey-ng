/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
//@rustc-env:RUSTC_BOOTSTRAP=1

// Without `allow_self_return`, an associated fn returning the bare type is
// flagged; and even with it, only *associated* fns of the type itself are
// exempt — free fns and other types' methods are not.

#[crown::unrooted_must_root_lint::must_root]
struct Unguarded(i32);

impl Unguarded {
    fn scale(&self) -> Unguarded {
        //~^ ERROR: Type must be rooted
        unimplemented!()
    }
}

#[crown::unrooted_must_root_lint::must_root(allow_self_return)]
struct Guarded(i32);

fn make_guarded() -> Guarded {
    //~^ ERROR: Type must be rooted
    unimplemented!()
}

struct Other;

impl Other {
    fn make_guarded(&self) -> Guarded {
        //~^ ERROR: Type must be rooted
        unimplemented!()
    }

    // Wrapping the return in a `Result` doesn't buy an exemption either.
    fn try_make_guarded(&self) -> Result<Guarded, ()> {
        //~^ ERROR: Type must be rooted
        unimplemented!()
    }
}

fn main() {}
