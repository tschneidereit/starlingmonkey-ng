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

// Register the completion callback that fires after done() is called. This handler serializes the
// test results as JSON and hands them to `__wptReportResults`, the sink the running mode installed:
// command mode's (from pre-harness.js) prints them for the runner to read off stdout, serve mode's
// (from wpt-server.js) delivers them into the response carrying this test. Calling it
// unconditionally keeps mode knowledge out of here — and makes a missing sink a loud TypeError
// rather than results quietly going nowhere.
add_completion_callback(function(tests, harness_status, asserts) {
  let results = tests.map(function(t) {
    return {
      name: t.name,
      status: t.status,
      message: t.message || null
    };
  });
  globalThis.__wptReportResults(JSON.stringify(results));
});
