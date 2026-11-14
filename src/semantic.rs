//! Full mode: builds the dependency graph from a rust-analyzer SCIP index.
//!
//! The index records every symbol occurrence after macro expansion, `cfg`
//! evaluation and type inference, so it sees what the syntax engine can't:
//! method calls on inferred types, macro-generated code, trait impls.
//! Pure: takes an already-parsed index.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use scip::symbol::parse_symbol;
use scip::types::descriptor::Suffix;
use scip::types::{Index, SymbolRole};

use crate::model::{Edge, ExternRef, Loc, ModId, Module, RefKind, Source};

/// Where the index lives relative to the crate being analyzed.
pub struct IndexRoots<'a> {
    /// Absolute directory the index's `relative_path`s are relative to.
    pub index_root: &'a Path,
    /// Absolute directory of the crate (its Cargo.toml).
    pub crate_dir: &'a Path,
}

/// Per-file line ranges the syntax pass already knows about.
pub struct LineInfo<'a> {
    /// Test-only code: references inside are ignored.
    pub tests: &'a BTreeMap<PathBuf, Vec<(usize, usize)>>,
    /// `pub use` statements: references on these lines are re-exports.
    pub reexports: &'a BTreeMap<PathBuf, Vec<(usize, usize)>>,
}

fn within(ranges: &[(usize, usize)], line: usize) -> bool {
    ranges.iter().any(|&(lo, hi)| line >= lo && line <= hi)
}

pub struct SemanticRefs {
    pub edges: Vec<Edge>,
    pub externs: Vec<ExternRef>,
}

pub fn from_index(
    index: &Index,
    roots: &IndexRoots,
    modules: &[Module],
    lines: &LineInfo,
) -> SemanticRefs {
    // Crate-relative file -> module that owns the file.
    let file_module: HashMap<&Path, ModId> = modules
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.inline)
        .map(|(id, m)| (m.file.as_path(), id))
        .collect();

    // Documents that belong to this crate, keyed by their crate-relative path.
    let docs: Vec<(PathBuf, ModId, &scip::types::Document)> = index
        .documents
        .iter()
        .filter_map(|d| {
            let abs = roots.index_root.join(&d.relative_path);
            let rel = abs.strip_prefix(roots.crate_dir).ok()?.to_path_buf();
            let id = *file_module.get(rel.as_path())?;
            Some((rel, id, d))
        })
        .collect();

    // Symbol -> module that defines it (first definition wins).
    let mut defined_in: HashMap<&str, ModId> = HashMap::new();
    for (_, id, d) in &docs {
        for o in d
            .occurrences
            .iter()
            .filter(|o| is_definition(o.symbol_roles))
        {
            if !is_ignored_symbol(&o.symbol) {
                defined_in.entry(o.symbol.as_str()).or_insert(*id);
            }
        }
    }

    let mut out = SemanticRefs {
        edges: Vec::new(),
        externs: Vec::new(),
    };
    let mut seen = BTreeSet::new();
    for (rel, from, d) in &docs {
        let tests = lines.tests.get(rel).map(Vec::as_slice).unwrap_or(&[]);
        let reexports = lines.reexports.get(rel).map(Vec::as_slice).unwrap_or(&[]);
        for o in &d.occurrences {
            if is_definition(o.symbol_roles) || o.symbol_roles & SymbolRole::Test as i32 != 0 {
                continue;
            }
            if is_ignored_symbol(&o.symbol) {
                continue;
            }
            let Some(&start) = o.range.first() else {
                continue;
            };
            let line = start as usize + 1;
            if within(tests, line) {
                continue;
            }
            let kind = if within(reexports, line) {
                RefKind::PubUse
            } else {
                RefKind::Path
            };
            let loc = Loc {
                file: rel.clone(),
                line,
            };
            match defined_in.get(o.symbol.as_str()) {
                Some(&to) if to != *from => {
                    if seen.insert((*from, to, line)) {
                        out.edges.push(Edge {
                            from: *from,
                            to,
                            written_to: to,
                            kind,
                            path: symbol_path(&o.symbol, Some("crate"))
                                .unwrap_or_else(|| o.symbol.clone()),
                            loc,
                            source: Source::Semantic,
                        });
                    }
                }
                Some(_) => {}
                None => {
                    if let Some(path) = symbol_path(&o.symbol, None) {
                        out.externs.push(ExternRef {
                            from: *from,
                            path,
                            loc,
                        });
                    }
                }
            }
        }
    }
    out
}

fn is_definition(roles: i32) -> bool {
    roles & SymbolRole::Definition as i32 != 0
}

/// Locals carry no cross-module meaning. Module symbols (`foo/bar/`) are just
/// path segments: `use crate::a::b::X` mentions `crate`, `a` and `a::b`, but
/// only `X` is a dependency. Counting module mentions would link every parent
/// and child module into a cycle.
fn is_ignored_symbol(symbol: &str) -> bool {
    symbol.starts_with("local ") || symbol.ends_with('/')
}

/// Render a SCIP symbol as a Rust-style path, e.g.
/// `rust-analyzer cargo std … time/Instant#now().` -> `std::time::Instant::now`.
/// `prefix` replaces the package name (use `"crate"` for in-crate symbols).
fn symbol_path(symbol: &str, prefix: Option<&str>) -> Option<String> {
    let parsed = parse_symbol(symbol).ok()?;
    let package = parsed
        .package
        .as_ref()
        .map(|p| p.name.replace('-', "_"))
        .unwrap_or_default();
    let mut parts = vec![prefix.map(str::to_string).unwrap_or(package)];
    // `impl` blocks: `c/impl#[`crate::a::Widget`]process().` is a method on
    // `a::Widget` whose impl lives in `c`.
    let mut impl_in: Option<String> = None;
    let mut after_impl = false;
    for d in &parsed.descriptors {
        let suffix = d.suffix.enum_value_or_default();
        match suffix {
            Suffix::Parameter | Suffix::Local | Suffix::Meta => continue,
            Suffix::TypeParameter if after_impl => {
                let ty = d.name.trim_matches('`');
                let ty = ty.split('<').next().unwrap_or(ty).trim();
                if ty.contains("::") {
                    // A path to a type defined elsewhere: name the type, note the impl site.
                    impl_in = Some(parts.join("::"));
                    parts = ty.split("::").map(str::to_string).collect();
                } else {
                    parts.push(ty.to_string());
                }
            }
            Suffix::TypeParameter => {}
            _ if d.name == "impl" && suffix == Suffix::Type => {}
            _ => parts.push(d.name.clone()),
        }
        after_impl = d.name == "impl" && suffix == Suffix::Type;
    }
    if parts.len() < 2 {
        return None;
    }
    let path = parts.join("::");
    Some(match impl_in {
        Some(site) => format!("{path} (impl in {site})"),
        None => path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_symbol_paths() {
        let s = "rust-analyzer cargo std https://github.com/rust-lang/rust/library/std time/Instant#now().";
        assert_eq!(symbol_path(s, None).unwrap(), "std::time::Instant::now");
        let s = "rust-analyzer cargo alloc https://x vec/impl#[`Vec<T>`]new().";
        assert_eq!(symbol_path(s, None).unwrap(), "alloc::vec::Vec::new");
        let s = "rust-analyzer cargo cargo-edict 0.1.0 model/impl#[Analysis]top_of().";
        assert_eq!(
            symbol_path(s, Some("crate")).unwrap(),
            "crate::model::Analysis::top_of"
        );
    }

    #[test]
    fn ignores_modules_and_locals() {
        assert!(is_ignored_symbol("local 4"));
        assert!(is_ignored_symbol(
            "rust-analyzer cargo x 0.1.0 billing/tax/"
        ));
        assert!(!is_ignored_symbol(
            "rust-analyzer cargo x 0.1.0 billing/tax/rate()."
        ));
    }
}
