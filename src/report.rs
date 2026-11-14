//! Renders results as human-readable text or JSON. Pure: returns strings.

use serde::Serialize;

use crate::model::{Severity, Violation};

#[derive(Debug, Serialize)]
pub struct CrateReport {
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub modules: usize,
    pub edges: usize,
    pub warnings: Vec<String>,
    pub violations: Vec<Violation>,
}

impl CrateReport {
    pub fn count(&self, s: Severity) -> usize {
        self.violations.iter().filter(|v| v.severity == s).count()
    }
}

pub fn human(reports: &[CrateReport]) -> String {
    let mut out = String::new();
    let (mut errors, mut warnings) = (0, 0);

    for r in reports {
        for w in &r.warnings {
            out.push_str(&format!("note: {w}\n"));
        }
        for v in &r.violations {
            let label = match v.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            out.push_str(&format!("{label}[{}]: {}\n", v.rule, v.message));
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
        out.push_str(&format!(
            "edict: `{}`: {} error{}, {} warning{} ({} modules, {} edges)\n",
            r.crate_name,
            e,
            plural(e),
            w,
            plural(w),
            r.modules,
            r.edges
        ));
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
