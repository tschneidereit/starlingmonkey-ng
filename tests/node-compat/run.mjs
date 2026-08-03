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
//   -v                      Verbose: print stderr on failure

import { execFileSync, spawnSync } from "child_process";
import { existsSync, readFileSync, writeFileSync, mkdtempSync, rmSync } from "fs";
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
  testDir: rel("node-test/test"),
  pattern: "",
  verbose: false,
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

const WASM_SKIP = new Set([
  "parallel/test-fs-symlink-dir-junction-relative.js",
  "parallel/test-fs-symlink-longpath.js",
  "parallel/test-timers-args.js",
]);

function loadTestList() {
  const file = rel("upstream-tests.json");
  if (!existsSync(file)) return [];
  let list = JSON.parse(readFileSync(file, "utf-8"));
  if (config.target === "wasm") list = list.filter((f) => !WASM_SKIP.has(f));
  return config.pattern ? list.filter((f) => f.includes(config.pattern)) : list;
}

// ---------------------------------------------------------------------------
// Test execution
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

function parseResult(stdout) {
  for (const line of stdout.split("\n")) {
    const s = line.replace(/^Log: */, "");
    if (s.startsWith("NODE_TEST_RESULT:")) {
      return JSON.parse(s.slice("NODE_TEST_RESULT:".length));
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// Result emitter (injected JS)
// ---------------------------------------------------------------------------

// Appended after the shim and before the test source. Fires on the 'exit'
// event so it runs AFTER all timers and async callbacks have completed.
//
// __syncError   — set by the try/catch wrapper around the test source
// __asyncError  — set by timer callback wrappers in upstream-shim.js
// __mustCallCheckCounters — populated by common.mustCall() in the shim
// __unknownRequires       — populated when require() hits unimplemented modules
const RESULT_EMITTER = `
var __syncError = null;
process.nextTick(function() {});
process.on('exit', function () {
  var mustCallErr = null;
  if (typeof __mustCallCheckCounters !== 'undefined') {
    for (var i = 0; i < __mustCallCheckCounters.length; i++) {
      var ctx = __mustCallCheckCounters[i];
      if (ctx.actual !== ctx.exact) {
        mustCallErr = 'mustCall(' + ctx.name + '): expected ' + ctx.exact + ' calls, got ' + ctx.actual;
        break;
      }
    }
  }
  var err = __syncError || (typeof __asyncError !== 'undefined' ? __asyncError : null) || mustCallErr;
  if (err) {
    console.log('NODE_TEST_RESULT: ' + JSON.stringify({ status: 'fail', error: String(err) }));
    return;
  }
  if (typeof __unknownRequires !== 'undefined' && __unknownRequires.length > 0) {
    console.log('NODE_TEST_RESULT: ' + JSON.stringify({ status: 'skip', modules: __unknownRequires }));
    return;
  }
  console.log('NODE_TEST_RESULT: ' + JSON.stringify({ status: 'pass' }));
});`;

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

const testList = loadTestList();
const shimSrc = testList.length > 0 ? readFileSync(rel("upstream-shim.js"), "utf-8") : "";

const targetLabel = config.target === "wasm" ? " [wasm]" : "";
console.log(`Running ${testList.length} node-compat test(s)${targetLabel}...\n`);

const tmpDir = mkdtempSync(path.join(process.cwd(), ".node-tmp-"));

let passed = 0;
let failed = 0;
let skipped = 0;
let errors = 0;

try {
  for (const testFile of testList) {
    const src = readFileSync(path.join(config.testDir, testFile), "utf-8");
    const tmp = path.join(tmpDir, testFile.replace(/\//g, "_"));
    writeFileSync(tmp, shimSrc + RESULT_EMITTER + "\ntry {\n" + src + "\n} catch (__e) { __syncError = __e; }");

    const { stdout = "", stderr = "", error, status } = spawnTest(tmp);
    const result = parseResult(stdout);

    if (!result) {
      console.error(`CRASH ${testFile} — ${error?.message ?? `exit ${status}`}`);
      if (config.verbose && stderr) console.error(`  ${stderr.trimEnd()}`);
      errors++;
      continue;
    }

    if (result.status === "pass") {
      console.log(`ok   ${testFile}`);
      passed++;
    } else if (result.status === "skip") {
      console.log(`skip ${testFile}  (${result.modules.join(", ")})`);
      skipped++;
    } else {
      console.error(`FAIL ${testFile}  ${result.error}`);
      if (config.verbose && stderr) console.error(`  ${stderr.trimEnd()}`);
      failed++;
    }
  }
} finally {
  rmSync(tmpDir, { recursive: true, force: true });
}

console.log(`\n${passed} passed, ${failed} failed, ${skipped} skipped, ${errors} error(s) — ${testList.length} file(s)`);

if (failed > 0 || errors > 0) process.exit(1);
