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

import { execFileSync, spawn, spawnSync } from "child_process";
import {
  copyFileSync,
  existsSync,
  readFileSync,
  writeFileSync,
  mkdirSync,
  rmSync,
  statSync,
} from "fs";
import path from "path";
import { cpus } from "os";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

function relativePath(p) {
  return new URL(p, import.meta.url).pathname;
}

// tests.json line prefixes:
//   "SKIP "      — never run, on any target.
//   "SLOW "      — skipped when --skip-slow-tests is passed.
//   "SKIP-WASM " — run on native, skipped on the wasm target. For tests that
//                  depend on JS APIs the wasm build does not expose (e.g.
//                  `WebAssembly`, which the SpiderMonkey-in-wasm build omits).
//   "SKIP-NATIVE " — run on wasm, skipped on the native target. The mirror of
//                  SKIP-WASM, for tests only the wasm host can run at all.
//   "NET "       — needs the live WPT server; see NET_PREFIX below.
//
// Reach for SKIP-WASM/SKIP-NATIVE only when a test cannot run on that target at
// all. When it runs but some subtests behave differently — the two HTTP stacks
// do not have identical capabilities — record that per subtest with
// `wasm_status` instead (see getExpectedResults), so the subtests that *do*
// pass stay covered on both targets.
const SKIP_PREFIX = "SKIP";
const SLOW_PREFIX = "SLOW";
const SKIP_WASM_PREFIX = "SKIP-WASM";
const SKIP_NATIVE_PREFIX = "SKIP-NATIVE";
// Tests that need the live WPT server (they call `fetch()` over the network).
// When any selected test is `NET`, the harness ensures `wpt serve` is running.
const NET_PREFIX = "NET";

const LogLevel = { Quiet: 0, Verbose: 1, VeryVerbose: 2 };

const config = {
  // Default automatically adjusted to "target/wasm32-wasip2/debug/starling.wasm" for wasm target.
  runtime: "target/debug/starling",
  target: "native", // "native" or "wasm"
  wptRoot: process.env.WPT_ROOT || relativePath("../../deps/wpt"),
  tmpDir: relativePath("../../deps/.wpt-tmp"),
  tests: {
    list: relativePath("tests.json"),
    expectations: relativePath("expectations"),
    updateExpectations: false,
    pattern: "",
  },
  skipSlowTests: false,
  // How many tests to have in flight at once. Defaults to number of CPUs * 2,
  // because many tests aren't CPU-bound, and can execute in parallel without
  // incurring compute contention.
  jobs: Math.max(1, cpus().length * 2),
  // Path to an ahead-of-time compiled runtime, set up for the wasm target
  // unless --no-precompile is passed. See ensurePrecompiledRuntime.
  precompiled: null,
  usePrecompiled: true,
  logLevel: LogLevel.Quiet,
  timeout: 30000, // 30 second timeout per test
};

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

const ArgParsers = {
  "--runtime": {
    help: `Path to starling binary (default: ${config.runtime})`,
    cmd: (val) => {
      config.runtime = val;
    },
  },
  "--target": {
    help: `Execution target: native or wasm (default: native)`,
    cmd: (val) => {
      if (val !== "native" && val !== "wasm") {
        console.error(
          `Unknown --target value: ${val}. Use "native" or "wasm".`,
        );
        process.exit(1);
      }
      config.target = val;
    },
  },
  "--wpt-root": {
    help: `Path to WPT checkout (default: env var 'WPT_ROOT' or ${config.wptRoot})`,
    cmd: (val) => {
      config.wptRoot = val;
    },
  },
  "--expectations": {
    help: `Path to expectations directory`,
    cmd: (val) => {
      config.tests.expectations = val;
    },
  },
  "--update-expectations": {
    help: "Update expectation files with current results",
    cmd: () => {
      config.tests.updateExpectations = true;
    },
  },
  "--skip-slow-tests": {
    help: "Skip tests marked as SLOW",
    cmd: () => {
      config.skipSlowTests = true;
    },
  },
  "--jobs": {
    help: `Tests to run concurrently (default: ${config.jobs})`,
    cmd: (val) => {
      const jobs = parseInt(val, 10);
      if (!Number.isInteger(jobs) || jobs < 1) {
        throw new Error(`--jobs must be a positive integer, got: ${val}`);
      }
      config.jobs = jobs;
    },
  },
  "--no-precompile": {
    help: "Skip ahead-of-time compilation of the wasm runtime",
    cmd: () => {
      config.usePrecompiled = false;
    },
  },
  "--timeout": {
    help: `Timeout per test in ms for a serial run, scaled by --jobs (default: ${config.timeout})`,
    cmd: (val) => {
      config.timeout = parseInt(val, 10);
    },
  },
  "-v": {
    help: "Verbose output",
    cmd: () => {
      config.logLevel = LogLevel.Verbose;
    },
  },
  "-vv": {
    help: "Very verbose output",
    cmd: () => {
      config.logLevel = LogLevel.VeryVerbose;
    },
  },
  "--help": {
    help: "Show this help message",
    cmd: () => {
      console.log(`Usage: node run-wpt.mjs [options] [pattern]

If a pattern is provided, only tests whose path contains the pattern will be run.

Options:`);
      for (const [name, parser] of Object.entries(ArgParsers)) {
        console.log(
          `  ${(name + (parser.cmd.length > 0 ? "=value" : "")).padEnd(30)} ${parser.help}`,
        );
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
      console.error(
        `Wasm runtime not found: ${config.runtime}. Run 'just build-wasm' first.`,
      );
    } else {
      console.error(
        `Runtime not found: ${config.runtime}. Run 'cargo build' first.`,
      );
    }
    return false;
  }

  if (config.target === "wasm") {
    // Verify wasmtime is available.
    try {
      execFileSync("wasmtime", ["--version"], { encoding: "utf-8" });
    } catch {
      console.error(
        "wasmtime not found. Install wasmtime to run WPT tests on wasm.",
      );
      return false;
    }
  }

  if (!existsSync(config.wptRoot)) {
    console.error(
      `WPT root not found: ${config.wptRoot}. Run 'just clone-wpt-tests' first, define 'WPT_ROOT' in your environment, or pass '--wpt-root'.`,
    );
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
    "utf-8",
  );
  const postHarness = readFileSync(relativePath("post-harness.js"), "utf-8");

  cachedBaseHarness =
    preHarness + "\n" + testHarness + "\n" + postHarness + "\n";
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

  // Point `globalThis.location` at the test's canonical WPT URL before any
  // harness or test code runs.
  let script = `__setLocation(${JSON.stringify(
    "http://web-platform.test:8000/" + testPath,
  )});\n`;
  script += getBaseHarness();

  // If the test uses idl_test(), inject a minimal fetch polyfill that serves
  // IDL files from the WPT /interfaces/ directory on disk. This avoids
  // needing a real fetch() implementation or WPT HTTP server.
  const idlTestMatch = testSource.match(/idl_test\(\s*\[([^\]]*)\]/);
  if (idlTestMatch) {
    const idlSpecs = idlTestMatch[1]
      .split(",")
      .map((s) => s.trim().replace(/^['"]|['"]$/g, ""))
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
    const depsMatch = testSource.match(
      /idl_test\(\s*\[[^\]]*\]\s*,\s*\[([^\]]*)\]/,
    );
    if (depsMatch) {
      const depSpecs = depsMatch[1]
        .split(",")
        .map((s) => s.trim().replace(/^['"]|['"]$/g, ""))
        .filter(Boolean);
      for (const spec of depSpecs) {
        const idlPath = path.join(config.wptRoot, "interfaces", spec + ".idl");
        if (existsSync(idlPath)) {
          idlMap["/interfaces/" + spec + ".idl"] = readFileSync(
            idlPath,
            "utf-8",
          );
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
      console.error(
        `  META script not found: ${metaPath} (resolved: ${resolvedPath})`,
      );
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

/**
 * Extract and strip all prefixes from `path`, returning the set of prefixes and the stripped path.
 *
 * Prefixes have the format "PREFIX[(comment)]", and can be repeated.
 * The returned prefixes map will contain the prefix as the key and the optional comment as the value (`undefined` if no comment).
 */
function extractPrefixes(path) {
  const prefixes = new Map();
  let remaining = path;
  while (true) {
    const match = remaining.match(/^([A-Z-]+)(\(([^)]+)\))?[, ]/);
    if (!match) break;
    const prefix = match[1];
    const comment = match[3];
    prefixes.set(prefix, comment);
    remaining = remaining.slice(match[0].length);
  }
  return { prefixes, path: remaining };
}

function getTests(pattern) {
  const raw = JSON.parse(readFileSync(config.tests.list, "utf-8"));
  const totalCount = raw.length;
  let testPaths = [];
  let needsServer = false;
   for (const rawPath of raw) {
    const { prefixes, path } = extractPrefixes(rawPath);
    if (!path.includes(pattern) || prefixes.has(SKIP_PREFIX) ||
      (config.target === "wasm" && prefixes.has(SKIP_WASM_PREFIX)) ||
      (config.target === "native" && prefixes.has(SKIP_NATIVE_PREFIX)) ||
      (config.skipSlowTests && prefixes.has(SLOW_PREFIX))) {
      continue;
    }
    needsServer ||= prefixes.has(NET_PREFIX);
    testPaths.push(path);
  }

  return { testPaths, totalCount, needsServer };
}

function expectationsPath(testPath) {
  return path.join(config.tests.expectations, testPath + ".json");
}

// The expectation field holding the wasm target's status, when it differs from
// native. Absent means "same as native".
const WASM_STATUS = "wasm_status";

/// Read the raw expectations file, exactly as stored.
function readExpectationsFile(testPath) {
  try {
    return JSON.parse(readFileSync(expectationsPath(testPath), "utf-8"));
  } catch {
    return {};
  }
}

// The expectations for the target being run, from one file shared by both.
//
// A subtest is stored as `{"status": "PASS"}` when both targets agree, and
// gains a `wasm_status` when they do not:
//
//     "some subtest": { "status": "PASS", "wasm_status": "FAIL" }
//
// `status` is the native status and the default; `wasm_status` overrides it on
// wasm. Keeping both in one file means a subtest that behaves the same
// everywhere — the overwhelming majority — is still written once.
//
// An entry with no status for this target (a `wasm_status`-only entry seen on
// native) is dropped, so the subtest reads as having no expectation rather than
// as an expectation that can never be met.
function getExpectedResults(testPath) {
  const raw = readExpectationsFile(testPath);
  const expectations = {};
  for (const [name, entry] of Object.entries(raw)) {
    const status =
      config.target === "wasm" && entry[WASM_STATUS] !== undefined
        ? entry[WASM_STATUS]
        : entry.status;
    if (status !== undefined) {
      expectations[name] = { status };
    }
  }
  return expectations;
}

// Fold this run's results into the stored expectations, touching only the
// target that ran. Running on wasm must not overwrite the native statuses, and
// vice versa, so that updating one target never silently invents results for
// the other.
function mergeExpectations(previous, results) {
  const merged = {};
  for (const result of results) {
    const prev = previous[result.name] ?? {};
    const observed = result.status === 0 ? "PASS" : "FAIL";
    // Preserve a FLAKY marker: it records a deliberate known-intermittent
    // subtest, not the outcome of any single run.
    const keep = (stored) => (stored === "FLAKY" ? "FLAKY" : undefined);

    if (config.target === "wasm") {
      const native = prev.status;
      const wasm = keep(prev[WASM_STATUS]) ?? observed;
      const entry = native === undefined ? {} : { status: native };
      // Only record a wasm status where it actually differs from native, so the
      // file does not fill up with redundant duplicates.
      if (wasm !== native) entry[WASM_STATUS] = wasm;
      merged[result.name] = entry;
    } else {
      const entry = { status: keep(prev.status) ?? observed };
      if (prev[WASM_STATUS] !== undefined && prev[WASM_STATUS] !== entry.status) {
        entry[WASM_STATUS] = prev[WASM_STATUS];
      }
      merged[result.name] = entry;
    }
  }
  // Carry over entries for the *other* target that this run did not observe, so
  // updating one target never drops the other's record.
  for (const [name, entry] of Object.entries(previous)) {
    if (merged[name]) continue;
    if (config.target === "wasm" && entry.status !== undefined) {
      merged[name] = { status: entry.status };
    } else if (config.target === "native" && entry[WASM_STATUS] !== undefined) {
      merged[name] = { [WASM_STATUS]: entry[WASM_STATUS] };
    }
  }
  return merged;
}

// Codegen options passed to both `wasmtime compile` and `wasmtime run`; a
// precompiled artifact is only loadable by a runtime configured the same way.
//
// Native unwind information is off because registering and — far worse —
// deregistering it dominates every wasm test. A sample of the wasmtime process
// during a trivial run puts essentially all of the ~1.1s of host time in
// `CodeMemory::drop` calling `__deregister_frame`, whose macOS implementation
// walks the registered frames linearly; with a module this size that is
// quadratic and swamps everything else. The runtime module is ~120MB with a
// correspondingly huge function count, so it hits this hard: dropping the
// unwind tables takes a wasm test from ~1.4s to ~0.35s.
//
// What it costs: native profilers and debuggers can no longer unwind through
// wasm frames. Guest-level diagnostics are unaffected — JS exceptions and their
// stacks come from SpiderMonkey, and wasm traps from wasmtime's own frame
// information — so error reporting in the harness is unchanged.
const WASMTIME_CODEGEN_FLAGS = ["-C", "native-unwind-info=n"];

// Compile the wasm runtime ahead of time, once, and run the tests against the
// result.
//
// Roughly a fifth of each wasm test's wall clock is wasmtime compiling the
// runtime module — it is ~120 MB, and the compilation is not cached between
// processes, so every test paid for it again. Compiling once up front and
// passing `--allow-precompiled` moves that cost from per-test to per-run.
//
// The artifact is tied to both the runtime build and the wasmtime version, so
// it is rebuilt whenever either is newer than it. On any failure we fall back
// to running the module directly rather than failing the run.
function ensurePrecompiledRuntime() {
  if (config.target !== "wasm" || !config.usePrecompiled) return null;

  const output = path.join(config.tmpDir, path.basename(config.runtime) + ".cwasm");
  let version = "";
  try {
    version = execFileSync("wasmtime", ["--version"], { encoding: "utf-8" }).trim();
  } catch {
    return null;
  }
  const stamp = output + ".stamp";
  const want = `${version}\n${statSync(config.runtime).mtimeMs}\n${WASMTIME_CODEGEN_FLAGS.join(" ")}`;
  try {
    if (readFileSync(stamp, "utf-8") === want && existsSync(output)) {
      return output;
    }
  } catch {
    // No usable stamp; compile below.
  }

  console.log("Precompiling the wasm runtime (once for this run) ...");
  try {
    mkdirSync(config.tmpDir, { recursive: true });
    execFileSync(
      "wasmtime",
      [
        "compile",
        ...WASMTIME_CODEGEN_FLAGS,
        "-W",
        "component-model-async=y",
        config.runtime,
        "-o",
        output,
      ],
      { stdio: "pipe" },
    );
    writeFileSync(stamp, want);
    return output;
  } catch (e) {
    console.warn(`  Precompilation failed, running uncompiled: ${e.message}`);
    return null;
  }
}

/// Spawn `command`, collecting stdout/stderr, with the same timeout and
/// output-cap semantics `spawnSync` gave us — but without blocking the loop, so
/// several tests can be in flight at once.
function spawnCollecting(command, args, { timeout, maxBuffer }) {
  return new Promise((resolve) => {
    let child;
    try {
      child = spawn(command, args);
    } catch (e) {
      resolve({ error: e, stdout: "", stderr: "" });
      return;
    }
    // Collect raw chunks and decode once at the end. Decoding each chunk as it
    // arrives corrupts any multi-byte UTF-8 sequence that straddles a chunk
    // boundary, turning it into replacement characters — which shows up as a
    // subtest whose name no longer matches its expectation, and only
    // intermittently, since where the boundaries fall depends on timing.
    const stdoutChunks = [];
    const stderrChunks = [];
    let stdoutBytes = 0;
    let error = null;
    const kill = (e) => {
      if (error) return;
      error = e;
      child.kill("SIGKILL");
    };
    const timer = setTimeout(
      () => kill(Object.assign(new Error("ETIMEDOUT"), { code: "ETIMEDOUT" })),
      timeout,
    );
    child.stdout.on("data", (d) => {
      stdoutChunks.push(d);
      stdoutBytes += d.length;
      if (stdoutBytes > maxBuffer) kill(new Error("stdout exceeded maxBuffer"));
    });
    child.stderr.on("data", (d) => {
      stderrChunks.push(d);
    });
    const decode = () => ({
      stdout: Buffer.concat(stdoutChunks).toString("utf-8"),
      stderr: Buffer.concat(stderrChunks).toString("utf-8"),
    });
    child.on("error", (e) => {
      clearTimeout(timer);
      resolve({ error: error ?? e, ...decode() });
    });
    child.on("close", (status, signal) => {
      clearTimeout(timer);
      resolve({ error, status, signal, ...decode() });
    });
  });
}

// Where this run writes `testPath`'s assembled script.
function runScriptPath(testPath) {
  return path.join(
    config.tmpDir,
    `${config.target}-${process.pid}-${testPath.replace(/\//g, "_")}`,
  );
}

// Build the command that runs `scriptFile`.
//
// `optimized` selects the flags the harness runs with — ahead-of-time compiled
// runtime, no native unwind information.
function testCommand(scriptFile, { optimized }) {
  if (config.target !== "wasm") {
    return {
      command: config.runtime,
      args: ["--legacy-script", "--wpt-mode", scriptFile],
    };
  }
  // wasmtime sees the guest filesystem, where --dir=.::/ maps the host CWD to
  // /, so the script has to be named by its path relative to the CWD. A host
  // path fails with "Error reading script".
  const wasiPath = "/" + path.relative(process.cwd(), scriptFile);
  const precompiled = optimized ? config.precompiled : null;
  return {
    command: "wasmtime",
    args: [
      "run",
      ...(optimized ? WASMTIME_CODEGEN_FLAGS : []),
      ...(precompiled ? ["--allow-precompiled"] : []),
      "--dir=.::/",
      "--dir=.",
      "--dir=/tmp",
      "-Sinherit-env=y,inherit-network=y,http=y,tcp=y,udp=y,p3=y",
      "-Wcomponent-model-async=y",
      precompiled ?? config.runtime,
      "--legacy-script",
      "--wpt-mode",
      wasiPath,
    ],
  };
}

async function runSingleTest(testPath) {
  const script = assembleTestScript(testPath);

  // Write the assembled script to a temp file of its own.
  //
  // The name carries the test, the target and this process's pid. A single shared name breaks
  // in two ways: two runs going at once (a wasm suite and a native spot-check, say) overwrite
  // each other's script between the write and the spawn, and with several tests in flight
  // within one run they do the same to each other. Either way the results of whichever script
  // won get reported under another test's name — silently, and looking exactly like a genuine
  // per-target difference.
  mkdirSync(config.tmpDir, { recursive: true });
  const tmpFile = runScriptPath(testPath);
  writeFileSync(tmpFile, script);

  const { command, args } = testCommand(tmpFile, { optimized: true });
  try {

    const result = await spawnCollecting(command, args, {
      // A test's wall clock includes time spent waiting for a core, so with N
      // tests in flight the budget has to grow with N or the slowest legitimate
      // tests start timing out purely from contention. `--timeout` is therefore
      // the budget for a serial run; the limit scales from there. The point of
      // the timeout is to catch a hang, which no amount of contention explains.
      timeout: config.timeout * Math.max(1, config.jobs),
      maxBuffer: 10 * 1024 * 1024,
    });

    const stdout = result.stdout || "";
    const stderr = result.stderr || "";
    let output = stdout;
    if (stderr) {
      output += "\n--- stderr ---\n" + stderr;
    }

    if (result.error) {
      return { error: result.error, output };
    }
    if (result.status !== 0 || result.signal) {
      const reason = result.signal
        ? `killed by signal ${result.signal}`
        : `exited with status ${result.status}`;
      return { error: new Error(reason), output };
    }

    // Parse results from stdout — look for the WPT_RESULTS_JSON marker.
    const lines = stdout.split("\n");
    for (const line of lines) {
      if (line.startsWith("Log: WPT_RESULTS_JSON:")) {
        const json = line.slice("Log: WPT_RESULTS_JSON:".length);
        return { results: JSON.parse(json), output };
      }
    }

    return { error: new Error("No WPT_RESULTS_JSON found in output"), output };
  } catch (e) {
    return { error: e, output: e.stdout || "" };
  }
}

// Run `testPaths` with up to `config.jobs` in flight, handing each result to
// `report` strictly in test order.
//
// Ordering matters more than it looks: `report` prints, accumulates the totals
// and rewrites expectation files, so letting it run in completion order would
// make both the output and `--update-expectations` depend on which test
// happened to finish first. Only the spawning is concurrent; everything that
// has an effect stays sequential and deterministic.
async function runTestsConcurrently(testPaths, report) {
  const jobs = Math.max(1, config.jobs);
  const outcomes = new Array(testPaths.length);
  const running = new Set();
  let nextToLaunch = 0;
  let nextToReport = 0;

  while (nextToReport < testPaths.length) {
    while (running.size < jobs && nextToLaunch < testPaths.length) {
      const index = nextToLaunch++;
      const task = (async () => {
        const started = Date.now();
        const outcome = await runSingleTest(testPaths[index]);
        outcomes[index] = { ...outcome, duration: Date.now() - started };
      })();
      running.add(task);
      task.finally(
        () => running.delete(task),
      );
    }

    if (outcomes[nextToReport] === undefined) {
      // Nothing to report yet; wait for whichever test finishes next.
      await Promise.race(running);
      continue;
    }

    await report(testPaths[nextToReport], outcomes[nextToReport]);
    // Release the captured output; a full suite would otherwise hold every
    // test's stdout until the end.
    outcomes[nextToReport] = null;
    nextToReport++;
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

// ---------------------------------------------------------------------------
// WPT server (for network `fetch()` tests)
// ---------------------------------------------------------------------------

/** A `wpt serve` instance this process started, so it can be stopped on exit. */
let managedWptServer = null;

function isWptServerReachable() {
  try {
    execFileSync(
      "curl",
      ["-s", "-o", "/dev/null", "--max-time", "3", "http://web-platform.test:8000/"],
      { stdio: "ignore" },
    );
    return true;
  } catch {
    return false;
  }
}

/**
 * Ensure the WPT server is running on `web-platform.test:8000`. If one is already
 * reachable (started externally), use it. Otherwise start `wpt serve` and wait
 * for it; it is stopped on process exit. The `web-platform.test` hosts entries
 * must already be present (`just wpt-setup` installs them from `deps/wpt-hosts`).
 */
function ensureWptServer() {
  if (isWptServerReachable()) {
    console.log("Using already-running WPT server at http://web-platform.test:8000/");
    return;
  }
  console.log("Starting WPT server (wpt serve --no-h2) ...");
  managedWptServer = spawn(path.join(config.wptRoot, "wpt"), ["serve", "--no-h2"], {
    cwd: config.wptRoot,
    stdio: "ignore",
    detached: true,
  });
  for (let i = 0; i < 90; i++) {
    if (isWptServerReachable()) {
      console.log("WPT server is ready.");
      return;
    }
    spawnSync("sleep", ["1"]);
  }
  stopWptServer();
  console.error(
    "WPT server did not become reachable. Ensure the hosts entries exist ('just wpt-setup').",
  );
  process.exit(1);
}

function stopWptServer() {
  if (managedWptServer) {
    try {
      // Kill the whole process group: `wpt serve` spawns per-protocol children.
      process.kill(-managedWptServer.pid);
    } catch {
      // Already gone.
    }
    managedWptServer = null;
  }
}

process.on("exit", stopWptServer);

async function run() {
  if (!applyConfig(process.argv)) {
    process.exit(1);
  }

  const { testPaths, totalCount, needsServer } = getTests(config.tests.pattern);

  if (needsServer) {
    ensureWptServer();
  }
  const pathLength = testPaths.reduce((len, p) => Math.max(p.length, len), 0);

  config.precompiled = ensurePrecompiledRuntime();
  const suiteStart = Date.now();

  const concurrency = config.jobs > 1 ? `, ${config.jobs} at a time` : "";
  console.log(
    `Running ${testPaths.length} of ${totalCount} tests${concurrency} ...\n`,
  );

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

  await runTestsConcurrently(testPaths, (testPath, outcome) => {
    if (config.logLevel >= LogLevel.Verbose) {
      console.log(`Running test ${testPath}`);
    }

    const expectations = getExpectedResults(testPath);
    const { results, error, output, duration } = outcome;

    const stats = {
      count: 0,
      pass: 0,
      missing: 0,
      unexpectedPass: 0,
      unexpectedFail: 0,
      duration,
    };

    if (error) {
      const expectPath = expectationsPath(testPath);
      const hasExpectations = existsSync(expectPath);

      if (hasExpectations) {
        console.log(`UNEXPECTED ERROR: ${testPath} (${duration}ms)`);
        console.log(`  MESSAGE: ${error.message}`);
        if (output) {
          // Show the tail rather than the head: WPT_RESULTS_JSON would be at the
          // end on success, and crash diagnostics from stderr are appended last.
          const limit = config.logLevel >= LogLevel.Verbose ? 120 : 30;
          const lines = output.trim().split("\n");
          const tail =
            lines.length > limit ? lines.slice(-limit).join("\n") : output;
          const prefix =
            lines.length > limit ? `... (last ${limit} lines)\n` : "";
          console.log(`  OUTPUT:\n${prefix}${tail}`);
        }
        if (config.tests.updateExpectations) {
          console.log(`  Removing expectations file ${expectPath}`);
          rmSync(expectPath);
          expectationsUpdated++;
        } else {
          unexpectedFailure = true;
          printSTR(testPath);
        }
      } else {
        console.log(`EXPECTED ERROR: ${testPath} (${duration}ms)`);
      }

      totalStats.duration += duration;
      totalStats.missing += Object.keys(expectations).length;
      return;
    }

    for (const result of results) {
      stats.count++;

      const expectation = expectations[result.name];
      if (expectation) {
        expectation.did_run = true;
      }

      if (result.status === 0) {
        stats.pass++;
      }

      // A FLAKY expectation marks a known-intermittent subtest (e.g. one that
      // depends on connection-reuse timing against a non-compliant server):
      // both outcomes are tolerated, so neither is reported as unexpected.
      if (expectation && expectation.status === "FLAKY") {
        continue;
      }

      if (result.status === 0) {
        if (!expectation || expectation.status === "FAIL") {
          console.log(
            `${expectation ? "UNEXPECTED" : "NEW"} PASS\n  NAME: ${result.name}`,
          );
          stats.unexpectedPass++;
        }
      } else if (!expectation || expectation.status === "PASS") {
        console.log(
          `${expectation ? "UNEXPECTED" : "NEW"} FAIL\n  NAME: ${result.name}\n  MESSAGE: ${result.message}`,
        );
        stats.unexpectedFail++;
      }
    }

    for (const [name, expectation] of Object.entries(expectations)) {
      if (!expectation.did_run) {
        stats.missing++;
        console.log(
          `MISSING TEST\n  NAME: ${name}\n  EXPECTED: ${expectation.status}`,
        );
      }
    }

    totalStats.count += stats.count;
    totalStats.pass += stats.pass;
    totalStats.missing += stats.missing;
    totalStats.unexpectedPass += stats.unexpectedPass;
    totalStats.unexpectedFail += stats.unexpectedFail;
    totalStats.duration += stats.duration;

    console.log(`${testPath.padEnd(pathLength)} ${formatStats(stats)}`);

    if (stats.unexpectedFail + stats.unexpectedPass + stats.missing > 0) {
      if (config.tests.updateExpectations) {
        const expectPath = expectationsPath(testPath);
        console.log(`  Writing expectations to ${expectPath}`);
        // Merge against the file as stored, not against `expectations` (which has
        // already been narrowed to this target), so the other target's statuses survive.
        const newExpectations = mergeExpectations(
          readExpectationsFile(testPath),
          results,
        );
        mkdirSync(path.dirname(expectPath), { recursive: true });
        writeFileSync(
          expectPath,
          JSON.stringify(newExpectations, null, 2) + "\n",
        );
        expectationsUpdated++;
      } else {
        printSTR(testPath);
      }
    }
  });

  // Report the suite's wall-clock time, not the sum of the per-test durations:
  // with tests running concurrently that sum exceeds the elapsed time, and it
  // grows as concurrency rises because each test's own duration absorbs the
  // contention. The per-test figures above stay as measured.
  totalStats.duration = Date.now() - suiteStart;
  console.log(
    `\n${"Done. Stats:".padEnd(pathLength)} ${formatStats(totalStats)}`,
  );

  if (config.tests.updateExpectations) {
    console.log(`Expectations updated: ${expectationsUpdated}`);
  } else if (
    totalStats.unexpectedFail + totalStats.unexpectedPass + totalStats.missing >
      0 ||
    unexpectedFailure
  ) {
    console.error(
      "\nUnexpected results. Run with --update-expectations to update.",
    );
    process.exit(1);
  }
}

run();
function printSTR(testPath) {
  // Copy the script to a stable name: the file the run used carries a pid, so
  // it would be meaningless (and eventually stale) in a command someone runs
  // later.
  const tmpPath = path.join(config.tmpDir, testPath.replace(/\//g, "_"));
  copyFileSync(runScriptPath(testPath), tmpPath);
  const { command, args } = testCommand(tmpPath, { optimized: false });
  console.error(`  To reproduce, run $ ${command} ${args.join(" ")}`);
}
