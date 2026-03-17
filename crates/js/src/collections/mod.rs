// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! Map, Set, and WeakMap collection types.
//!
//! Each collection is represented by a scope-rooted newtype wrapping `Handle<'s, *mut JSObject>`.
//! Create instances with the `new()` constructor and use methods for operations.
//!
//! # Example
//!
//! ```ignore
//! use crate::collections::map::Map;
//!
//! let map = Map::new(scope)?;
//! map.set(scope, key, val)?;
//! assert!(map.has(scope, key)?);
//! ```

pub mod map;
pub mod set;
pub mod weak_map;
