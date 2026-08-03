#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception
//
// Node.js compatibility test runner for StarlingMonkey.
//
// Usage:
//   node tests/node-compat/run.mjs [options] [pattern]
//
// Options:
//   --target=[native|wasm]  Execution target (default: native)
//   --runtime=PATH          Override the runtime binary path
//   --update                Regenerate per-file expectations from current results
//   -v                      Verbose: print individual test names

import { execFileSync, spawnSync } from "child_process";
import { existsSync, readFileSync, writeFileSync, mkdirSync, mkdtempSync, rmSync } from "fs";
import path from "path";

function rel(p) {
  return new URL(p, import.meta.url).pathname;
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

const config = {
  runtime: "target/debug/starling",
  runtimeExplicit: false,
  target: "native",
  upstreamDir: rel("node-test/test"),
  expectationsDir: rel("expectations"),
  pattern: "",
  verbose: false,
  update: false,
};

for (const arg of process.argv.slice(2)) {
  if (arg.startsWith("--runtime=")) {
    config.runtime = arg.split("=")[1];
    config.runtimeExplicit = true;
  } else if (arg.startsWith("--target=")) {
    const val = arg.split("=")[1];
    if (val !== "native" && val !== "wasm") {
      console.error(`Unknown --target value: ${val}. Use "native" or "wasm".`);
      process.exit(1);
    }
    config.target = val;
  } else if (arg === "--update") {
    config.update = true;
  } else if (arg === "-v") {
    config.verbose = true;
  } else if (arg[0] !== "-") {
    config.pattern = arg;
  } else {
    console.error(`Unknown argument: ${arg}`);
    process.exit(1);
  }
}

if (config.target === "wasm" && !config.runtimeExplicit) {
  config.runtime = "target/wasm32-wasip2/debug/starling.wasm";
}

if (!existsSync(config.runtime)) {
  const hint =
    config.target === "wasm"
      ? "Run 'just build-node-wasm' first."
      : "Run 'just build-node' first.";
  console.error(`Runtime not found: ${config.runtime}. ${hint}`);
  process.exit(1);
}

if (config.target === "wasm") {
  try {
    execFileSync("wasmtime", ["--version"], { encoding: "utf-8" });
  } catch {
    console.error("wasmtime not found. Install wasmtime to run tests on wasm.");
    process.exit(1);
  }
}

// ---------------------------------------------------------------------------
// Test discovery
// ---------------------------------------------------------------------------

function loadTestList() {
  const file = rel("upstream-tests.json");
  if (!existsSync(file)) return [];
  const list = JSON.parse(readFileSync(file, "utf-8"));
  return config.pattern ? list.filter((f) => f.includes(config.pattern)) : list;
}

function loadExpectationsForFile(testFile) {
  const p = path.join(config.expectationsDir, testFile + ".json");
  return existsSync(p) ? JSON.parse(readFileSync(p, "utf-8")) : {};
}

// ---------------------------------------------------------------------------
// Test execution
//
// NOTE: node:test framework tests (test()/describe()/it()) are not supported.
// Roughly 4% of upstream tests use node:test.  They are accepted by the shim
// as no-ops and fall through to the "script passed" synthetic result — meaning
// they will appear to PASS even if the test body is never executed.  Do not
// add node:test files to upstream-tests.json until native node:test support
// lands (see upstream-shim.js for details).
// ---------------------------------------------------------------------------

function spawnTest(tmpFile) {
  if (config.target === "wasm") {
    const wasiPath = "/" + path.relative(process.cwd(), tmpFile);
    return spawnSync("wasmtime", [
      "run", "--dir=.::/", "--dir=.", "--dir=/tmp",
      "-Sinherit-env=y,inherit-network=y,http=y,tcp=y,udp=y,p3=y",
      "-Wcomponent-model-async=y",
      config.runtime, wasiPath,
    ], { timeout: 10000, encoding: "utf-8", maxBuffer: 10 * 1024 * 1024 });
  }
  return spawnSync(config.runtime, [tmpFile], {
    timeout: 10000, encoding: "utf-8", maxBuffer: 10 * 1024 * 1024,
  });
}

function parseResults(stdout) {
  for (const line of stdout.split("\n")) {
    const s = line.replace(/^Log: */, "");
    if (s.startsWith("NODE_TEST_RESULTS:")) {
      return JSON.parse(s.slice("NODE_TEST_RESULTS:".length));
    }
  }
  return null;
}

function runTest(tmpDir, testPath, script) {
  const tmp = path.join(tmpDir, testPath.replace(/\//g, "_"));
  writeFileSync(tmp, script);
  const { stdout = "", stderr = "", error, status } = spawnTest(tmp);
  const results = parseResults(stdout);
  return results
    ? { results, stderr }
    : { error: error ?? new Error(`exit ${status}`), stderr, stdout };
}

function reportCrash(label, error, stderr, stdout) {
  console.error(`FAIL ${label} — ${error.message}`);
  if (config.verbose) {
    if (stderr) console.error(`  stderr:\n${stderr.trimEnd()}`);
    if (stdout) console.error(`  stdout:\n${stdout.trimEnd()}`);
  } else if (stderr) {
    console.error(`  stderr: ${stderr.split("\n").slice(-3).join(" | ")}`);
  }
}

// ---------------------------------------------------------------------------
// Expectations
// ---------------------------------------------------------------------------

function compareAgainstExpectations(perTestResults, expectations) {
  const regressions = [];
  const improvements = [];
  const newFails = [];
  let passCount = 0;
  let knownFails = 0;
  let skipCount = 0;

  for (const [name, status] of Object.entries(perTestResults || {})) {
    if (status === "skip" || status === "pending") { skipCount++; continue; }
    const exp = expectations[name];
    const expectedFail = exp?.status === "FAIL";

    if (status === "pass") {
      if (expectedFail) improvements.push(name);
      else passCount++;
    } else {
      if (exp === undefined) newFails.push(name);
      else if (expectedFail) knownFails++;
      else regressions.push(name);
    }
  }

  return { regressions, improvements, newFails, passCount, knownFails, skipCount };
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

function pad(v, n) { return (v + "").padStart(n); }

// Appended before every test file. process.nextTick fires after the test's
// synchronous code and emits the result line the runner parses.
// Must come BEFORE the test source so the callback is queued before any throw.
// process.nextTick is implemented natively in the Rust process module.
const RESULT_EMITTER = `
process.nextTick(function () {
  var out = { pass: 1, fail: 0, total: 1, errors: [], tests: { 'script passed': 'pass' } };
  console.log('NODE_TEST_RESULTS: ' + JSON.stringify(out));
});`;

const testList = loadTestList();
const shimSrc = testList.length > 0 ? readFileSync(rel("upstream-shim.js"), "utf-8") : "";

const targetLabel = config.target === "wasm" ? " [wasm]" : "";
console.log(`Running ${testList.length} node-compat test(s)${targetLabel}...\n`);
if (config.update) console.log("(--update: regenerating expectations)\n");

const tmpDir = mkdtempSync(path.join(process.cwd(), ".node-tmp-"));

let totalPass = 0;
let totalFail = 0;
let totalErrors = 0;

try {
  for (const testFile of testList) {
    const src = readFileSync(path.join(config.upstreamDir, testFile), "utf-8");
    const { results, error, stderr, stdout } = runTest(
      tmpDir,
      "upstream_" + testFile,
      shimSrc + RESULT_EMITTER + "\n// Upstream test source\n" + src
    );

    if (error) {
      reportCrash(testFile, error, stderr, stdout);
      totalErrors++;
      continue;
    }

    const perTestResults = results.tests ?? {};

    if (config.update) {
      const expFile = path.join(config.expectationsDir, testFile + ".json");
      mkdirSync(path.dirname(expFile), { recursive: true });
      const expData = {};
      for (const [name, status] of Object.entries(perTestResults)) {
        if (status !== "skip" && status !== "pending") {
          expData[name] = { status: status === "pass" ? "PASS" : "FAIL" };
        }
      }
      writeFileSync(expFile, JSON.stringify(expData, null, 2) + "\n");
      const total = results.pass + results.fail;
      console.log(`  ${testFile}  ${pad(results.pass, 3)}/${pad(total, 3)} pass  → wrote ${expFile}`);
      continue;
    }

    const expectations = loadExpectationsForFile(testFile);
    const { regressions, improvements, newFails, passCount, knownFails, skipCount } =
      compareAgainstExpectations(perTestResults, expectations);

    const effectiveTotal = passCount + knownFails + regressions.length + newFails.length;

    if (regressions.length > 0 || newFails.length > 0) {
      console.error(`FAIL ${testFile}  ${pad(passCount, 3)}/${pad(effectiveTotal, 3)}`);
      for (const name of regressions) {
        const detail = (results.errors ?? []).find((e) => e.startsWith(name + ":")) ?? name;
        console.error(`  ✗ [regression] ${detail}`);
      }
      for (const name of newFails) {
        console.error(`  ✗ [new fail]   ${name}`);
      }
      totalFail += regressions.length + newFails.length;
      totalPass += passCount;
    } else {
      let suffix = "";
      if (knownFails > 0) suffix += `  (${knownFails} known failure${knownFails > 1 ? "s" : ""})`;
      if (skipCount > 0) suffix += `  (${skipCount} skipped)`;
      if (improvements.length > 0) suffix += `  (${improvements.length} improved!)`;
      console.log(`ok   ${testFile}  ${pad(passCount, 3)}/${pad(effectiveTotal, 3)}${suffix}`);
      if (config.verbose) {
        for (const name of improvements) console.log(`  ✓ [improved] ${name}`);
      }
      totalPass += passCount;
    }
  }
} finally {
  rmSync(tmpDir, { recursive: true, force: true });
}

console.log(`\n${totalPass} passed, ${totalFail} failed, ${totalErrors} error(s) — ${testList.length} file(s)`);

if (totalFail > 0 || totalErrors > 0) process.exit(1);
