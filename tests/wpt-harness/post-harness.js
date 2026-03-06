// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception
//
// Post-harness: registers the WPT test completion callback and outputs
// results as JSON to stdout. This script runs after testharness.js has
// been loaded (via the concatenated harness script), immediately before
// the actual test scripts are evalScript'd by the runner.
//
// The Node.js orchestrator (run-wpt.mjs) injects evalScript calls for
// META scripts and the test source after this block, followed by a
// `done()` call.

// Tell testharness.js we'll call done() explicitly when ready.
setup({ explicit_done: true });

// Register the completion callback that fires after done() is called.
// This handler serializes the test results as JSON to stdout.
add_completion_callback(function(tests, harness_status, asserts) {
  let results = tests.map(function(t) {
    return {
      name: t.name,
      status: t.status,
      message: t.message || null
    };
  });
  // Print results as JSON — the orchestrator reads this from stdout.
  console.log("WPT_RESULTS_JSON:" + JSON.stringify(results));
});
