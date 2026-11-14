//! Soft limits: file length and fan-out. Warnings by default.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::Config;
use crate::model::{Analysis, Loc, ModId, Note, Violation};

pub fn check(a: &Analysis, cfg: &Config) -> Vec<Violation> {
    let Some(severity) = cfg.limits.level.severity() else {
        return Vec::new();
    };
    let mut out = Vec::new();

    for m in a
        .modules
        .iter()
        .filter(|m| !m.inline && m.lines > cfg.limits.max_lines)
    {
        out.push(Violation {
            rule: "limits",
            severity,
            message: format!(
                "`{}` has {} lines (max {})",
                m.display(),
                m.lines,
                cfg.limits.max_lines
            ),
            notes: vec![Note {
                loc: Loc {
                    file: m.file.clone(),
                    line: 1,
                },
                message: "consider splitting this file".into(),
            }],
        });
    }

    let mut deps: BTreeMap<ModId, BTreeSet<ModId>> = BTreeMap::new();
    for e in a.edges.iter().filter(|e| !a.is_barrel(e)) {
        let (f, t) = (a.file_module_of(e.from), a.file_module_of(e.to));
        if f != t {
            deps.entry(f).or_default().insert(t);
        }
    }
    for (m, targets) in deps
        .into_iter()
        .filter(|(_, t)| t.len() > cfg.limits.max_fan_out)
    {
        let names: Vec<String> = targets.iter().map(|&t| a.name(t)).collect();
        out.push(Violation {
            rule: "limits",
            severity,
            message: format!(
                "`{}` depends on {} modules (max {}): {}",
                a.name(m),
                targets.len(),
                cfg.limits.max_fan_out,
                names.join(", ")
            ),
            notes: vec![Note {
                loc: Loc {
                    file: a.modules[m].file.clone(),
                    line: 1,
                },
                message: "high fan-out".into(),
            }],
        });
    }
    out
}
