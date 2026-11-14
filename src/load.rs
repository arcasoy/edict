//! Filesystem layer: finds crates, reads and parses source files, and builds
//! the module tree by following `mod` declarations from the crate root.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::attrs::is_cfg_test;
use crate::model::{ModId, Module};

/// A crate to analyze.
#[derive(Debug, Clone)]
pub struct CrateRoot {
    pub name: String,
    /// Directory containing the crate's Cargo.toml.
    pub dir: PathBuf,
    /// `src/lib.rs`, `src/main.rs`, or a `[lib] path`.
    pub root_file: PathBuf,
}

pub struct LoadedCrate {
    pub root: CrateRoot,
    pub modules: Vec<Module>,
    /// Items of each module, indexed by `ModId`.
    pub items: Vec<Vec<syn::Item>>,
    pub warnings: Vec<String>,
}

/// Find crates at `path`: a package, or every member of a workspace.
pub fn find_crates(path: &Path) -> Result<Vec<CrateRoot>, String> {
    let manifest = path.join("Cargo.toml");
    let src = fs::read_to_string(&manifest)
        .map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
    let table: toml::Table = src
        .parse()
        .map_err(|e| format!("invalid {}: {e}", manifest.display()))?;

    let mut out = Vec::new();
    if let Some(pkg) = table.get("package").and_then(|p| p.as_table()) {
        let name = pkg
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| format!("{} has no package.name", manifest.display()))?
            .replace('-', "_");
        let lib_path = table
            .get("lib")
            .and_then(|l| l.get("path"))
            .and_then(|p| p.as_str())
            .map(|p| path.join(p));
        let root_file = lib_path
            .filter(|p| p.exists())
            .or_else(|| Some(path.join("src/lib.rs")).filter(|p| p.exists()))
            .or_else(|| Some(path.join("src/main.rs")).filter(|p| p.exists()));
        match root_file {
            Some(root_file) => out.push(CrateRoot {
                name,
                dir: path.to_path_buf(),
                root_file,
            }),
            None if table.get("workspace").is_none() => {
                return Err(format!(
                    "no src/lib.rs or src/main.rs in {}",
                    path.display()
                ));
            }
            None => {}
        }
    }

    if let Some(members) = table
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
    {
        for m in members.iter().filter_map(|m| m.as_str()) {
            for dir in expand_member(path, m)? {
                out.extend(find_crates(&dir)?);
            }
        }
    }
    Ok(out)
}

fn expand_member(base: &Path, pattern: &str) -> Result<Vec<PathBuf>, String> {
    if let Some(prefix) = pattern.strip_suffix("/*") {
        let dir = base.join(prefix);
        let mut dirs: Vec<PathBuf> = fs::read_dir(&dir)
            .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join("Cargo.toml").exists())
            .collect();
        dirs.sort();
        Ok(dirs)
    } else {
        Ok(vec![base.join(pattern)])
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
