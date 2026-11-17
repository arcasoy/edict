//! Renders results as human-readable text or JSON. Pure: returns strings.

use serde::Serialize;

use crate::model::{Severity, Violation};

#[derive(Debug, Serialize)]
pub struct CrateReport {
    #[serde(rename = "crate")]
    pub crate_name: String,
    /// `fast` (syntax engine) or `full` (syntax + rust-analyzer).
    pub mode: &'static str,
    pub modules: usize,
    pub edges: usize,
    /// Full mode: file-to-file dependencies only rust-analyzer found.
    pub semantic_only_edges: Option<usize>,
    /// Violations the baseline records (reported but not failing).
    pub baselined: usize,
    /// Baseline entries that no longer occur: fixed since it was recorded.
    pub stale: usize,
    pub warnings: Vec<String>,
    pub violations: Vec<Violation>,
}

impl CrateReport {
    /// New (not baselined) violations of this severity.
    pub fn count(&self, s: Severity) -> usize {
        self.violations
            .iter()
            .filter(|v| v.severity == s && !v.baselined)
            .count()
    }
}

pub fn human(reports: &[CrateReport], show_baselined: bool) -> String {
    let mut out = String::new();
    let (mut errors, mut warnings) = (0, 0);

    for r in reports {
        for w in &r.warnings {
            out.push_str(&format!("note: {w}\n"));
        }
        for v in r
            .violations
            .iter()
            .filter(|v| show_baselined || !v.baselined)
        {
            let label = match v.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            let known = if v.baselined { " (baselined)" } else { "" };
            out.push_str(&format!("{label}[{}]{known}: {}\n", v.rule, v.message));
            for n in &v.notes {
                out.push_str(&format!(
                    "  --> {}:{}: {}\n",
                    n.loc.file.display(),
                    n.loc.line,
                    n.message
                ));
            }
            out.push('\n');
        }
        let (e, w) = (r.count(Severity::Error), r.count(Severity::Warning));
        errors += e;
        warnings += w;
        let mut extra = match r.semantic_only_edges {
            Some(n) => format!(", {n} found only by rust-analyzer"),
            None => String::new(),
        };
        if r.baselined > 0 {
            extra.push_str(&format!("; {} baselined", r.baselined));
        }
        out.push_str(&format!(
            "edict ({}): `{}`: {} error{}, {} warning{} ({} modules, {} edges{extra})\n",
            r.mode,
            r.crate_name,
            e,
            plural(e),
            w,
            plural(w),
            r.modules,
            r.edges
        ));
        if r.stale > 0 {
            out.push_str(&format!(
                "edict: `{}`: {} baselined violation{} fixed; run `cargo edict baseline` to lock that in\n",
                r.crate_name,
                r.stale,
                if r.stale == 1 { " is" } else { "s are" }
            ));
        }
    }
    if reports.len() > 1 {
        out.push_str(&format!(
            "edict: total: {errors} error{}, {warnings} warning{}\n",
            plural(errors),
            plural(warnings)
        ));
    }
    out
}

pub fn json(reports: &[CrateReport]) -> String {
    serde_json::to_string_pretty(reports).unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"))
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}
