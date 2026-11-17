//! Axiom 3: a top-level module is a facade.
//!
//! - Every module inside a top-level module stays private to it: `mod tax;`,
//!   never `pub(crate) mod tax;`. The compiler then rejects reach-ins.
//! - Code outside only uses what the top-level module exports at its root:
//!   `crate::billing::Invoice` is fine, `crate::billing::tax::rate` is not.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::Config;
use crate::model::{Analysis, Edge, ModId, Note, Scope, Source, Violation};

const MAX_NOTES: usize = 5;

pub fn check(a: &Analysis, cfg: &Config) -> Vec<Violation> {
    let Some(severity) = cfg.facade.level.severity() else {
        return Vec::new();
    };
    let mut out = visibility(a, cfg, severity);
    let mut seen = BTreeSet::new();
    let mut groups: BTreeMap<(ModId, ModId), Vec<&Edge>> = BTreeMap::new();

    // Facades are about how a path is written, which only the syntax engine sees.
    for e in a.edges.iter().filter(|e| e.source == Source::Syntax) {
        let Some(from_top) = a.top_of(e.from) else {
            continue;
        }; // crate root is the composition root
        let Some(target_top) = a.top_of(e.written_to) else {
            continue;
        };
        if from_top == target_top || e.written_to == target_top {
            continue;
        }
        if seen.insert((e.from, e.path.clone(), e.loc.line)) {
            groups.entry((e.from, e.written_to)).or_default().push(e);
        }
    }

    out.extend(groups.into_iter().map(|((from, into), edges)| {
        let target_top = a.top_of(into).expect("grouped above");
        let mut notes: Vec<Note> = edges
            .iter()
            .take(MAX_NOTES)
            .map(|e| Note {
                loc: e.loc.clone(),
                message: format!("uses `{}`", e.path),
            })
            .collect();
        if edges.len() > MAX_NOTES {
            let loc = edges[MAX_NOTES].loc.clone();
            notes.push(Note {
                loc,
                message: format!("...and {} more", edges.len() - MAX_NOTES),
            });
        }
        Violation {
            rule: "facade",
            key: format!("facade:reach:{}->{}", a.name(from), a.name(into)),
            baselined: false,
            severity,
            message: format!(
                "`{}` reaches into `{}`; use what `{}` exports at its root",
                a.name(from),
                a.name(into),
                a.name(target_top)
            ),
            notes,
        }
    }));
    out
}

/// Modules below a top-level module must not be declared visible outside it.
fn visibility(a: &Analysis, cfg: &Config, severity: crate::model::Severity) -> Vec<Violation> {
    let mut out = Vec::new();
    for (m, module) in a.modules.iter().enumerate() {
        let Some(top) = a.top_of(m) else { continue };
        if m == top {
            continue; // top-level modules themselves may have any visibility
        }
        let Some(decl) = module.decl.clone() else {
            continue;
        };
        let leaks = match a.scope_of(m) {
            Some(Scope::External) => !(a.is_lib && cfg.facade.allow_public_modules),
            Some(Scope::Within(s)) => s != top && !a.is_descendant(s, top),
            None => false, // unresolvable `pub(in ...)`: the compiler will complain
        };
        if leaks {
            out.push(Violation {
                rule: "facade",
                key: format!("facade:visibility:{}", a.name(m)),
                baselined: false,
                severity,
                message: format!(
                    "`{}` is declared `{}`, making it visible outside `{}`",
                    a.name(m),
                    module.vis.keyword(),
                    a.name(top)
                ),
                notes: vec![Note {
                    loc: decl,
                    message: format!(
                        "make it private (or pub(super) within `{}`) and re-export what callers need with `pub use` at `{}`'s root",
                        a.name(top),
                        a.name(top)
                    ),
                }],
            });
        }
    }
    out
}
