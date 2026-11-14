//! Axiom 1: the module dependency graph is a DAG.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::{Config, Granularity};
use crate::graph::{sccs, shortest_cycle};
use crate::model::{Analysis, Edge, ModId, Note, ROOT, Violation};

pub fn check(a: &Analysis, cfg: &Config) -> Vec<Violation> {
    let Some(severity) = cfg.cycles.level.severity() else {
        return Vec::new();
    };
    let node_of = |m: ModId| match cfg.cycles.granularity {
        Granularity::File => a.file_module_of(m),
        Granularity::Top => a.top_of(m).unwrap_or(ROOT),
    };

    let n = a.modules.len();
    let mut adj = vec![BTreeSet::new(); n];
    let mut witness: BTreeMap<(ModId, ModId), &Edge> = BTreeMap::new();
    for e in a.edges.iter().filter(|e| !a.is_barrel(e)) {
        let (f, t) = (node_of(e.from), node_of(e.to));
        if f != t {
            adj[f].insert(t);
            witness.entry((f, t)).or_insert(e);
        }
    }

    let mut out = Vec::new();
    for comp in sccs(n, &adj).into_iter().filter(|c| c.len() > 1) {
        let members: BTreeSet<ModId> = comp.iter().copied().collect();
        let cycle = shortest_cycle(&adj, &members, comp[0]);
        let names: Vec<String> = cycle.iter().map(|&m| a.name(m)).collect();
        let mut message = format!("module cycle: {}", names.join(" -> "));
        if comp.len() > cycle.len() - 1 {
            let all: Vec<String> = comp.iter().map(|&m| a.name(m)).collect();
            message.push_str(&format!(
                " ({} modules are entangled: {})",
                comp.len(),
                all.join(", ")
            ));
        }
        let notes = cycle
            .windows(2)
            .filter_map(|w| witness.get(&(w[0], w[1])))
            .map(|e| Note {
                loc: e.loc.clone(),
                message: format!("`{}` uses `{}`", a.name(e.from), e.path),
            })
            .collect();
        out.push(Violation {
            rule: "cycles",
            severity,
            message,
            notes,
        });
    }
    out
}
