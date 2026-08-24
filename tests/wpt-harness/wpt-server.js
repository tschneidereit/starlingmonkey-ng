// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception
//
// The serve-mode WPT runner: the content script the runtime runs under `--serve --wpt-mode`.
//
// Each request carries one assembled test script (harness + test) in its body; this evaluates it
// and answers with the JSON results. That puts the tests through the configuration that matters
// most — running inside a request handler, and on wasm against a wizened component — rather than
// through a one-shot command.
//
// There is no host-side WPT entry point: this is an ordinary `fetch` handler, so the same binary
// and the same code path serve it as serve any other request.
//
// Everything lives inside this IIFE because the tests are evaluated into *this* global. A bare
// top-level declaration here would be a global that a test can collide with — and testharness.js
// really does collide: it exposes a global `timeout()` that forces the harness to time out. A
// local `timeout` here shadowed by that one silently timed out every async test.
(() => {
  addEventListener("fetch", (event) => {
    event.respondWith(runTest(event.request));
  });

  const TIMED_OUT = Symbol("timed out");

  /// Bound the wait for a test that never reports, as the middle of three nested limits:
  /// testharness.js times a normal test out internally at 10s and reports per-subtest TIMEOUT
  /// statuses, and the runner's own per-test budget (`config.timeout`, 30s serial) is the outer
  /// kill. Sitting between them, this backstop only fires for a test that testharness itself could
  /// not finish — reporting it as a server error, and releasing the slot, before the runner gives
  /// up on the request. Undercutting testharness's 10s would instead turn every slow-but-reporting
  /// test into a 500 that command mode records real results for.
  const WAIT_LIMIT_MS = 20000;

  /// Evaluate one assembled test script and resolve with its results.
  async function runTest(request) {
    const script = await request.text();

    // One test at a time per global. testharness.js keeps its state (the test list, the completion
    // callbacks) on the global, as does the results hook below, so two tests running concurrently
    // in one global would interleave into nonsense — each delivering the other's results.
    //
    // Hosts that hand every request a fresh instance never reach this: their globals are unshared.
    // Where the global *is* shared (the native server), the runner sends tests one at a time. This
    // guard makes a violation of that a clear error rather than a silent hang.
    if (globalThis.__wptRunning) {
      return errorResponse(
        new Error(
          "a WPT test is already running in this global: run the server with one request in " +
            "flight at a time, or with one instance per request",
        ),
      );
    }
    globalThis.__wptRunning = true;

    // post-harness.js hands the results to `__wptReportResults` from the testharness completion
    // callback. Install this mode's sink before evaluating: a synchronous test completes during
    // `evalScript` below, so the hook has to already be there — and pre-harness.js only falls back
    // to command mode's sink when it finds none.
    //
    // Resolving the pending response is all it does. Stopping the event loop, which command mode's
    // sink does here, would end the very work that produces that response; this mode stops the loop
    // in the `finally` below instead, once the response exists.
    let deliver;
    const results = new Promise((resolve) => {
      deliver = resolve;
    });
    globalThis.__wptReportResults = deliver;

    try {
      try {
        // `evalScript` (a --wpt-mode global) evaluates in non-syntactic scope, so the script's
        // top-level `let`/`const` are visible to the `evalScript` calls it makes for its own META
        // dependencies — the semantics `<script>` tags have, which the harness relies on.
        evalScript(script, "wpt-test");
      } catch (e) {
        // A top-level throw means the harness never reached `done()`, so `results` never settles.
        return errorResponse(e);
      }

      // An async test finishes after `evalScript` returns, so wait for the completion callback.
      const limit = waitLimit();
      const json = await Promise.race([results, limit.reached]);
      // Cancel the limit once the test has reported. Left running it keeps this request's event
      // loop alive for the rest of its wait — which the runtime dutifully drains after the
      // response, holding the request's slot and, on a server handling one request at a time,
      // stalling the next one for exactly that long.
      limit.cancel();
      if (json === TIMED_OUT) {
        return errorResponse(new Error("the test did not complete"));
      }
      return new Response(json, {
        headers: { "content-type": "application/json" },
      });
    } finally {
      // Always give the slot back. A test that never completes would otherwise hold it for the
      // rest of the run, turning one hung test into a 503 for every test after it.
      globalThis.__wptRunning = false;
      // Stop this request's event loop, now that its response is built. A finished WPT test may
      // have left a live `setInterval` behind (`timers/clearinterval-from-callback` does), which
      // keeps the loop alive indefinitely — the runtime drains it after responding, so the request
      // never finishes and a server handling one at a time never accepts another. Command mode
      // calls this from the completion callback for the same reason; here it has to wait until the
      // response exists, since stopping the loop ends the work that produces it.
      if (typeof __wptDone === "function") __wptDone();
    }
  }

  /// The wait limit as a promise plus the means to call it off, so a test that reported in time
  /// leaves nothing pending behind it.
  function waitLimit() {
    let id;
    const reached = new Promise((resolve) => {
      id = setTimeout(() => resolve(TIMED_OUT), WAIT_LIMIT_MS);
    });
    return { reached, cancel: () => clearTimeout(id) };
  }

  /// A test that threw before reporting results is a 500, matching what the command-mode runner
  /// does with a runtime that exits without printing the results marker. Answering 200 with no
  /// results would otherwise read as a zero-subtest pass.
  function errorResponse(e) {
    // A SpiderMonkey `stack` carries only frames, so report the message separately or a thrown
    // SyntaxError arrives as an anonymous traceback.
    let message;
    try {
      message = e instanceof Error ? `${e.name}: ${e.message}\n${e.stack}` : String(e);
    } catch {
      message = "a non-Error value was thrown";
    }
    return new Response(JSON.stringify({ error: message }), {
      status: 500,
      headers: { "content-type": "application/json" },
    });
  }
})();
