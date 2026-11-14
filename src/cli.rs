//! Command-line entry: argument parsing, config loading, orchestration, exit codes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::config::{Config, TEMPLATE};
use crate::extract::extract;
use crate::load::{CrateRoot, find_crates, load_crate};
use crate::model::{Analysis, Severity};
use crate::report::{self, CrateReport};
use crate::resolve::resolve;
use crate::rules;

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
}

#[derive(Subcommand)]
enum Command {
    /// Check the crate against its laws (default).
    Check,
    /// Write a starter edict.toml.
    Init,
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
        Command::Check => check(&args.path, args.config.as_deref(), args.format),
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

fn check(dir: &Path, explicit_config: Option<&Path>, format: Format) -> ExitCode {
    let crates = match find_crates(dir) {
        Ok(c) if c.is_empty() => {
            eprintln!("edict: no crates found in {}", dir.display());
            return ExitCode::from(2);
        }
        Ok(c) => c,
        Err(e) => {
            eprintln!("edict: {e}");
            return ExitCode::from(2);
        }
    };

    let mut reports = Vec::new();
    for root in &crates {
        let cfg = match load_config(explicit_config, root, dir) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("edict: {e}");
                return ExitCode::from(2);
            }
        };
        match analyze(root, &cfg) {
            Ok(r) => reports.push(r),
            Err(e) => {
                eprintln!("edict: {e}");
                return ExitCode::from(2);
            }
        }
    }

    let output = match format {
        Format::Human => report::human(&reports),
        Format::Json => report::json(&reports),
    };
    println!("{output}");

    if reports.iter().any(|r| r.count(Severity::Error) > 0) {
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

fn analyze(root: &CrateRoot, cfg: &Config) -> Result<CrateReport, String> {
    let loaded = load_crate(root)?;
    let ex = extract(&loaded.modules, &loaded.items);
    let (edges, externs) = resolve(&root.name, &loaded.modules, &ex.exports, &ex.raw);
    let analysis = Analysis {
        crate_name: root.name.clone(),
        modules: loaded.modules,
        edges,
        externs,
        statics: ex.statics,
        macros: ex.macros,
    };
    let mut violations = rules::run(&analysis, cfg);
    violations.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.rule.cmp(b.rule)));
    Ok(CrateReport {
        crate_name: analysis.crate_name.clone(),
        modules: analysis.modules.len(),
        edges: analysis.edges.iter().filter(|e| e.from != e.to).count(),
        warnings: loaded.warnings,
        violations,
    })
}
