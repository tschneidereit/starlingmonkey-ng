// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Async stack capture: off by default, on with `--async-stacks`, and on in realms a `Debugger`
//! observes.

// This file contains nothing platform-specific, so skip it on wasm32.
#![cfg(not(target_arch = "wasm32"))]

use core_runtime::config::RuntimeConfig;
use core_runtime::runtime::Runtime;
use core_runtime::test_util::eval_and_read_out;

/// Stores the stack of an error created after an `await` in `globalThis.__out`.
const ASYNC_STACK: &str = "
    async function inner() { await null; return new Error().stack; }
    async function outer() { return await inner(); }
    outer().then(stack => { globalThis.__out = stack; });
";

fn async_stack(config: &str) -> String {
    let rt = Runtime::init(&RuntimeConfig::from_arg_string(config).unwrap()).expect("runtime init");
    let scope = rt.default_global();
    eval_and_read_out(&scope, ASYNC_STACK)
}

#[test]
fn async_stacks_are_off_by_default() {
    let stack = async_stack("");
    assert!(stack.starts_with("inner@"), "{stack}");
    assert!(!stack.contains("async*"), "{stack}");
}

#[test]
fn async_stacks_flag_records_async_parent_frames() {
    let stack = async_stack("--async-stacks");
    assert!(stack.contains("async*outer@"), "{stack}");
}

#[test]
fn debuggee_realms_record_async_parent_frames() {
    let rt = Runtime::init(&RuntimeConfig::default()).expect("runtime init");
    {
        let debugger_scope = rt.new_global();
        js::debug::define_debugger_object(&debugger_scope, debugger_scope.global().handle())
            .expect("defining Debugger");
        eval_and_read_out(
            &debugger_scope,
            "globalThis.dbg = new Debugger(); dbg.addAllGlobalsAsDebuggees(); \
             globalThis.__out = dbg.getDebuggees().length",
        );
    }
    let scope = rt.default_global();
    let stack = eval_and_read_out(&scope, ASYNC_STACK);
    assert!(stack.contains("async*outer@"), "{stack}");
}
