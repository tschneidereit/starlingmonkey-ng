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
//! rooted!(&in(cx) let map = Map::new(cx)?);
//! map.set(cx, key.handle(), val.handle())?;
//! assert!(map.has(cx, key.handle())?);
//! ```

pub mod map;
pub mod set;
pub mod weak_map;
