//! Axiom 2: dependencies only point down the declared layers.

use std::collections::BTreeMap;

use crate::config::Config;
use crate::model::{Analysis, Edge, Loc, ModId, Note, ROOT, Violation};

const MAX_NOTES: usize = 5;

pub fn check(a: &Analysis, cfg: &Config) -> Vec<Violation> {
    let Some(severity) = cfg.layers.level.severity() else {
        return Vec::new();
    };
    if cfg.layer.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let tops = &a.modules[ROOT].children;

    for layer in &cfg.layer {
        for m in layer.modules.iter().filter(|m| !tops.contains_key(*m)) {
            out.push(Violation {
                rule: "layers",
                severity,
                message: format!(
                    "layer `{}` lists module `{m}`, which is not a top-level module of this crate",
                    layer.name
                ),
                notes: Vec::new(),
            });
        }
    }

    if cfg.layers.require_all {
        for (name, &id) in tops {
            if cfg.layer_of(name).is_none() {
                out.push(Violation {
                    rule: "layers",
                    severity,
                    message: format!("top-level module `{name}` is not assigned to any layer"),
                    notes: vec![Note {
                        loc: Loc {
                            file: a.modules[id].file.clone(),
                            line: 1,
                        },
                        message: "declared here".into(),
                    }],
                });
            }
        }
    }

    let allowed = cfg.allowed_layers();
    let mut groups: BTreeMap<(ModId, ModId), Vec<&Edge>> = BTreeMap::new();
    for e in a.edges.iter().filter(|e| !a.is_barrel(e)) {
        let (Some(ft), Some(tt)) = (a.top_of(e.from), a.top_of(e.to)) else {
            continue;
        };
        if ft == tt {
            continue;
        }
        let (Some(lf), Some(lt)) = (cfg.layer_of(&a.name(ft)), cfg.layer_of(&a.name(tt))) else {
            continue;
        };
        if lf != lt && !allowed[lf].contains(&lt) {
            groups.entry((ft, tt)).or_default().push(e);
        }
    }

    for ((ft, tt), edges) in groups {
        let lf = &cfg.layer[cfg.layer_of(&a.name(ft)).expect("grouped above")].name;
        let lt = &cfg.layer[cfg.layer_of(&a.name(tt)).expect("grouped above")].name;
        let mut notes: Vec<Note> = edges
            .iter()
            .take(MAX_NOTES)
            .map(|e| Note {
                loc: e.loc.clone(),
                message: format!("`{}` uses `{}`", a.name(e.from), e.path),
            })
            .collect();
        if edges.len() > MAX_NOTES {
            let last = edges[MAX_NOTES].loc.clone();
            notes.push(Note {
                loc: last,
                message: format!("...and {} more", edges.len() - MAX_NOTES),
            });
        }
        out.push(Violation {
            rule: "layers",
            severity,
            message: format!(
                "`{}` (layer `{lf}`) depends on `{}` (layer `{lt}`), but `{lf}` may not depend on `{lt}`",
                a.name(ft),
                a.name(tt)
            ),
            notes,
        });
    }
    out
}
