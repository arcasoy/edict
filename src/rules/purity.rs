//! Axiom 4: layers marked `pure` touch no I/O, clocks, randomness or runtimes.

use crate::config::Config;
use crate::model::{Analysis, Note, Violation};

pub fn check(a: &Analysis, cfg: &Config) -> Vec<Violation> {
    let Some(severity) = cfg.purity.level.severity() else {
        return Vec::new();
    };
    if !cfg.layer.iter().any(|l| l.pure) {
        return Vec::new();
    }
    let deny = cfg.purity.deny_list();
    let mut out = Vec::new();
    // Both engines can report the same call; report it once.
    let mut seen = std::collections::BTreeSet::new();

    for x in &a.externs {
        let Some(top) = a.top_of(x.from) else {
            continue;
        };
        let Some(li) = cfg.layer_of(&a.name(top)) else {
            continue;
        };
        let layer = &cfg.layer[li];
        if !layer.pure {
            continue;
        }
        let hit = deny
            .iter()
            .find(|d| x.path == **d || x.path.starts_with(&format!("{d}::")));
        if let Some(d) = hit.filter(|_| seen.insert((x.from, x.path.clone(), x.loc.line))) {
            out.push(Violation {
                rule: "purity",
                severity,
                message: format!(
                    "`{}` is in pure layer `{}` but uses `{}` (denied: `{d}`)",
                    a.name(x.from),
                    layer.name,
                    x.path
                ),
                notes: vec![Note {
                    loc: x.loc.clone(),
                    message: "pass this in from an outer layer instead".into(),
                }],
            });
        }
    }
    out
}
