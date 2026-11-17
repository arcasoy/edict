//! The analysis pipeline: find crates, load and parse them, optionally merge
//! the SCIP index, and run the laws. Everything the commands share.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::extract::extract;
use crate::load::{CrateRoot, find_crates, load_crate};
use crate::model::{Analysis, Source};
use crate::report::CrateReport;
use crate::resolve::resolve;
use crate::rules;
use crate::scip_index::{self, LoadedIndex};
use crate::semantic::{self, IndexRoots, LineInfo};

pub const CONFIG_FILE: &str = "edict.toml";

pub struct Settings<'a> {
    /// Explicit config file; otherwise edict.toml in the crate, then the workspace.
    pub config: Option<&'a Path>,
    /// Merge rust-analyzer's SCIP index into the graph.
    pub full: bool,
    /// Existing SCIP index to use instead of running rust-analyzer.
    pub scip: Option<&'a Path>,
    /// Don't print notes (agent-hook mode).
    pub quiet: bool,
}

/// One analyzed crate, before the laws run.
pub struct Built {
    pub analysis: Analysis,
    pub config: Config,
    pub warnings: Vec<String>,
    /// Full mode only: file-to-file dependencies only rust-analyzer found.
    pub semantic_only: Option<usize>,
    pub full: bool,
}

/// Analyze every crate under `dir`.
pub fn build_all(dir: &Path, s: &Settings) -> Result<Vec<Built>, String> {
    let found = find_crates(dir)?;
    if found.crates.is_empty() {
        return Err(format!("no crates found in {}", dir.display()));
    }
    if !s.quiet {
        for note in &found.notes {
            eprintln!("edict: note: {note}");
        }
    }
    // One index for the whole workspace, shared by every crate in it.
    let index = if s.full {
        Some(scip_index::obtain(dir, s.scip)?)
    } else {
        None
    };
    found
        .crates
        .iter()
        .map(|root| {
            let config = load_config(s.config, root, dir)?;
            build(root, config, index.as_ref())
        })
        .collect()
}

/// Run the laws on one analyzed crate.
pub fn report(b: &Built) -> CrateReport {
    let mut violations = rules::run(&b.analysis, &b.config);
    violations.sort_by(|x, y| y.severity.cmp(&x.severity).then_with(|| x.rule.cmp(y.rule)));
    CrateReport {
        crate_name: b.analysis.crate_name.clone(),
        mode: if b.full { "full" } else { "fast" },
        modules: b.analysis.modules.len(),
        edges: b.analysis.edges.iter().filter(|e| e.from != e.to).count(),
        semantic_only_edges: b.semantic_only,
        baselined: 0,
        stale: 0,
        warnings: b.warnings.clone(),
        violations,
    }
}

fn load_config(
    explicit: Option<&Path>,
    root: &CrateRoot,
    workspace: &Path,
) -> Result<Config, String> {
    let candidates: Vec<PathBuf> = match explicit {
        Some(p) => vec![p.to_path_buf()],
        None => vec![root.dir.join(CONFIG_FILE), workspace.join(CONFIG_FILE)],
    };
    for path in candidates {
        if path.exists() {
            let src = fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            return Config::parse(&src).map_err(|e| format!("{}: {e}", path.display()));
        }
    }
    if let Some(p) = explicit {
        return Err(format!("config {} not found", p.display()));
    }
    Ok(Config::default())
}

fn build(root: &CrateRoot, config: Config, index: Option<&LoadedIndex>) -> Result<Built, String> {
    let loaded = load_crate(root)?;
    let ex = extract(&loaded.modules, &loaded.items);
    let (mut edges, mut externs) = resolve(&root.name, &loaded.modules, &ex.exports, &ex.raw);

    // Full mode = fast mode plus everything rust-analyzer finds. Union, never
    // replace: full mode can only add findings, so a fast pass is a strict
    // subset of a full pass.
    if let Some(idx) = index {
        let crate_dir = scip_index::canonical(&root.dir)?;
        let roots = IndexRoots {
            index_root: &idx.root,
            crate_dir: &crate_dir,
        };
        let lines = LineInfo {
            tests: &ex.test_lines,
            reexports: &ex.reexport_lines,
        };
        let sem = semantic::from_index(&idx.index, &roots, &loaded.modules, &lines);
        edges.extend(sem.edges);
        externs.extend(sem.externs);
    }

    let analysis = Analysis {
        crate_name: root.name.clone(),
        is_lib: root.is_lib,
        modules: loaded.modules,
        edges,
        externs,
        statics: ex.statics,
        macros: ex.macros,
    };
    let semantic_only = index.map(|_| semantic_only(&analysis));
    Ok(Built {
        analysis,
        config,
        warnings: loaded.warnings,
        semantic_only,
        full: index.is_some(),
    })
}

/// File-to-file dependencies that only rust-analyzer found: a direct measure
/// of what fast mode misses on this codebase.
fn semantic_only(a: &Analysis) -> usize {
    let pair = |from, to| (a.file_module_of(from), a.file_module_of(to));
    let syntactic: BTreeSet<_> = a
        .edges
        .iter()
        .filter(|e| e.source == Source::Syntax && !a.is_barrel(e))
        .map(|e| pair(e.from, e.to))
        .collect();
    let semantic: BTreeSet<_> = a
        .edges
        .iter()
        .filter(|e| e.source == Source::Semantic && !a.is_barrel(e))
        .map(|e| pair(e.from, e.to))
        .filter(|(f, t)| f != t)
        .collect();
    semantic.difference(&syntactic).count()
}
