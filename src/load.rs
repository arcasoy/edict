//! Filesystem layer: finds crates, reads and parses source files, and builds
//! the module tree by following `mod` declarations from the crate root.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::attrs::is_cfg_test;
use crate::model::{Loc, ModId, Module, Visibility};

/// A crate to analyze.
#[derive(Debug, Clone)]
pub struct CrateRoot {
    pub name: String,
    /// Directory containing the crate's Cargo.toml.
    pub dir: PathBuf,
    /// `src/lib.rs`, `src/main.rs`, or a `[lib] path`.
    pub root_file: PathBuf,
    /// Rooted at a library target rather than a binary.
    pub is_lib: bool,
}

pub struct LoadedCrate {
    pub root: CrateRoot,
    pub modules: Vec<Module>,
    /// Items of each module, indexed by `ModId`.
    pub items: Vec<Vec<syn::Item>>,
    pub warnings: Vec<String>,
}

/// Crates found under a path, plus notes about members that were skipped.
pub struct Found {
    pub crates: Vec<CrateRoot>,
    pub notes: Vec<String>,
}

/// Find crates at `path`: a package, or every member of a workspace.
pub fn find_crates(path: &Path) -> Result<Found, String> {
    let mut found = Found {
        crates: Vec::new(),
        notes: Vec::new(),
    };
    let mut seen = BTreeSet::new();
    visit(path, true, &mut found, &mut seen)?;
    Ok(found)
}

fn visit(
    path: &Path,
    is_root: bool,
    found: &mut Found,
    seen: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    // Workspaces may list themselves (`members = ["."]`) or overlap; visit each once.
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !seen.insert(key) {
        return Ok(());
    }
    let manifest = path.join("Cargo.toml");
    let src = fs::read_to_string(&manifest)
        .map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
    let table: toml::Table = src
        .parse()
        .map_err(|e| format!("invalid {}: {e}", manifest.display()))?;
    let workspace = table.get("workspace").and_then(|w| w.as_table());

    if let Some(pkg) = table.get("package").and_then(|p| p.as_table()) {
        let name = pkg
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| format!("{} has no package.name", manifest.display()))?
            .replace('-', "_");
        match root_file(path, &table) {
            Some((root_file, is_lib)) => found.crates.push(CrateRoot {
                name,
                dir: path.to_path_buf(),
                root_file,
                is_lib,
            }),
            None if is_root && workspace.is_none() => {
                return Err(format!("no library or binary target in {}", path.display()));
            }
            None => found.notes.push(format!(
                "skipping `{name}` ({}): no library or binary target (benches or examples only?)",
                path.display()
            )),
        }
    }

    let Some(ws) = workspace else { return Ok(()) };
    let excluded: BTreeSet<PathBuf> = ws
        .get("exclude")
        .and_then(|e| e.as_array())
        .into_iter()
        .flatten()
        .filter_map(|e| e.as_str())
        .flat_map(|pattern| expand_member(path, pattern))
        .map(|p| p.canonicalize().unwrap_or(p))
        .collect();
    for pattern in ws
        .get("members")
        .and_then(|m| m.as_array())
        .into_iter()
        .flatten()
        .filter_map(|m| m.as_str())
    {
        let dirs = expand_member(path, pattern);
        if dirs.is_empty() {
            found
                .notes
                .push(format!("workspace member `{pattern}` matched no crate"));
        }
        for dir in dirs {
            if !excluded.contains(&dir.canonicalize().unwrap_or_else(|_| dir.clone())) {
                visit(&dir, false, found, seen)?;
            }
        }
    }
    Ok(())
}

/// The crate's root file: `[lib] path`, `src/lib.rs`, `src/main.rs`, or the
/// first `[[bin]]` with a path.
fn root_file(dir: &Path, table: &toml::Table) -> Option<(PathBuf, bool)> {
    let declared_lib = table
        .get("lib")
        .and_then(|l| l.get("path"))
        .and_then(|p| p.as_str())
        .map(|p| dir.join(p));
    let first_bin = table
        .get("bin")
        .and_then(|b| b.as_array())
        .into_iter()
        .flatten()
        .find_map(|b| b.get("path").and_then(|p| p.as_str()))
        .map(|p| dir.join(p));
    [
        declared_lib.map(|p| (p, true)),
        Some((dir.join("src/lib.rs"), true)),
        Some((dir.join("src/main.rs"), false)),
        first_bin.map(|p| (p, false)),
    ]
    .into_iter()
    .flatten()
    .find(|(p, _)| p.is_file())
}

/// Expand a workspace member pattern (`crates/*`, `axum-*`, `.`) into crate directories.
fn expand_member(base: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut dirs = vec![base.to_path_buf()];
    for seg in pattern.split('/').filter(|s| !s.is_empty() && *s != ".") {
        if seg.contains(['*', '?']) {
            dirs = dirs
                .iter()
                .flat_map(|d| {
                    fs::read_dir(d)
                        .into_iter()
                        .flatten()
                        .flatten()
                        .map(|e| e.path())
                })
                .filter(|p| {
                    p.is_dir()
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| wildcard(seg, n))
                })
                .collect();
        } else {
            dirs = dirs.iter().map(|d| d.join(seg)).collect();
        }
    }
    dirs.retain(|d| d.join("Cargo.toml").is_file());
    dirs.sort();
    dirs
}

fn wildcard(pattern: &str, name: &str) -> bool {
    fn go(p: &[u8], n: &[u8]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some(b'*'), _) => go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..])),
            (Some(b'?'), Some(_)) => go(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &n[1..]),
            _ => false,
        }
    }
    go(pattern.as_bytes(), name.as_bytes())
}

fn visibility(v: &syn::Visibility) -> Visibility {
    match v {
        syn::Visibility::Public(_) => Visibility::Public,
        syn::Visibility::Inherited => Visibility::Private,
        syn::Visibility::Restricted(r) => {
            let path: Vec<String> = r
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            match path.as_slice() {
                [one] if one == "crate" => Visibility::Crate,
                [one] if one == "super" => Visibility::Super,
                [one] if one == "self" => Visibility::Private,
                _ => Visibility::Restricted(path),
            }
        }
    }
}

/// Parse the crate starting at its root file and follow every `mod` declaration.
pub fn load_crate(root: &CrateRoot) -> Result<LoadedCrate, String> {
    let mut loaded = LoadedCrate {
        root: root.clone(),
        modules: Vec::new(),
        items: Vec::new(),
        warnings: Vec::new(),
    };
    let (items, lines) = parse_file(&root.root_file)?;
    let child_dir = root.root_file.parent().unwrap_or(&root.dir).to_path_buf();
    add_module(
        &mut loaded,
        Vec::new(),
        root.root_file.clone(),
        None,
        false,
        lines,
        items,
        child_dir,
    );
    Ok(loaded)
}

fn parse_file(path: &Path) -> Result<(Vec<syn::Item>, usize), String> {
    let src =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let file = syn::parse_file(&src).map_err(|e| {
        let start = e.span().start();
        format!(
            "{}:{}:{}: parse error: {e}",
            path.display(),
            start.line,
            start.column + 1
        )
    })?;
    Ok((file.items, src.lines().count()))
}

#[allow(clippy::too_many_arguments)]
fn add_module(
    loaded: &mut LoadedCrate,
    segments: Vec<String>,
    file: PathBuf,
    parent: Option<ModId>,
    inline: bool,
    lines: usize,
    items: Vec<syn::Item>,
    child_dir: PathBuf,
) -> ModId {
    let id = loaded.modules.len();
    let rel_file = file
        .strip_prefix(&loaded.root.dir)
        .unwrap_or(&file)
        .to_path_buf();
    loaded.modules.push(Module {
        segments: segments.clone(),
        file: rel_file,
        parent,
        children: BTreeMap::new(),
        inline,
        lines,
        vis: Visibility::Private,
        decl: None,
    });
    loaded.items.push(Vec::new());

    for item in &items {
        let syn::Item::Mod(m) = item else { continue };
        if is_cfg_test(&m.attrs) {
            continue;
        }
        let name = m.ident.to_string();
        let mut child_segments = segments.clone();
        child_segments.push(name.clone());

        let child = match &m.content {
            Some((_, inner)) => {
                let dir = child_dir.join(&name);
                add_module(
                    loaded,
                    child_segments,
                    file.clone(),
                    Some(id),
                    true,
                    0,
                    inner.clone(),
                    dir,
                )
            }
            None => {
                let Some(path) = locate_module_file(&child_dir, &file, &name, &m.attrs) else {
                    loaded.warnings.push(format!(
                        "{}: module file for `mod {name};` not found (cfg-gated or generated?), skipping",
                        file.display()
                    ));
                    continue;
                };
                match parse_file(&path) {
                    Ok((child_items, child_lines)) => {
                        let dir = if path.file_name().is_some_and(|f| f == "mod.rs") {
                            path.parent().unwrap_or(&child_dir).to_path_buf()
                        } else {
                            path.with_extension("")
                        };
                        add_module(
                            loaded,
                            child_segments,
                            path,
                            Some(id),
                            false,
                            child_lines,
                            child_items,
                            dir,
                        )
                    }
                    Err(e) => {
                        loaded.warnings.push(e);
                        continue;
                    }
                }
            }
        };
        let decl_file = loaded.modules[id].file.clone();
        loaded.modules[child].vis = visibility(&m.vis);
        loaded.modules[child].decl = Some(Loc {
            file: decl_file,
            line: m.ident.span().start().line,
        });
        loaded.modules[id].children.insert(name, child);
    }

    loaded.items[id] = items;
    id
}

fn locate_module_file(
    child_dir: &Path,
    current_file: &Path,
    name: &str,
    attrs: &[syn::Attribute],
) -> Option<PathBuf> {
    for attr in attrs {
        if attr.path().is_ident("path") {
            if let syn::Meta::NameValue(nv) = &attr.meta {
                if let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) = &nv.value
                {
                    let base = current_file.parent().unwrap_or(child_dir);
                    let p = base.join(s.value());
                    return p.exists().then_some(p);
                }
            }
        }
    }
    let flat = child_dir.join(format!("{name}.rs"));
    if flat.exists() {
        return Some(flat);
    }
    let nested = child_dir.join(name).join("mod.rs");
    nested.exists().then_some(nested)
}

#[cfg(test)]
mod tests {
    use super::wildcard;

    #[test]
    fn wildcards() {
        assert!(wildcard("axum-*", "axum-core"));
        assert!(!wildcard("axum-*", "axum"));
        assert!(wildcard("*", "anything"));
        assert!(wildcard("tok?o", "tokio"));
        assert!(!wildcard("a*c", "abd"));
    }
}
