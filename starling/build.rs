// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    // Routes `__wasm_call_ctors` through `__wrap___wasm_call_ctors` in `lib.rs`, so every path
    // that runs the static constructors shares one guard. Without it they run twice and
    // SpiderMonkey's `InitializeUptime` aborts.
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        println!("cargo::rustc-link-arg-cdylib=--wrap=__wasm_call_ctors");
    }
}
