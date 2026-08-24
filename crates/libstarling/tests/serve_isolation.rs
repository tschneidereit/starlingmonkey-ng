// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception
//! Per-request global isolation (`--serve-isolated`): a fresh global per request carries the
//! builtins, keeps one request's top-level declarations out of the next one's way, and receives
//! dispatch through its own listeners.

#![cfg(not(target_arch = "wasm32"))]

use core_runtime::runtime::{clear_global_initializers, Runtime};
use js::conversion::FromJSVal;
use js::gc::scope::Scope;
use web_fetch::request::Request;
use web_globals::events::algorithms::ScriptStackState;

/// A runtime with the builtins registered, ready for globals to be created from it. Each test
/// clears the initializers first: they accumulate in a process-wide registry, so a test inheriting
/// the previous one's would register each builtin twice over.
fn test_runtime() -> std::rc::Rc<Runtime> {
    clear_global_initializers();
    libstarling::register_builtins();
    Runtime::init(&core_runtime::config::RuntimeConfig::default())
}

fn eval(scope: &Scope<'_>, src: &str) -> Result<String, String> {
    match js::compile::evaluate_with_filename(scope, src, "<spike>", 1) {
        Ok(v) => String::from_jsval(scope, v, ()).map_err(|e| format!("{e:?}")),
        Err(_) => {
            let exn = js::error::ExnThrown::capture(scope);
            Err(format!("{exn}"))
        }
    }
}

#[test]
fn fresh_globals_are_isolated_and_initialized() {
    let rt = test_runtime();

    // Two globals, created the way a per-request server would.
    {
        let a = rt.new_global();
        // Builtins are installed on it: the initializers run per global.
        assert_eq!(
            eval(&a, "typeof addEventListener + ',' + typeof Response").unwrap(),
            "function,function",
            "a fresh global must carry the registered builtins"
        );
        // A top-level lexical declaration — the thing that collides across tests today.
        eval(&a, "const collide = 1; var marker = 'a';").unwrap();
        assert_eq!(eval(&a, "String(collide)").unwrap(), "1");
    }
    {
        let b = rt.new_global();
        // The same declaration must not collide, and `var` state must not leak.
        eval(&b, "const collide = 2;").expect("redeclaration must not collide across globals");
        assert_eq!(eval(&b, "String(collide)").unwrap(), "2");
        assert_eq!(
            eval(&b, "typeof marker").unwrap(),
            "undefined",
            "globals must not share var state"
        );
    }
}

#[test]
fn a_fetch_handler_registered_in_a_fresh_global_receives_dispatch() {
    let rt = test_runtime();

    let scope = rt.new_global();
    fetch_event::add_to_global(&scope, scope.global());

    // The content script, as a per-request server would re-evaluate it into this global.
    eval(
        &scope,
        "globalThis.__seen = null;
         addEventListener('fetch', (e) => { globalThis.__seen = e.request.url; });",
    )
    .unwrap();

    let el = core_runtime::event_loop::EventLoop::new();
    core_runtime::event_loop::with_event_loop(&el, |_| {
        let request = js::compile::evaluate_with_filename(
            &scope,
            "new Request('http://example.com/')",
            "<r>",
            1,
        )
        .expect("request value");
        let request = Request::from_jsval(&scope, request, ()).unwrap();
        fetch_event::fetch_event::FetchEvent::dispatch(&scope, request, ScriptStackState::Empty)
            .expect("dispatch");
        js::jobs::run_jobs(&scope);
    });

    assert_eq!(
        eval(&scope, "String(globalThis.__seen)").unwrap(),
        "http://example.com/",
        "the fresh global's own listener must receive the event"
    );
}
