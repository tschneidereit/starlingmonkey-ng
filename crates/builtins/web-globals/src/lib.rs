// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Web platform global functions and interfaces.
//!
//! ## Globals
//!
//! - `btoa` and `atob`: [WHATWG Forgiving Base64] encoding/decoding
//! - `console`: Basic console logging (`log`, `info`, `debug`, `warn`, `error`)
//! - `DOMException`: [WebIDL DOMException] interface
//!
//! [WHATWG Forgiving Base64]: https://infra.spec.whatwg.org/#forgiving-base64
//! [WebIDL DOMException]: https://webidl.spec.whatwg.org/#idl-DOMException

pub mod base64;
// pub mod console;
pub mod dom_exception;
pub mod wpt_support;

pub fn add_to_global(scope: &js::prelude::Scope<'_>, global: js::Object<'_>) {
    unsafe {
        // Note: the Rust console builtin isn't currently used: we use the C++ version for now.
        // console::console_ns::add_to_global(scope, global);
        dom_exception::DOMException::add_to_global(scope, global);
        base64::base64_globals::add_to_global(scope, global);
    }
}
