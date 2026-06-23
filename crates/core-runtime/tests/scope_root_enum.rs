// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! `#[derive(ScopeRoot)]` must handle a unit enum variant.
//!
//! The mirror/pattern builder treated a unit variant as the parenthesized
//! (tuple) case, emitting `Variant()` patterns that fail to compile against a
//! unit variant (E0532). A unit variant now carries no field group.

use core_runtime::config::RuntimeConfig;
use core_runtime::runtime::Runtime;
use core_runtime::Traceable;
use js::gc::handle::Heap;
use js::native::Value;
use js::ScopeRoot;

#[derive(Traceable, ScopeRoot)]
#[allow(dead_code)]
enum Reply {
    // Unit variant — previously produced an uncompilable `Reply::Empty ()` pattern.
    Empty,
    // Single `Heap<Value>` field (fast path).
    One(Heap<Value>),
    // Two `Heap` fields (traced path: boxed before rooting).
    Two { a: Heap<Value>, b: Heap<Value> },
}

#[test]
fn scope_root_enum_unit_variant_roots() {
    let rt = Runtime::init(&RuntimeConfig::default());
    let scope = rt.default_global();

    let rooted = Reply::Empty.root(&scope);
    assert!(matches!(rooted, StackReply::Empty));
}
