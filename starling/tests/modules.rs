// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! End-to-end checks of module loading in the `starling` binary.

#![cfg(not(target_arch = "wasm32"))]

use std::process::Command;

/// A module importing the entry module by path gets the entry module itself, so
/// the entry's top level runs once and the importer sees its bindings. That holds
/// for an absolute entry path and for a bare filename in the working directory.
#[test]
fn importing_the_entry_module_gets_the_entry_module() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("main.mjs"),
        "import { markerType } from './dep.mjs';\n\
         export const marker = {};\n\
         globalThis.runs = (globalThis.runs ?? 0) + 1;\n\
         console.log(`runs ${globalThis.runs} ${markerType()}`);",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("dep.mjs"),
        "import * as main from './main.mjs';\n\
         export const markerType = () => typeof main.marker;",
    )
    .unwrap();
    for entry in [dir.path().join("main.mjs"), "main.mjs".into()] {
        let out = Command::new(env!("CARGO_BIN_EXE_starlingmonkey"))
            .current_dir(dir.path())
            .arg(&entry)
            .output()
            .expect("failed to run starling");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "runs 1 object\n",
            "entry {}: stderr: {}",
            entry.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
