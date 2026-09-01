// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Implementation of the `File` global.
//!
//! Implements [`File`](https://w3c.github.io/FileAPI/#file-section) WebIDL.

use core_runtime::{webidl_interface, webidl_methods};

/// <https://w3c.github.io/FileAPI/#file-section>
#[webidl_interface]
pub struct File {
    // TODO(@zkat): This is a placeholder for now because the Blob WPTs rely on File existing.
}

#[webidl_methods]
impl File {}
