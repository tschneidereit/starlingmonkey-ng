// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Runtime configuration for the Fetch builtin.
//!
//! Several Fetch constraints are browser-security policies (forbidden request
//! and response headers, forbidden methods, no-CORS header/method safelisting)
//! rather than HTTP correctness rules. StarlingMonkey's primary use case is
//! server-side, where those policies are usually unwanted — a server must be
//! able to set `Host`, `Content-Length`, a `CONNECT` method, and so on. So they
//! are gated behind a single switch.
//!
//! The default is **enabled** (browser-compatible), which is what the Web
//! Platform Tests expect. An embedder can call
//! [`set_enforce_request_restrictions`]`(false)` to get permissive,
//! server-oriented behavior; the header-name/value *validity* checks and the
//! `immutable` guard are HTTP-correctness rules and are always enforced.

use std::cell::Cell;

thread_local! {
    static ENFORCE_REQUEST_RESTRICTIONS: Cell<bool> = const { Cell::new(true) };
}

/// Enable or disable enforcement of the browser-security Fetch constraints
/// (forbidden request/response headers, forbidden methods, no-CORS safelisting).
pub fn set_enforce_request_restrictions(enabled: bool) {
    ENFORCE_REQUEST_RESTRICTIONS.with(|cell| cell.set(enabled));
}

/// Whether the browser-security Fetch constraints are currently enforced.
#[inline]
pub fn enforce_request_restrictions() -> bool {
    ENFORCE_REQUEST_RESTRICTIONS.with(|cell| cell.get())
}
