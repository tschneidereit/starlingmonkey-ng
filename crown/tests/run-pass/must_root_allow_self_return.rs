/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
//@rustc-env:RUSTC_BOOTSTRAP=1

#![allow(dead_code)]

// `allow_self_return` permits associated fns of the marked type to return it
// bare, whatever their name; callers binding the result are still checked.

#[crown::unrooted_must_root_lint::must_root(allow_self_return)]
struct Foo(i32);

impl Foo {
    fn scale(&self, factor: i32) -> Foo {
        Foo(self.0 * factor)
    }

    fn origin() -> Foo {
        Foo(0)
    }
}

trait Doubled {
    fn doubled(&self) -> Self;
}

impl Doubled for Foo {
    fn doubled(&self) -> Foo {
        Foo(self.0 * 2)
    }
}

fn main() {}
