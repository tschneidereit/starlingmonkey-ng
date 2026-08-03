// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception
//
// Compatibility shim prepended to vendored Node.js test files.
//
// The native require() function (installed by node-builtins) handles
// node:assert, node:fs, node:path and their bare aliases. This shim
// wraps it to provide:
//   - assert as a callable (Node CJS compat: assert(v) === assert.ok(v))
//   - test-infrastructure modules: common, tmpdir, fixtures, util

var __nativeRequire = require;
var __assert = (function() {
  var ns = __nativeRequire('node:assert');
  return Object.assign(
    function assert(v, msg) { return ns.ok(v, msg); },
    ns
  );
})();

var __fs = __nativeRequire('node:fs');
var __path = __nativeRequire('node:path');

// --- common helpers ---
var __mustCallCheckCounters = [];
var __common = {
  isWindows: process.platform === 'win32',
  isIBMi: false,
  hasTemporal: typeof globalThis.Temporal !== 'undefined',
  hasIntl: typeof Intl !== 'undefined',
};

__common.mustCall = function(fn, exact) {
  if (typeof fn === 'number') { exact = fn; fn = function() {}; }
  if (typeof exact !== 'number') exact = 1;
  var ctx = { exact: exact, actual: 0, name: fn.name || '<anonymous>' };
  __mustCallCheckCounters.push(ctx);
  return function() {
    ctx.actual++;
    return fn.apply(this, arguments);
  };
};
__common.mustSucceed = function(fn, exact) {
  return __common.mustCall(function(err) {
    __assert.ok(err === null || err === undefined, err);
    if (typeof fn === 'function') return fn.apply(this, Array.prototype.slice.call(arguments, 1));
  }, exact);
};
__common.mustNotCall = function(msg) {
  return function() {
    __assert.fail((msg || 'function should not have been called'));
  };
};
__common.mustNotMutateObjectDeep = function(original) {
  return original;
};
__common.expectWarning = function() {};

__common.skip = function(msg) {
  console.log('NODE_TEST_RESULTS: ' + JSON.stringify({
    pass: 0, fail: 0, skip: 1, total: 1,
    errors: [],
    tests: { 'script passed': 'skip' },
  }));
};

// --- tmpdir ---
var __tmpdirPath = process.cwd() + '/.tmp.0';
var __tmpdir = {
  get path() { return __tmpdirPath; },
  set path(v) { __tmpdirPath = v; },
  refresh: function() {
    try {
      if (__fs.existsSync(__tmpdirPath)) {
        __fs.rmSync(__tmpdirPath, { recursive: true, force: true });
      }
    } catch(e) {}
    __fs.mkdirSync(__tmpdirPath, { recursive: true });
  },
  resolve: function() {
    var parts = Array.prototype.slice.call(arguments);
    var result = __tmpdirPath;
    for (var i = 0; i < parts.length; i++) {
      if (parts[i]) result = result + '/' + parts[i];
    }
    return __path.resolve(result);
  },
};

// --- fixtures ---
var __fixturesDir = __path.resolve(process.cwd(), 'tests/node-compat/node-test/test/fixtures');
var __fixtures = {
  fixturesDir: __fixturesDir,
  path: function() {
    var parts = Array.prototype.slice.call(arguments);
    var result = __fixturesDir;
    for (var i = 0; i < parts.length; i++) {
      if (parts[i]) result = result + '/' + parts[i];
    }
    return result;
  },
  readSync: function(name, enc) {
    return __fs.readFileSync(__fixtures.path(name), enc);
  },
};

// --- require override ---
// Intercept test-infrastructure modules; fall through to native require.
var __unknownRequires = [];
require = function require(id) {
  switch (id) {
    case 'assert': case 'node:assert': return __assert;
    case '../common': case './common': case 'common': return __common;
    case '../common/tmpdir': case './common/tmpdir': case 'tmpdir': return __tmpdir;
    case '../common/fixtures': case './common/fixtures': case 'fixtures': return __fixtures;
    case 'util':
      return {
        inspect: function(v) {
          try { return JSON.stringify(v); } catch(e) { return String(v); }
        },
        getCallSites: function() {
          try {
            var e = new Error();
            var stack = e.stack.split('\n').slice(1);
            return stack.map(function(line) {
              var m = line.match(/at (.+):(\d+):(\d+)/);
              return m ? { scriptName: m[1], lineNumber: parseInt(m[2]) } : {};
            });
          } catch(e) { return []; }
        },
      };
  }
  try { return __nativeRequire(id); }
  catch(e) { __unknownRequires.push(id); return {}; }
};

// ---------------------------------------------------------------------------
// Async error tracking
// ---------------------------------------------------------------------------
var __asyncError = null;

(function() {
  var _st = globalThis.setTimeout;
  var _si = globalThis.setInterval;
  globalThis.setTimeout = function(fn) {
    var a = Array.prototype.slice.call(arguments);
    if (typeof fn === 'function') {
      var origFn = fn;
      a[0] = function() {
        try { return origFn.apply(this, arguments); }
        catch(e) { if (!__asyncError) __asyncError = e; }
      };
    }
    return _st.apply(null, a);
  };
  globalThis.setInterval = function(fn) {
    var a = Array.prototype.slice.call(arguments);
    if (typeof fn === 'function') {
      var origFn = fn;
      a[0] = function() {
        try { return origFn.apply(this, arguments); }
        catch(e) { if (!__asyncError) __asyncError = e; }
      };
    }
    return _si.apply(null, a);
  };
})();
