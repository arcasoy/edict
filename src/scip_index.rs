//! Obtains a SCIP index: reads one from disk, or runs `rust-analyzer scip`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use protobuf::Message;
use scip::types::Index;

pub struct LoadedIndex {
    pub index: Index,
    /// Absolute directory the index's document paths are relative to.
    pub root: PathBuf,
}

/// Read `explicit` if given; otherwise index `workspace` with rust-analyzer.
pub fn obtain(workspace: &Path, explicit: Option<&Path>) -> Result<LoadedIndex, String> {
    let bytes = match explicit {
        Some(path) => {
            fs::read(path).map_err(|e| format!("cannot read SCIP index {}: {e}", path.display()))?
        }
        None => run_rust_analyzer(workspace)?,
    };
    let index = Index::parse_from_bytes(&bytes).map_err(|e| format!("invalid SCIP index: {e}"))?;
    let root_uri = index.metadata.project_root.clone();
    let root = match root_uri.strip_prefix("file://") {
        Some(p) => PathBuf::from(p),
        None if explicit.is_none() => canonical(workspace)?,
        None => {
            return Err(format!(
                "SCIP index has an unsupported project_root `{root_uri}`"
            ));
        }
    };
    Ok(LoadedIndex { index, root })
}

fn run_rust_analyzer(workspace: &Path) -> Result<Vec<u8>, String> {
    let out = std::env::temp_dir().join(format!("edict-{}.scip", std::process::id()));
    // rust-analyzer logs heavily (and reports its own internal bugs) on
    // stderr even with -q. Keep it quiet unless indexing fails.
    let run = Command::new("rust-analyzer")
        .arg("-q")
        .arg("scip")
        .arg(workspace)
        .arg("--output")
        .arg(&out)
        .output()
        .map_err(|e| {
            format!("cannot run rust-analyzer ({e}); install it with `rustup component add rust-analyzer`, or pass --scip <index>")
        })?;
    if !run.status.success() {
        let log = String::from_utf8_lossy(&run.stderr);
        let tail: Vec<&str> = log.lines().rev().take(20).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        return Err(format!(
            "`rust-analyzer scip` failed ({}):\n{}",
            run.status,
            tail.join("\n")
        ));
    }
    let bytes = fs::read(&out).map_err(|e| format!("cannot read {}: {e}", out.display()));
    let _ = fs::remove_file(&out);
    bytes
}

pub fn canonical(p: &Path) -> Result<PathBuf, String> {
    p.canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", p.display()))
}
