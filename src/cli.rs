//! Command-line entry: argument parsing, config loading, orchestration, exit codes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

use std::collections::BTreeMap;

use crate::baseline::{self, Baseline};
use crate::config::TEMPLATE;
use crate::graph_view;
use crate::model::Severity;
use crate::pipeline;
use crate::report::{self, CrateReport};

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

    /// Baseline file (default: edict-baseline.json in --path). Violations it
    /// records are tolerated; only new ones fail.
    #[arg(long, global = true, value_name = "FILE")]
    baseline: Option<PathBuf>,

    /// Ignore the baseline: report and fail on every violation.
    #[arg(long, global = true)]
    no_baseline: bool,

    /// Also list baselined violations (they still don't fail the check).
    #[arg(long, global = true)]
    show_baselined: bool,
}

struct CheckOpts<'a> {
    config: Option<&'a Path>,
    format: Format,
    full: bool,
    scip: Option<&'a Path>,
    hook: bool,
    /// Baseline file to use, or `None` to ignore baselines.
    baseline: Option<PathBuf>,
    show_baselined: bool,
}

impl CheckOpts<'_> {
    fn settings(&self) -> pipeline::Settings<'_> {
        pipeline::Settings {
            config: self.config,
            full: self.full,
            scip: self.scip,
            quiet: self.hook,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Check the crate against its laws (default).
    Check,
    /// Write a starter edict.toml.
    Init,
    /// Record current violations so only new ones fail. Re-running only
    /// drops fixed entries (a ratchet); it never adds new ones unless
    /// --accept-new.
    Baseline {
        /// Also record violations the existing baseline doesn't cover.
        #[arg(long)]
        accept_new: bool,
    },
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

    let baseline_path = args
        .baseline
        .clone()
        .unwrap_or_else(|| args.path.join(baseline::FILE_NAME));
    let opts = CheckOpts {
        config: args.config.as_deref(),
        format: args.format,
        full: args.full || args.scip.is_some(),
        scip: args.scip.as_deref(),
        hook: args.hook,
        baseline: (!args.no_baseline).then_some(baseline_path.clone()),
        show_baselined: args.show_baselined,
    };
    match args.command.unwrap_or(Command::Check) {
        Command::Init => init(&args.path),
        Command::Check => check(&args.path, &opts),
        Command::Baseline { accept_new } => {
            record_baseline(&args.path, &opts, &baseline_path, accept_new)
        }
        Command::Graph { out } => graph(
            &args.path,
            &out,
            &CheckOpts {
                hook: false,
                ..opts
            },
        ),
    }
}

fn init(dir: &Path) -> ExitCode {
    let path = dir.join(pipeline::CONFIG_FILE);
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

/// Analyze every crate under `dir` and run the laws.
fn collect(dir: &Path, opts: &CheckOpts) -> Result<Vec<CrateReport>, String> {
    Ok(pipeline::build_all(dir, &opts.settings())?
        .iter()
        .map(pipeline::report)
        .collect())
}

fn check(dir: &Path, opts: &CheckOpts) -> ExitCode {
    let fail = |msg: String| {
        eprintln!("edict: {msg}");
        ExitCode::from(if opts.hook { 2 } else { 3 })
    };
    let mut reports = match collect(dir, opts) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };

    if let Some(path) = opts.baseline.as_ref().filter(|p| p.exists()) {
        let parsed = fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))
            .and_then(|src| Baseline::parse(&src).map_err(|e| format!("{}: {e}", path.display())));
        let base = match parsed {
            Ok(b) => b,
            Err(e) => return fail(e),
        };
        let empty = baseline::Entries::new();
        for r in &mut reports {
            let entries = base.crates.get(&r.crate_name).unwrap_or(&empty);
            r.stale = baseline::apply(entries, &mut r.violations);
            r.baselined = r.violations.iter().filter(|v| v.baselined).count();
        }
    }

    let failed = reports.iter().any(|r| r.count(Severity::Error) > 0);
    let output = match opts.format {
        Format::Human => report::human(&reports, opts.show_baselined),
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

fn record_baseline(dir: &Path, opts: &CheckOpts, path: &Path, accept_new: bool) -> ExitCode {
    let fail = |msg: String| {
        eprintln!("edict: {msg}");
        ExitCode::from(3)
    };
    let reports = match collect(dir, opts) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let current: BTreeMap<String, baseline::Entries> = reports
        .iter()
        .map(|r| (r.crate_name.clone(), baseline::record(&r.violations)))
        .collect();
    let total = |b: &BTreeMap<String, baseline::Entries>| -> usize {
        b.values().flat_map(|e| e.values()).sum()
    };

    let old = if path.exists() {
        match fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|s| Baseline::parse(&s))
        {
            Ok(b) => Some(b),
            Err(e) => return fail(format!("{}: {e}", path.display())),
        }
    } else {
        None
    };

    let next = match (&old, accept_new) {
        (Some(old), false) => {
            let empty = baseline::Entries::new();
            let mut next = BTreeMap::new();
            let mut new_findings = Vec::new();
            for r in &reports {
                let (entries, added) = baseline::ratchet(
                    old.crates.get(&r.crate_name).unwrap_or(&empty),
                    &current[&r.crate_name],
                );
                for key in added {
                    if let Some(v) = r.violations.iter().find(|v| v.key == key) {
                        new_findings.push(format!(
                            "`{}`: {}[{}]: {}",
                            r.crate_name,
                            label(v.severity),
                            v.rule,
                            v.message
                        ));
                    }
                }
                if !entries.is_empty() {
                    next.insert(r.crate_name.clone(), entries);
                }
            }
            if !new_findings.is_empty() {
                eprintln!("edict: these violations aren't in the baseline, so it was not changed:");
                for f in &new_findings {
                    eprintln!("  {f}");
                }
                eprintln!("edict: fix them, or rerun with --accept-new to record them");
                return ExitCode::FAILURE;
            }
            next
        }
        _ => current.into_iter().filter(|(_, e)| !e.is_empty()).collect(),
    };

    if let Err(e) = fs::write(path, Baseline::new(next.clone()).render()) {
        return fail(format!("cannot write {}: {e}", path.display()));
    }
    match old {
        Some(old) => {
            let (before, after) = (total(&old.crates), total(&next));
            println!(
                "edict: baseline {}: {after} recorded violations ({} fixed since last time)",
                path.display(),
                before.saturating_sub(after)
            );
        }
        None => println!(
            "edict: wrote {}: {} recorded violations",
            path.display(),
            total(&next)
        ),
    }
    ExitCode::SUCCESS
}

fn label(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

fn graph(dir: &Path, out: &Path, opts: &CheckOpts) -> ExitCode {
    let built = match pipeline::build_all(dir, &opts.settings()) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("edict: {e}");
            return ExitCode::from(3);
        }
    };
    let many = built.len() > 1;
    for b in &built {
        let path = if many {
            let stem = out
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("edict-graph");
            out.with_file_name(format!("{stem}-{}.html", b.analysis.crate_name))
        } else {
            out.to_path_buf()
        };
        if let Err(e) = fs::write(&path, graph_view::render(&b.analysis, &b.config)) {
            eprintln!("edict: cannot write {}: {e}", path.display());
            return ExitCode::from(3);
        }
        println!("edict: wrote {}", path.display());
    }
    ExitCode::SUCCESS
}
