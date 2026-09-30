// SPDX-License-Identifier: Apache-2.0-WITH-LLVM-exception

//! The pass over the snapshotted component that gives it exactly the surface
//! the user asked for.
//!
//! The snapshot exports everything the runtime exports (`init`,
//! `wizer-initialize`, and the `wasi:cli/run` and `wasi:http/handler`
//! builtins) next to the world's own exports, and imports every WASI interface
//! the runtime links against, whether or not the application uses it.
//! [`finalize`] wraps it in a composition that exports only the names in
//! `keep_exports` and satisfies the imports of every disabled [`Feature`] from a
//! stub component whose functions trap, so the result imports none of a disabled
//! feature's interfaces.
//!
//! The stub component is generated from the snapshot's own import types, so the
//! resource types the remaining imports share with the stubbed ones stay the
//! same types. An interface whose types a remaining interface uses cannot be
//! stubbed at all: the composition would have to import it twice, once for each
//! side, as two distinct types. Such a conflict between a requested feature and
//! a remaining interface is an error naming both. An interface no feature owns
//! is stubbed only once nothing left uses it.
//!
//! Two reads are the exception to trapping, because the runtime makes them
//! whatever the application does. SpiderMonkey reads the monotonic clock on its
//! own, for GC scheduling, and the runtime reads it once on the first call after
//! a snapshot, so a disabled clock's `now` returns zero. `Date.now()` and
//! `performance.now()` then read constants, and waiting on the clock traps.
//! SpiderMonkey also seeds its hash tables from `wasi:random/insecure-seed`,
//! once per task on wasm32-wasip3, so that returns zeroes. `Math.random` and
//! `crypto.getRandomValues` read `wasi:random/random`, which still traps.

use std::collections::HashSet;

use anyhow::{bail, Context as _};
use indexmap::IndexSet;
use wac_graph::{types::Package, CompositionGraph, EncodeOptions};
use wasm_encoder::reencode::{Reencode as _, RoundtripReencoder};
use wasmparser::{ExternalKind, Payload, TypeRef};
use wit_parser::{
    decoding::DecodedWasm, InterfaceId, ManglingAndAbi, PackageName, Resolve, Type, TypeDefKind,
    TypeOwner, World, WorldId, WorldItem, WorldKey,
};

/// A group of WASI interfaces the componentizer can disable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, clap::ValueEnum)]
pub enum Feature {
    /// `wasi:cli/std*` and `wasi:cli/terminal-*`: `console` output and the
    /// standard streams.
    Stdio,
    /// `wasi:random/*`: `Math.random` seeding and `crypto.getRandomValues`.
    Random,
    /// `wasi:clocks/*`: `Date.now`, `performance.now`, and timers.
    Clocks,
    /// `wasi:http/*`: `fetch`.
    Http,
    /// `wasi:filesystem/*`: module loading from disk.
    Filesystem,
}

impl Feature {
    /// The interface-name prefixes of the feature's interfaces, without the
    /// package version.
    fn prefixes(self) -> &'static [&'static str] {
        match self {
            Feature::Stdio => &[
                "wasi:cli/stdin",
                "wasi:cli/stdout",
                "wasi:cli/stderr",
                "wasi:cli/terminal-",
            ],
            Feature::Random => &["wasi:random/"],
            Feature::Clocks => &["wasi:clocks/"],
            Feature::Http => &["wasi:http/"],
            Feature::Filesystem => &["wasi:filesystem/"],
        }
    }

    fn name(self) -> &'static str {
        match self {
            Feature::Stdio => "stdio",
            Feature::Random => "random",
            Feature::Clocks => "clocks",
            Feature::Http => "http",
            Feature::Filesystem => "filesystem",
        }
    }
}

/// Interfaces no feature owns, stubbed once every interface using their types
/// is stubbed.
///
/// `wasi:io` defines the p2 streams and pollables. `wasi:cli/types` defines the
/// `error-code` the p3 standard streams return. Neither is reached except
/// through an interface some feature owns.
const DEPENDENCY_ONLY: &[&str] = &["wasi:io/", "wasi:cli/types"];

/// The package the generated stub world lives in.
const STUB_PACKAGE: (&str, &str) = ("starling", "stubs");

/// The address of the zeroed return area a zero-returning stub returns a
/// pointer to.
const ZERO_AREA: i32 = 8;

/// An export of the snapshot that the finalized component keeps.
///
/// Both names are component export names: an interface's WIT name
/// (`starling:bench/api`) or a plain name, such as a world-level function's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeptExport {
    /// The snapshot's name for the export.
    pub snapshot_name: String,
    /// The finalized component's name for the export.
    pub name: String,
}

impl KeptExport {
    /// An export kept under the snapshot's name for it.
    pub fn unrenamed(name: String) -> Self {
        KeptExport {
            snapshot_name: name.clone(),
            name,
        }
    }
}

/// Wrap `snapshot` so the result exports exactly `keep_exports` and imports
/// nothing from the interfaces of `disabled`.
///
/// A [`KeptExport::snapshot_name`] the snapshot does not export is an error.
pub fn finalize(
    snapshot: &[u8],
    keep_exports: &[KeptExport],
    disabled: &[Feature],
) -> anyhow::Result<Vec<u8>> {
    let DecodedWasm::Component(mut resolve, world) =
        wit_parser::decoding::decode(snapshot).context("decoding the snapshot's world")?
    else {
        bail!("the snapshot is not a component");
    };

    let stubbed = stub_set(&resolve, world, keep_exports, disabled)?;
    let stub_names: Vec<String> = stubbed
        .iter()
        .map(|id| interface_name(&resolve, world, *id))
        .collect();

    let stub = if stubbed.is_empty() {
        None
    } else {
        Some(stub_component(&mut resolve, &stubbed)?)
    };

    let mut graph = CompositionGraph::new();
    let main = Package::from_bytes("main", None, snapshot.to_vec(), graph.types_mut())
        .context("registering the snapshot")?;
    let main = graph.register_package(main)?;
    let main_instance = graph.instantiate(main);
    if let Some(stub) = stub {
        let stub = Package::from_bytes("stubs", None, stub, graph.types_mut())
            .context("registering the stub component")?;
        let stub = graph.register_package(stub)?;
        // Instantiated before the main instance, so its own imports, the
        // interfaces the stubbed ones use, come first among the composition's.
        let stub_instance = graph.instantiate(stub);
        for name in &stub_names {
            let export = graph.alias_instance_export(stub_instance, name)?;
            graph.set_instantiation_argument(main_instance, name, export)?;
        }
    }
    for kept in keep_exports {
        let snapshot_name = &kept.snapshot_name;
        let export = graph
            .alias_instance_export(main_instance, snapshot_name)
            .with_context(|| format!("the snapshot does not export `{snapshot_name}`"))?;
        graph.export(export, &kept.name)?;
    }
    graph
        .encode(EncodeOptions::default())
        .context("encoding the finalized component")
}

/// The imported interfaces to stub: every interface of a disabled feature, plus
/// the dependency-only ones nothing remaining uses.
fn stub_set(
    resolve: &Resolve,
    world: WorldId,
    keep_exports: &[KeptExport],
    disabled: &[Feature],
) -> anyhow::Result<IndexSet<InterfaceId>> {
    let imports: Vec<InterfaceId> = resolve.worlds[world]
        .imports
        .values()
        .filter_map(|item| match item {
            WorldItem::Interface { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    let owner = |id: InterfaceId| -> Option<Feature> {
        let name = interface_name(resolve, world, id);
        disabled
            .iter()
            .copied()
            .find(|f| f.prefixes().iter().any(|p| name.starts_with(p)))
    };
    // The feature an interface belongs to, disabled or not.
    let feature_of = |id: InterfaceId| -> Option<Feature> {
        let name = interface_name(resolve, world, id);
        <Feature as clap::ValueEnum>::value_variants()
            .iter()
            .copied()
            .find(|f| f.prefixes().iter().any(|p| name.starts_with(p)))
    };
    let dependency_only = |id: InterfaceId| {
        let name = interface_name(resolve, world, id);
        DEPENDENCY_ONLY.iter().any(|p| name.starts_with(p))
    };

    let mut stubbed: IndexSet<InterfaceId> = imports
        .iter()
        .copied()
        .filter(|id| owner(*id).is_some() || dependency_only(*id))
        .collect();

    // The interfaces whose types the world `use`s at its top level. A disabled
    // feature's interface among them is a conflict, and a dependency-only one stays.
    let world_deps: IndexSet<InterfaceId> = resolve.worlds[world]
        .imports
        .values()
        .chain(resolve.worlds[world].exports.values())
        .filter_map(|item| match item {
            WorldItem::Type { id, .. } => match resolve.types[*id].kind {
                TypeDefKind::Type(Type::Id(used)) => match resolve.types[used].owner {
                    TypeOwner::Interface(interface) => Some(interface),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        })
        .collect();
    let world_uses: Vec<String> = world_deps
        .iter()
        .filter_map(|dep| {
            owner(*dep).map(|feature| {
                format!(
                    "`{}` of `{}` is used by the world",
                    interface_name(resolve, world, *dep),
                    feature.name(),
                )
            })
        })
        .collect();
    stubbed.retain(|id| owner(*id).is_some() || !world_deps.contains(id));

    // The interfaces that stay: the remaining imports and the kept exports.
    // Anything they use stays too.
    let kept_exports: Vec<InterfaceId> = resolve.worlds[world]
        .exports
        .iter()
        .filter(|(key, _)| {
            let name = resolve.name_world_key(key);
            keep_exports.iter().any(|kept| kept.snapshot_name == name)
        })
        .filter_map(|(_, item)| match item {
            WorldItem::Interface { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    loop {
        let users: Vec<InterfaceId> = imports
            .iter()
            .copied()
            .filter(|id| !stubbed.contains(id))
            .chain(kept_exports.iter().copied())
            .collect();
        let mut unstubbed = None;
        // Each use of a disabled feature's interface by one that stays, as the
        // feature, the used interface and the user.
        let mut conflicts = Vec::new();
        for user in users {
            for dep in resolve.interface_direct_deps(user) {
                if !stubbed.contains(&dep) {
                    continue;
                }
                match owner(dep) {
                    Some(feature) if !conflicts.contains(&(feature, dep, user)) => {
                        conflicts.push((feature, dep, user))
                    }
                    Some(_) => {}
                    None => unstubbed = unstubbed.or(Some(dep)),
                }
            }
        }
        if !conflicts.is_empty() || !world_uses.is_empty() {
            let uses: Vec<String> = world_uses
                .iter()
                .cloned()
                .chain(conflicts.iter().map(|(feature, dep, user)| {
                    let user_is = if kept_exports.contains(user) {
                        ", which the component exports".to_string()
                    } else {
                        feature_of(*user)
                            .map(|f| format!(" (feature `{}`)", f.name()))
                            .unwrap_or_default()
                    };
                    format!(
                        "`{}` of `{}` is used by `{}`{user_is}",
                        interface_name(resolve, world, *dep),
                        feature.name(),
                        interface_name(resolve, world, *user)
                    )
                }))
                .collect();
            // The features to disable as well: those of the remaining imports that use a
            // disabled feature, and not a feature that is disabled already.
            let mut also: Vec<&str> = conflicts
                .iter()
                .filter(|(_, _, user)| !kept_exports.contains(user))
                .filter_map(|(_, _, user)| feature_of(*user))
                .filter(|feature| !disabled.contains(feature))
                .map(|feature| feature.name())
                .collect();
            also.sort_unstable();
            also.dedup();
            let hint = if also.is_empty() {
                String::new()
            } else {
                format!(
                    " Disable {} as well.",
                    also.iter()
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(" and ")
                )
            };
            bail!(
                "cannot disable these features while interfaces that stay enabled use them: {}.{hint}",
                uses.join(", ")
            );
        }
        match unstubbed {
            Some(dep) => {
                stubbed.shift_remove(&dep);
            }
            None => return Ok(stubbed),
        }
    }
}

/// A stub component exporting `stubbed`, with every function trapping except a
/// clock's `now`.
fn stub_component(
    resolve: &mut Resolve,
    stubbed: &IndexSet<InterfaceId>,
) -> anyhow::Result<Vec<u8>> {
    // The stub world imports what its exports use, transitively, so the types
    // line up with the snapshot's.
    let mut imports = IndexSet::new();
    let mut pending: Vec<InterfaceId> = stubbed.iter().copied().collect();
    while let Some(id) = pending.pop() {
        for dep in resolve.interface_direct_deps(id).collect::<Vec<_>>() {
            if !stubbed.contains(&dep) && imports.insert(dep) {
                pending.push(dep);
            }
        }
    }
    let item = |id: InterfaceId| {
        (
            WorldKey::Interface(id),
            WorldItem::Interface {
                id,
                stability: Default::default(),
                external_id: None,
                docs: Default::default(),
                span: Default::default(),
            },
        )
    };
    let package = resolve.packages.alloc(wit_parser::Package {
        name: PackageName {
            namespace: STUB_PACKAGE.0.into(),
            name: STUB_PACKAGE.1.into(),
            version: None,
        },
        docs: Default::default(),
        interfaces: Default::default(),
        worlds: Default::default(),
    });
    let world = resolve.worlds.alloc(World {
        name: STUB_PACKAGE.1.into(),
        imports: imports.iter().copied().map(item).collect(),
        exports: stubbed.iter().copied().map(item).collect(),
        package: Some(package),
        docs: Default::default(),
        stability: Default::default(),
        includes: Default::default(),
        span: Default::default(),
    });
    resolve.packages[package]
        .worlds
        .insert(STUB_PACKAGE.1.into(), world);

    let module = wit_component::dummy_module(resolve, world, ManglingAndAbi::Standard32);
    let mut module = patch_stub_module(&module)?;
    wit_component::embed_component_metadata(
        &mut module,
        resolve,
        world,
        wit_component::StringEncoding::UTF8,
    )?;
    wit_component::ComponentEncoder::default()
        .validate(true)
        .module(&module)
        .context("registering the stub module")?
        .encode()
        .context("encoding the stub component")
}

/// Whether the stub export `name` returns zeroes rather than trapping.
///
/// `name` is a standard-mangled core export name, `cm32p2|<interface>|<item>`,
/// where the interface keeps the canonicalized version it was imported under, as
/// in `cm32p2|wasi:clocks/monotonic-clock@0.3|now`. A name in any other shape
/// traps.
fn returns_zero(name: &str) -> bool {
    let Some((interface, item)) = mangled_parts(name) else {
        return false;
    };
    match item {
        "now" => interface.starts_with("wasi:clocks/"),
        "get-insecure-seed" => interface.starts_with("wasi:random/insecure-seed"),
        _ => false,
    }
}

/// The interface and item names of the standard-mangled core export `name`, or
/// `None` for a name the mangling does not produce, such as `cm32p2_realloc`.
fn mangled_parts(name: &str) -> Option<(&str, &str)> {
    name.strip_prefix("cm32p2|")?.split_once('|')
}

/// Give the dummy module a page of memory, replace each zero-returning export's
/// body, and name the module and its exported functions.
///
/// The dummy module declares a zero-page memory and traps in every function.
/// A `now` returning its result through memory needs a page for the zeroed
/// return area, and its body becomes the zero constants of its result types,
/// with an `i32` result taken to be that return pointer.
///
/// The module is named `disabled-features`, and each exported function
/// `<interface>#<item> (disabled)`, so the backtrace of a trap in a stub names
/// the disabled function.
fn patch_stub_module(module: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut out = wasm_encoder::Module::new();
    let mut types: Vec<wasmparser::FuncType> = Vec::new();
    let mut imported_funcs = 0u32;
    let mut func_types: Vec<u32> = Vec::new();
    let mut patched: HashSet<u32> = HashSet::new();
    let mut names = wasm_encoder::NameMap::new();
    let mut code = wasm_encoder::CodeSection::new();
    let mut code_index = 0u32;
    let mut reencoder = RoundtripReencoder;
    for payload in wasmparser::Parser::new(0).parse_all(module) {
        let payload = payload.context("parsing the stub module")?;
        match &payload {
            Payload::TypeSection(reader) => {
                for group in reader.clone() {
                    for ty in group?.into_types() {
                        types.push(match ty.composite_type.inner {
                            wasmparser::CompositeInnerType::Func(f) => f,
                            _ => bail!("the stub module has a non-function type"),
                        });
                    }
                }
            }
            Payload::ImportSection(reader) => {
                for group in reader.clone() {
                    for import in group? {
                        if matches!(import?.1.ty, TypeRef::Func(_)) {
                            imported_funcs += 1;
                        }
                    }
                }
            }
            Payload::FunctionSection(reader) => {
                for ty in reader.clone() {
                    func_types.push(ty?);
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader.clone() {
                    let export = export?;
                    if export.kind != ExternalKind::Func {
                        continue;
                    }
                    if returns_zero(export.name) {
                        patched.insert(export.index);
                    }
                    if let Some((interface, item)) = mangled_parts(export.name) {
                        names.append(export.index, &format!("{interface}#{item} (disabled)"));
                    }
                }
            }
            Payload::MemorySection(reader) => {
                let mut memories = wasm_encoder::MemorySection::new();
                for memory in reader.clone() {
                    let mut ty = reencoder.memory_type(memory?)?;
                    ty.minimum = ty.minimum.max(1);
                    memories.memory(ty);
                }
                out.section(&memories);
                continue;
            }
            Payload::CodeSectionEntry(body) => {
                let index = imported_funcs + code_index;
                code_index += 1;
                if patched.contains(&index) {
                    let ty = &types[func_types[(index - imported_funcs) as usize] as usize];
                    let mut f = wasm_encoder::Function::new([]);
                    for result in ty.results() {
                        match result {
                            wasmparser::ValType::I32 => {
                                f.instruction(&wasm_encoder::Instruction::I32Const(ZERO_AREA));
                            }
                            wasmparser::ValType::I64 => {
                                f.instruction(&wasm_encoder::Instruction::I64Const(0));
                            }
                            other => bail!("a zero-returning stub has an unexpected {other:?}"),
                        }
                    }
                    f.instruction(&wasm_encoder::Instruction::End);
                    code.function(&f);
                } else {
                    reencoder.parse_function_body(&mut code, body.clone())?;
                }
                continue;
            }
            Payload::CodeSectionStart { .. } => continue,
            // Replaced by the names written below.
            Payload::CustomSection(section) if section.name() == "name" => continue,
            _ => {}
        }
        // Everything else is copied unchanged. The code section is written where
        // its entries ended, which `parse_all` reports as the section after the
        // last entry, so it is flushed before the next section is copied.
        if code_index > 0 && !matches!(payload, Payload::CodeSectionEntry(_)) {
            out.section(&code);
            code = wasm_encoder::CodeSection::new();
            code_index = 0;
        }
        if let Some((id, range)) = payload.as_section() {
            out.section(&wasm_encoder::RawSection {
                id,
                data: &module[range.start as usize..range.end as usize],
            });
        }
    }
    if code_index > 0 {
        out.section(&code);
    }
    let mut name_section = wasm_encoder::NameSection::new();
    name_section.module("disabled-features");
    name_section.functions(&names);
    out.section(&name_section);
    Ok(out.finish())
}

/// The WIT name of an interface the world imports or exports.
fn interface_name(resolve: &Resolve, world: WorldId, id: InterfaceId) -> String {
    let w = &resolve.worlds[world];
    let key = w
        .imports
        .iter()
        .chain(w.exports.iter())
        .find_map(|(key, item)| match item {
            WorldItem::Interface { id: i, .. } if *i == id => Some(key),
            _ => None,
        });
    match key {
        Some(key) => resolve.name_world_key(key),
        // An interface reached only through `use`, named by its package.
        None => resolve.id_of(id).expect("a used interface has a package"),
    }
}

/// The export names of `world`, the surface a component built for it keeps.
pub fn world_export_names(resolve: &Resolve, world: WorldId) -> Vec<String> {
    resolve.worlds[world]
        .exports
        .keys()
        .map(|key| resolve.name_world_key(key))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world exporting the kinds of interface `stub_component` stubs: a clock,
    /// the insecure seed, and ordinary randomness.
    const ZERO_WIT: &str = "\
package test:zero;

world stubs {
  export wasi:clocks/monotonic-clock@0.3.0;
  export wasi:random/insecure-seed@0.3.0;
  export wasi:random/random@0.3.0;
}

package wasi:clocks@0.3.0 {
  interface monotonic-clock {
    now: func() -> u64;
    get-resolution: func() -> u64;
  }
}

package wasi:random@0.3.0 {
  interface insecure-seed {
    get-insecure-seed: func() -> tuple<u64, u64>;
  }
  interface random {
    get-random-u64: func() -> u64;
  }
}
";

    /// `returns_zero` reads the names `dummy_module` writes, so it is checked
    /// against generated ones rather than against hand-written ones. A clock's
    /// `now` and the insecure seed return zeroes. Every other export traps.
    #[test]
    fn clock_now_and_insecure_seed_are_the_exports_returning_zero() {
        let mut resolve = Resolve::default();
        let package = resolve.push_str("zero.wit", ZERO_WIT).unwrap();
        let world = resolve.select_world(&[package], Some("stubs")).unwrap();
        let module = wit_component::dummy_module(&resolve, world, ManglingAndAbi::Standard32);

        let (mut zeroed, mut trapping) = (Vec::new(), Vec::new());
        for payload in wasmparser::Parser::new(0).parse_all(&module) {
            let Payload::ExportSection(reader) = payload.unwrap() else {
                continue;
            };
            for export in reader {
                let export = export.unwrap();
                if export.kind != ExternalKind::Func {
                    continue;
                }
                if returns_zero(export.name) {
                    zeroed.push(export.name.to_string());
                } else {
                    trapping.push(export.name.to_string());
                }
            }
        }

        assert_eq!(
            zeroed,
            [
                "cm32p2|wasi:clocks/monotonic-clock@0.3|now",
                "cm32p2|wasi:random/insecure-seed@0.3|get-insecure-seed",
            ]
        );
        for name in ["get-resolution", "get-random-u64"] {
            assert!(
                trapping.iter().any(|n| n.ends_with(name)),
                "`{name}` traps: {trapping:?}"
            );
        }
    }

    #[test]
    fn every_feature_has_a_name_and_prefixes() {
        for feature in [
            Feature::Stdio,
            Feature::Random,
            Feature::Clocks,
            Feature::Http,
            Feature::Filesystem,
        ] {
            assert!(!feature.name().is_empty());
            assert!(!feature.prefixes().is_empty());
        }
    }
}
