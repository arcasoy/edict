//! Command-line entry: argument parsing, config loading, orchestration, exit codes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

use std::collections::BTreeSet;

use crate::config::{Config, TEMPLATE};
use crate::extract::extract;
use crate::graph_view;
use crate::load::{CrateRoot, find_crates, load_crate};
use crate::model::{Analysis, Severity, Source};
use crate::report::{self, CrateReport};
use crate::resolve::resolve;
use crate::rules;
use crate::scip_index::{self, LoadedIndex};
use crate::semantic::{self, IndexRoots, LineInfo};

const CONFIG_FILE: &str = "edict.toml";

#[derive(Parser)]
#[command(name = "cargo", bin_name = "cargo")]
enum Cargo {
    /// Deterministic architecture laws for Rust crates.
    Edict(EdictArgs),
}

#[derive(Args)]
#[command(version, about = "Deterministic architecture laws for Rust crates.")]
struct EdictArgs {
    #[command(subcommand)]
    command: Option<Command>,

    /// Crate or workspace directory.
    #[arg(long, global = true, default_value = ".")]
    path: PathBuf,

    /// Config file (default: edict.toml in the crate, then the workspace root).
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Output format.
    #[arg(long, global = true, value_enum, default_value_t = Format::Human)]
    format: Format,

    /// Full mode: also build the graph from rust-analyzer's SCIP index
    /// (macros expanded, types inferred). Slower; use before push or in CI.
    #[arg(long, global = true)]
    full: bool,

    /// Use an existing SCIP index instead of running rust-analyzer. Implies --full.
    #[arg(long, global = true, value_name = "INDEX")]
    scip: Option<PathBuf>,

    /// Agent-hook output: silent on success; on errors, report to stderr and exit 2.
    #[arg(long, global = true)]
    hook: bool,
}

struct CheckOpts<'a> {
    config: Option<&'a Path>,
    format: Format,
    full: bool,
    scip: Option<&'a Path>,
    hook: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Check the crate against its laws (default).
    Check,
    /// Write a starter edict.toml.
    Init,
    /// Write an interactive HTML map of the module graph.
    Graph {
        /// Output file. In a workspace, one file per crate: `<stem>-<crate>.html`.
        #[arg(long, default_value = "edict-graph.html")]
        out: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Human,
    Json,
}

pub fn main() -> ExitCode {
    // Invoked as `cargo edict ...` cargo passes "edict" as the first argument;
    // invoked directly as `cargo-edict ...` it does not.
    let mut args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some("edict") {
        args.insert(1, "edict".to_string());
    }
    let Cargo::Edict(args) = Cargo::parse_from(args);

    match args.command.unwrap_or(Command::Check) {
        Command::Init => init(&args.path),
        Command::Graph { out } => graph(
            &args.path,
            &out,
            &CheckOpts {
                config: args.config.as_deref(),
                format: args.format,
                full: args.full || args.scip.is_some(),
                scip: args.scip.as_deref(),
                hook: false,
            },
        ),
        Command::Check => check(
            &args.path,
            &CheckOpts {
                config: args.config.as_deref(),
                format: args.format,
                full: args.full || args.scip.is_some(),
                scip: args.scip.as_deref(),
                hook: args.hook,
            },
        ),
    }
}

fn init(dir: &Path) -> ExitCode {
    let path = dir.join(CONFIG_FILE);
    if path.exists() {
        eprintln!("edict: {} already exists", path.display());
        return ExitCode::FAILURE;
    }
    match fs::write(&path, TEMPLATE) {
        Ok(()) => {
            println!("edict: wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("edict: cannot write {}: {e}", path.display());
            ExitCode::FAILURE
        }
    }
}

fn check(dir: &Path, opts: &CheckOpts) -> ExitCode {
    let fail = |msg: String| {
        eprintln!("edict: {msg}");
        ExitCode::from(if opts.hook { 2 } else { 3 })
    };
    let crates = match find_crates(dir) {
        Ok(c) if c.is_empty() => return fail(format!("no crates found in {}", dir.display())),
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    // One index for the whole workspace, shared by every crate in it.
    let index = if opts.full {
        match scip_index::obtain(dir, opts.scip) {
            Ok(i) => Some(i),
            Err(e) => return fail(e),
        }
    } else {
        None
    };

    let mut reports = Vec::new();
    for root in &crates {
        let cfg = match load_config(opts.config, root, dir) {
            Ok(c) => c,
            Err(e) => return fail(e),
        };
        match analyze(root, &cfg, index.as_ref()) {
            Ok(r) => reports.push(r),
            Err(e) => return fail(e),
        }
    }

    let failed = reports.iter().any(|r| r.count(Severity::Error) > 0);
    let output = match opts.format {
        Format::Human => report::human(&reports),
        Format::Json => report::json(&reports),
    };
    if opts.hook {
        if failed {
            eprintln!("{output}");
            return ExitCode::from(2);
        }
        return ExitCode::SUCCESS;
    }
    println!("{output}");
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
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

struct Built {
    analysis: Analysis,
    warnings: Vec<String>,
    /// Full mode only: file-to-file dependencies only rust-analyzer found.
    semantic_only: Option<usize>,
}

fn build(root: &CrateRoot, index: Option<&LoadedIndex>) -> Result<Built, String> {
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
        modules: loaded.modules,
        edges,
        externs,
        statics: ex.statics,
        macros: ex.macros,
    };
    let semantic_only = index.map(|_| semantic_only(&analysis));
    Ok(Built {
        analysis,
        warnings: loaded.warnings,
        semantic_only,
    })
}

fn analyze(
    root: &CrateRoot,
    cfg: &Config,
    index: Option<&LoadedIndex>,
) -> Result<CrateReport, String> {
    let Built {
        analysis,
        warnings,
        semantic_only,
    } = build(root, index)?;
    let mut violations = rules::run(&analysis, cfg);
    violations.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.rule.cmp(b.rule)));
    Ok(CrateReport {
        crate_name: analysis.crate_name.clone(),
        mode: if index.is_some() { "full" } else { "fast" },
        modules: analysis.modules.len(),
        edges: analysis.edges.iter().filter(|e| e.from != e.to).count(),
        semantic_only_edges: semantic_only,
        warnings,
        violations,
    })
}

fn graph(dir: &Path, out: &Path, opts: &CheckOpts) -> ExitCode {
    let fail = |msg: String| {
        eprintln!("edict: {msg}");
        ExitCode::from(3)
    };
    let crates = match find_crates(dir) {
        Ok(c) if c.is_empty() => return fail(format!("no crates found in {}", dir.display())),
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let index = if opts.full {
        match scip_index::obtain(dir, opts.scip) {
            Ok(i) => Some(i),
            Err(e) => return fail(e),
        }
    } else {
        None
    };
    for root in &crates {
        let cfg = match load_config(opts.config, root, dir) {
            Ok(c) => c,
            Err(e) => return fail(e),
        };
        let built = match build(root, index.as_ref()) {
            Ok(b) => b,
            Err(e) => return fail(e),
        };
        let path = if crates.len() == 1 {
            out.to_path_buf()
        } else {
            let stem = out
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("edict-graph");
            out.with_file_name(format!("{stem}-{}.html", root.name))
        };
        if let Err(e) = fs::write(&path, graph_view::render(&built.analysis, &cfg)) {
            return fail(format!("cannot write {}: {e}", path.display()));
        }
        println!("edict: wrote {}", path.display());
    }
    ExitCode::SUCCESS
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
