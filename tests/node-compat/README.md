# Node.js Compatibility Tests

This directory tracks StarlingMonkey's compatibility with the upstream Node.js
test suite.

## Source

Test files come from [`denoland/node_test`](https://github.com/denoland/node_test),
a vendored snapshot of the [`test/`](https://github.com/nodejs/node/tree/main/test)
directory from the Node.js repository.  The submodule currently pins **Node.js
v26.3.0**.

```
tests/node-compat/
  node-test/           ← git submodule: github.com/denoland/node_test
    test/
      parallel/        ← upstream test files (read by the runner)
      common/          ← Node.js test utilities (not used directly)
  expectations/        ← per-file expectations (WPT-style JSON)
    parallel/
      test-process-release.js.json
      ...
  upstream-tests.json  ← list of test files to run
  upstream-shim.js     ← prepended to every upstream test (see below)
  run.mjs              ← test runner
```

## Running the tests

```sh
# first-time setup — initialise the submodule
git submodule update --init tests/node-compat/node-test

# build with the node_compat feature flag first
cargo build --features node_compat

# run all tests
node tests/node-compat/run.mjs

# filter by name
node tests/node-compat/run.mjs process-release

# verbose output
node tests/node-compat/run.mjs -v

# update expectations from current results
node tests/node-compat/run.mjs --update
```

## Adding more test files

1. Check that the file exists in the submodule:
   ```sh
   ls tests/node-compat/node-test/test/parallel/test-process-foo.js
   ```
2. Add the path to [`upstream-tests.json`](upstream-tests.json):
   ```json
   ["parallel/test-process-foo.js"]
   ```
3. Run `--update` to record the initial expected results:
   ```sh
   node tests/node-compat/run.mjs --update
   git add tests/node-compat/expectations/parallel/test-process-foo.js.json
   ```
