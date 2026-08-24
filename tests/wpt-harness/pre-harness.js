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

// Where a finished test's results go. post-harness.js calls this and nothing else, so the running
// mode is expressed by which sink is installed rather than by the epilogue sniffing for one.
//
// Serve mode (wpt-server.js) installs its own — delivering into the response it is holding open —
// before evaluating this script, so this only supplies command mode's default.
globalThis.__wptReportResults ??= function(json) {
  // The runner reads this line off stdout.
  console.log("WPT_RESULTS_JSON:" + json);
  // Stop the event loop now that the results are out: a finished test may have left a live
  // setInterval (or other pending timer) running, which would otherwise keep the process alive
  // until the harness timeout. Serve mode's sink deliberately does not do this — there the
  // response is still to be sent, and this test's loop is the one carrying that work.
  if (typeof __wptDone === "function") __wptDone();
};
