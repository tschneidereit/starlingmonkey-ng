// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception
//
// Compatibility shim prepended to vendored Node.js test files.
// Provides a synchronous require() backed by native modules.
//
// node:assert is implemented as a native Rust module; imported here so that
// require('assert') can return it synchronously.
import * as __assert_ns from "node:assert";

// Wrap the namespace in a callable so assert(v) works like assert.ok(v).
var __assert = Object.assign(
  function assert(v, msg) { return __assert_ns.ok(v, msg); },
  __assert_ns
);

var __req = Object.create(null);
__req['assert']      = __assert;
__req['node:assert'] = __assert;
__req['../common']   = {};
__req['./common']    = {};

function require(id) {
  if (id in __req) return __req[id];
  return {};
}
