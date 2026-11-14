//! Axiom 3: code outside a top-level module only uses what that module
//! exports at its root. `crate::billing::Invoice` is fine;
//! `crate::billing::tax::rate` reaches into internals.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::Config;
use crate::model::{Analysis, Edge, ModId, Note, Violation};

const MAX_NOTES: usize = 5;

pub fn check(a: &Analysis, cfg: &Config) -> Vec<Violation> {
    let Some(severity) = cfg.facade.level.severity() else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut groups: BTreeMap<(ModId, ModId), Vec<&Edge>> = BTreeMap::new();

    for e in &a.edges {
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

    groups
        .into_iter()
        .map(|((from, into), edges)| {
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
                severity,
                message: format!(
                    "`{}` reaches into `{}`; use what `{}` exports at its root",
                    a.name(from),
                    a.name(into),
                    a.name(target_top)
                ),
                notes,
            }
        })
        .collect()
}
