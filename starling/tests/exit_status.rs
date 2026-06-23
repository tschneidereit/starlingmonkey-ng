// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! End-to-end exit-status checks for the `starling` binary.

#![cfg(not(target_arch = "wasm32"))]

use std::process::Command;

/// An error before the event loop runs — here an unreadable `-i` initializer
/// script — must exit with code 1. The Runtime is still alive when
/// `process::exit` fires the atexit handler, so a shutdown that insisted on
/// dropping the engine would trip mozjs's outstanding-handle assert and turn
/// the exit into a SIGABRT.
#[test]
fn initializer_error_exits_with_code_one() {
    let out = Command::new(env!("CARGO_BIN_EXE_starling"))
        .args(["-i", "/nonexistent/init.js", "/nonexistent/app.js"])
        .output()
        .expect("failed to run starling");
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected a clean exit(1), got {:?} (stderr: {})",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Scheduling APIs work during initializer evaluation, and the initializer's
/// microtasks drain before the content script runs — but async work left
/// behind has nothing to drive it and must be a hard error.
#[test]
fn initializer_event_loop_semantics() {
    let dir = tempfile::tempdir().unwrap();

    // Microtasks queued by the initializer are visible to the content script.
    let init = dir.path().join("init.js");
    std::fs::write(
        &init,
        "Promise.resolve().then(() => { globalThis._fromInit = 'ready'; });",
    )
    .unwrap();
    let main = dir.path().join("main.mjs");
    std::fs::write(
        &main,
        "if (globalThis._fromInit !== 'ready') { throw new Error('initializer microtask not drained'); }",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_starling"))
        .args(["-i", &init.to_string_lossy(), &main.to_string_lossy()])
        .output()
        .expect("failed to run starling");
    assert_eq!(
        out.status.code(),
        Some(0),
        "initializer microtasks must drain before content (stderr: {})",
        String::from_utf8_lossy(&out.stderr)
    );

    // setTimeout no longer throws "No active event loop" — but a timer left
    // pending when the initializer ends is an error.
    let init_timer = dir.path().join("init_timer.js");
    std::fs::write(&init_timer, "setTimeout(() => {}, 1000);").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_starling"))
        .args(["-i", &init_timer.to_string_lossy(), &main.to_string_lossy()])
        .output()
        .expect("failed to run starling");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("left asynchronous work"),
        "expected the leftover-async-work error, got: {stderr}"
    );

    // A timer scheduled and cleared synchronously is fine.
    let init_cleared = dir.path().join("init_cleared.js");
    std::fs::write(
        &init_cleared,
        "globalThis._fromInit = 'ready'; var t = setTimeout(() => {}, 1000); clearTimeout(t);",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_starling"))
        .args([
            "-i",
            &init_cleared.to_string_lossy(),
            &main.to_string_lossy(),
        ])
        .output()
        .expect("failed to run starling");
    assert_eq!(
        out.status.code(),
        Some(0),
        "a synchronously cleared timer must not fail init (stderr: {})",
        String::from_utf8_lossy(&out.stderr)
    );
}
