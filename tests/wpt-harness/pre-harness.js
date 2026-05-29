// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception
//
// Pre-harness: sets up the global environment that the WPT testharness.js
// expects before any tests are loaded.
//
// This script runs before testharness.js is loaded.

// `self` is the global object in workers and the WPT harness expects it.
globalThis.self = globalThis;

// WPT testharness.js checks for GLOBAL.isWindow(), isWorker(), etc.
globalThis.GLOBAL = {
  isWindow: function() { return false; },
  isWorker: function() { return true; },
  isShadowRealm: function() { return false; },
};

// Some tests reference `window` or `Window`.
globalThis.window = globalThis;
globalThis.Window = {
  prototype: {}
};
