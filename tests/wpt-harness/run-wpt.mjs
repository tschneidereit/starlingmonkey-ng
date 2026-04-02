#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception
//
// WPT test runner for Starling-NG.
//
// Reads tests from tests.json, assembles a harness script for each test,
// runs it via the `starling` binary, and compares results against
// expectations files.
//
// Usage:
//   node tests/wpt-harness/run-wpt.mjs [options] [pattern]
//
// Options:
//   --wpt-root=PATH            Path to WPT checkout (default: deps/wpt)
//   --target=[native|wasm]     Execution target (default: native)
//   --runtime=PATH             Path to starling binary (default: target/debug/starling)
//   --expectations=PATH        Path to expectations dir (default: tests/wpt-harness/expectations)
//   --update-expectations      Update expectation files with current results
//   -v                         Verbose output
//   -vv                        Very verbose output
//   --help                     Show help

import { execFileSync, execFile } from "child_process";
import { existsSync, readFileSync, writeFileSync, mkdirSync, rmSync } from "fs";
import path from "path";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

function relativePath(p) {
  return new URL(p, import.meta.url).pathname;
}

const SKIP_PREFIX = "SKIP ";
const SLOW_PREFIX = "SLOW ";

const LogLevel = { Quiet: 0, Verbose: 1, VeryVerbose: 2 };

const config = {
  // Default automatically adjusted to "target/wasm32-wasip2/debug/starling.wasm" for wasm target.
  runtime: "target/debug/starling",
  target: "native",  // "native" or "wasm"
  wptRoot: relativePath("../../deps/wpt"),
  tests: {
    list: relativePath("tests.json"),
    expectations: relativePath("expectations"),
    updateExpectations: false,
    pattern: "",
  },
  skipSlowTests: false,
  logLevel: LogLevel.Quiet,
  timeout: 30000, // 30 second timeout per test
};

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

const ArgParsers = {
  "--runtime": {
    help: `Path to starling binary (default: ${config.runtime})`,
    cmd: val => { config.runtime = val; },
  },
  "--target": {
    help: `Execution target: native or wasm (default: native)`,
    cmd: val => {
      if (val !== "native" && val !== "wasm") {
        console.error(`Unknown --target value: ${val}. Use "native" or "wasm".`);
        process.exit(1);
      }
      config.target = val;
    },
  },
  "--wpt-root": {
    help: `Path to WPT checkout (default: ${config.wptRoot})`,
    cmd: val => { config.wptRoot = val; },
  },
  "--expectations": {
    help: `Path to expectations directory`,
    cmd: val => { config.tests.expectations = val; },
  },
  "--update-expectations": {
    help: "Update expectation files with current results",
    cmd: () => { config.tests.updateExpectations = true; },
  },
  "--skip-slow-tests": {
    help: "Skip tests marked as SLOW",
    cmd: () => { config.skipSlowTests = true; },
  },
  "--timeout": {
    help: `Timeout per test in ms (default: ${config.timeout})`,
    cmd: val => { config.timeout = parseInt(val, 10); },
  },
  "-v": {
    help: "Verbose output",
    cmd: () => { config.logLevel = LogLevel.Verbose; },
  },
  "-vv": {
    help: "Very verbose output",
    cmd: () => { config.logLevel = LogLevel.VeryVerbose; },
  },
  "--help": {
    help: "Show this help message",
    cmd: () => {
      console.log(`Usage: node run-wpt.mjs [options] [pattern]

If a pattern is provided, only tests whose path contains the pattern will be run.

Options:`);
      for (const [name, parser] of Object.entries(ArgParsers)) {
        console.log(`  ${(name + (parser.cmd.length > 0 ? "=value" : "")).padEnd(30)} ${parser.help}`);
      }
      process.exit(0);
    },
  },
};

function applyConfig(argv) {
  for (const entry of argv.slice(2)) {
    if (entry[0] !== "-") {
      config.tests.pattern = entry;
      continue;
    }
    const [arg, ...rest] = entry.split("=");
    const val = rest.join("=");
    const parser = ArgParsers[arg];
    if (parser) {
      parser.cmd(val || undefined);
    } else {
      console.error(`Unknown argument: ${arg}`);
      process.exit(1);
    }
  }

  // When targeting wasm, adjust the runtime path if not explicitly set.
  if (config.target === "wasm" && config.runtime === "target/debug/starling") {
    config.runtime = "target/wasm32-wasip2/debug/starling.wasm";
  }

  if (!existsSync(config.runtime)) {
    if (config.target === "wasm") {
      console.error(`Wasm runtime not found: ${config.runtime}. Run 'just build-wasm' first.`);
    } else {
      console.error(`Runtime not found: ${config.runtime}. Run 'cargo build' first.`);
    }
    return false;
  }

  if (config.target === "wasm") {
    // Verify wasmtime is available.
    try {
      execFileSync("wasmtime", ["--version"], { encoding: "utf-8" });
    } catch {
      console.error("wasmtime not found. Install wasmtime to run WPT tests on wasm.");
      return false;
    }
  }

  if (!existsSync(config.wptRoot)) {
    console.error(`WPT root not found: ${config.wptRoot}. Run 'just wpt-setup' first.`);
    return false;
  }

  return true;
}

// ---------------------------------------------------------------------------
// Harness assembly
// ---------------------------------------------------------------------------

// Cache the base harness (pre-harness + testharness.js + post-harness)
let cachedBaseHarness = null;

function getBaseHarness() {
  if (cachedBaseHarness) return cachedBaseHarness;

  const preHarness = readFileSync(relativePath("pre-harness.js"), "utf-8");
  const testHarness = readFileSync(
    path.join(config.wptRoot, "resources", "testharness.js"),
    "utf-8"
  );
  const postHarness = readFileSync(relativePath("post-harness.js"), "utf-8");

  cachedBaseHarness = preHarness + "\n" + testHarness + "\n" + postHarness + "\n";
  return cachedBaseHarness;
}

/**
 * Assemble the complete test script for a given WPT test path.
 *
 * The assembled script:
 * 1. Runs the base harness (pre-harness + testharness.js + post-harness)
 *    in the global scope (via legacy script mode).
 * 2. Uses evalScript() for each META: script= dependency.
 * 3. Uses evalScript() for the test source itself.
 * 4. Calls done() to trigger the completion callback.
 */

// Legacy WPT path aliases. The WPT HTTP server handles some redirects that
// aren't present when loading files directly from disk.
const WPT_PATH_ALIASES = {
  "/resources/WebIDLParser.js": "/resources/webidl2/lib/webidl2.js",
};

function assembleTestScript(testPath) {
  const fullPath = path.join(config.wptRoot, testPath);
  const testSource = readFileSync(fullPath, "utf-8");

  // Parse META: script= directives from the test source.
  const metaScripts = [];
  for (const match of testSource.matchAll(/\/\/ *META: *script=(.+)/g)) {
    metaScripts.push(match[1].trim());
  }

  let script = getBaseHarness();

  // If the test uses idl_test(), inject a minimal fetch polyfill that serves
  // IDL files from the WPT /interfaces/ directory on disk. This avoids
  // needing a real fetch() implementation or WPT HTTP server.
  const idlTestMatch = testSource.match(/idl_test\(\s*\[([^\]]*)\]/);
  if (idlTestMatch) {
    const idlSpecs = idlTestMatch[1]
      .split(",")
      .map(s => s.trim().replace(/^['"]|['"]$/g, ""))
      .filter(Boolean);

    // Pre-read all referenced IDL files and build a fetch polyfill.
    const idlMap = {};
    for (const spec of idlSpecs) {
      const idlPath = path.join(config.wptRoot, "interfaces", spec + ".idl");
      if (existsSync(idlPath)) {
        idlMap["/interfaces/" + spec + ".idl"] = readFileSync(idlPath, "utf-8");
      }
    }

    // Also check for dependency IDL specs (second argument to idl_test).
    const depsMatch = testSource.match(/idl_test\(\s*\[[^\]]*\]\s*,\s*\[([^\]]*)\]/);
    if (depsMatch) {
      const depSpecs = depsMatch[1]
        .split(",")
        .map(s => s.trim().replace(/^['"]|['"]$/g, ""))
        .filter(Boolean);
      for (const spec of depSpecs) {
        const idlPath = path.join(config.wptRoot, "interfaces", spec + ".idl");
        if (existsSync(idlPath)) {
          idlMap["/interfaces/" + spec + ".idl"] = readFileSync(idlPath, "utf-8");
        }
      }
    }

    if (Object.keys(idlMap).length > 0) {
      script += `// Minimal fetch polyfill for idl_test — serves pre-inlined IDL files.\n`;
      script += `globalThis.__wpt_idl_files = ${JSON.stringify(idlMap)};\n`;
      script += `globalThis.fetch = function(url) {\n`;
      script += `  var content = globalThis.__wpt_idl_files[url];\n`;
      script += `  if (content !== undefined) {\n`;
      script += `    return Promise.resolve({ ok: true, text: function() { return Promise.resolve(content); } });\n`;
      script += `  }\n`;
      script += `  return Promise.reject(new Error("fetch not available for: " + url));\n`;
      script += `};\n`;
    }
  }

  // Load META scripts via evalScript.
  for (const metaPath of metaScripts) {
    // Apply path aliases for legacy WPT paths.
    const effectivePath = WPT_PATH_ALIASES[metaPath] || metaPath;

    let resolvedPath;
    if (effectivePath.startsWith("/")) {
      // Absolute path within WPT root.
      resolvedPath = path.join(config.wptRoot, effectivePath);
    } else {
      // Relative to the test file.
      resolvedPath = path.join(path.dirname(fullPath), effectivePath);
    }
    if (!existsSync(resolvedPath)) {
      console.error(`  META script not found: ${metaPath} (resolved: ${resolvedPath})`);
      continue;
    }
    const metaSource = readFileSync(resolvedPath, "utf-8");
    script += toEvalScriptCall(metaSource, metaPath);
  }

  // Load the test source via evalScript.
  script += toEvalScriptCall(testSource, testPath);

  // Signal test completion.
  script += `done();\n`;

  return script;
}

function toEvalScriptCall(source, url) {
  let escaped = source.split("\\").join("\\\\");
  escaped = escaped.split("`").join("\\`");
  escaped = escaped.split("${").join("\\${");
  return `// ${url}\nevalScript(\`${escaped}\`, ${JSON.stringify(url)});\n\n`;
}

// ---------------------------------------------------------------------------
// Test execution
// ---------------------------------------------------------------------------

function getTests(pattern) {
  const raw = JSON.parse(readFileSync(config.tests.list, "utf-8"));
  let testPaths = raw.filter(p => !p.startsWith(SKIP_PREFIX));
  const totalCount = testPaths.length;

  if (config.skipSlowTests) {
    testPaths = testPaths.filter(p => !p.startsWith(SLOW_PREFIX));
  }

  testPaths = testPaths
    .map(p => (p.startsWith(SLOW_PREFIX) ? p.slice(SLOW_PREFIX.length) : p))
    .filter(p => p.includes(pattern));

  return { testPaths, totalCount };
}

function getExpectedResults(testPath) {
  const expectPath = path.join(config.tests.expectations, testPath + ".json");
  try {
    return JSON.parse(readFileSync(expectPath, "utf-8"));
  } catch {
    return {};
  }
}

function runSingleTest(testPath) {
  const script = assembleTestScript(testPath);

  // Write assembled script to a temp file.
  const tmpDir = path.join(config.wptRoot, "..", ".wpt-tmp");
  mkdirSync(tmpDir, { recursive: true });
  const tmpFile = path.join(tmpDir, "wpt-test.js");
  writeFileSync(tmpFile, script);

  try {
    let command, args;

    if (config.target === "wasm") {
      // Run via wasmtime with filesystem access to the temp directory and CWD.
      // The --dir=.::/ maps the host CWD to the WASI root /, so convert the
      // absolute host path to a WASI path relative to CWD.
      const wasiPath = "/" + path.relative(process.cwd(), tmpFile);
      command = "wasmtime";
      args = [
        "run",
        "--dir=.::/",
        config.runtime,
        "--legacy-script",
        "--wpt-mode",
        wasiPath,
      ];
    } else {
      command = config.runtime;
      args = [
        "--legacy-script",
        "--wpt-mode",
        tmpFile,
      ];
    }

    const stdout = execFileSync(command, args, {
      timeout: config.timeout,
      encoding: "utf-8",
      maxBuffer: 10 * 1024 * 1024,
    });

    // Parse results from stdout — look for the WPT_RESULTS_JSON marker.
    const lines = stdout.split("\n");
    for (const line of lines) {
      if (line.startsWith("Log: WPT_RESULTS_JSON:")) {
        const json = line.slice("Log: WPT_RESULTS_JSON:".length);
        return { results: JSON.parse(json), output: stdout };
      }
    }

    return { error: new Error("No WPT_RESULTS_JSON found in output"), output: stdout };
  } catch (e) {
    return { error: e, output: e.stdout || "" };
  }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

function formatStats(stats) {
  return `${pad(stats.pass, 4)} / ${pad(stats.count, 4)} (${pad("+" + stats.unexpectedPass, 5)}, ${pad("-" + stats.unexpectedFail, 5)}, ${pad("?" + stats.missing, 5)}) passing in ${pad(stats.duration, 4)}ms`;
}

function pad(v, n) {
  return (v + "").padStart(n);
}

async function run() {
  if (!applyConfig(process.argv)) {
    process.exit(1);
  }

  const { testPaths, totalCount } = getTests(config.tests.pattern);
  const pathLength = testPaths.reduce((len, p) => Math.max(p.length, len), 0);

  console.log(`Running ${testPaths.length} of ${totalCount} tests ...\n`);

  let expectationsUpdated = 0;
  let unexpectedFailure = false;

  const totalStats = {
    duration: 0,
    count: 0,
    pass: 0,
    missing: 0,
    unexpectedPass: 0,
    unexpectedFail: 0,
  };

  for (const testPath of testPaths) {
    if (config.logLevel >= LogLevel.Verbose) {
      console.log(`Running test ${testPath}`);
    }

    const expectations = getExpectedResults(testPath);
    const t1 = Date.now();
    const { results, error, output } = runSingleTest(testPath);
    const duration = Date.now() - t1;

    const stats = {
      count: 0,
      pass: 0,
      missing: 0,
      unexpectedPass: 0,
      unexpectedFail: 0,
      duration,
    };

    if (error) {
      const expectPath = path.join(config.tests.expectations, testPath + ".json");
      const hasExpectations = existsSync(expectPath);

      if (hasExpectations) {
        console.log(`UNEXPECTED ERROR: ${testPath} (${duration}ms)`);
        console.log(`  MESSAGE: ${error.message}`);
        if (config.logLevel >= LogLevel.Verbose && output) {
          console.log(`  OUTPUT: ${output.slice(0, 500)}`);
        }
        if (config.tests.updateExpectations) {
          console.log(`  Removing expectations file ${expectPath}`);
          rmSync(expectPath);
          expectationsUpdated++;
        } else {
          unexpectedFailure = true;
        }
      } else {
        console.log(`EXPECTED ERROR: ${testPath} (${duration}ms)`);
      }

      totalStats.duration += duration;
      totalStats.missing += Math.max(Object.keys(expectations).length, 1);
      continue;
    }

    for (const result of results) {
      stats.count++;

      const expectation = expectations[result.name];
      if (expectation) {
        expectation.did_run = true;
      }

      if (result.status === 0) {
        stats.pass++;
        if (!expectation || expectation.status === "FAIL") {
          console.log(`${expectation ? "UNEXPECTED" : "NEW"} PASS\n  NAME: ${result.name}`);
          stats.unexpectedPass++;
        }
      } else if (!expectation || expectation.status === "PASS") {
        console.log(
          `${expectation ? "UNEXPECTED" : "NEW"} FAIL\n  NAME: ${result.name}\n  MESSAGE: ${result.message}`
        );
        stats.unexpectedFail++;
      }
    }

    for (const [name, expectation] of Object.entries(expectations)) {
      if (!expectation.did_run) {
        stats.missing++;
        console.log(`MISSING TEST\n  NAME: ${name}\n  EXPECTED: ${expectation.status}`);
      }
    }

    totalStats.count += stats.count;
    totalStats.pass += stats.pass;
    totalStats.missing += stats.missing;
    totalStats.unexpectedPass += stats.unexpectedPass;
    totalStats.unexpectedFail += stats.unexpectedFail;
    totalStats.duration += stats.duration;

    console.log(`${testPath.padEnd(pathLength)} ${formatStats(stats)}`);

    if (config.tests.updateExpectations && (stats.unexpectedFail + stats.unexpectedPass + stats.missing > 0)) {
      const expectPath = path.join(config.tests.expectations, testPath + ".json");
      console.log(`  Writing expectations to ${expectPath}`);
      const newExpectations = {};
      for (const result of results) {
        newExpectations[result.name] = {
          status: result.status === 0 ? "PASS" : "FAIL",
        };
      }
      mkdirSync(path.dirname(expectPath), { recursive: true });
      writeFileSync(expectPath, JSON.stringify(newExpectations, null, 2) + "\n");
      expectationsUpdated++;
    }
  }

  console.log(`\n${"Done. Stats:".padEnd(pathLength)} ${formatStats(totalStats)}`);

  if (config.tests.updateExpectations) {
    console.log(`Expectations updated: ${expectationsUpdated}`);
  } else if (totalStats.unexpectedFail + totalStats.unexpectedPass + totalStats.missing > 0 || unexpectedFailure) {
    console.error("\nUnexpected results. Run with --update-expectations to update.");
    process.exit(1);
  }
}

run();
