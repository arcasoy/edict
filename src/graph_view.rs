//! `cargo edict graph`: renders the module graph as one self-contained,
//! interactive HTML page. Pure: returns the page as a string.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::config::Config;
use crate::graph::sccs;
use crate::model::{Analysis, ModId, ROOT, Source};

const TEMPLATE: &str = include_str!("graph_view.html");
const MAX_SAMPLES: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Top,
    File,
}

pub fn render(a: &Analysis, cfg: &Config) -> String {
    let layers: Vec<Value> = cfg
        .layer
        .iter()
        .enumerate()
        .map(|(i, l)| json!({ "name": l.name, "index": i, "pure": l.pure }))
        .collect();
    let data = json!({
        "crate": a.crate_name,
        "layers": layers,
        "views": { "top": view(a, cfg, View::Top), "file": view(a, cfg, View::File) },
    });
    // Keep `</script>` in module paths from closing the script tag.
    let json = data.to_string().replace("</", "<\\/");
    TEMPLATE.replace("/*__EDICT_DATA__*/null", &json)
}

fn view(a: &Analysis, cfg: &Config, v: View) -> Value {
    let node_of = |m: ModId| -> ModId {
        match v {
            View::File => a.file_module_of(m),
            View::Top => a.top_of(m).unwrap_or(ROOT),
        }
    };
    let ids: Vec<ModId> = (0..a.modules.len())
        .filter(|&m| node_of(m) == m)
        .filter(|&m| v == View::File || m == ROOT || a.modules[m].parent == Some(ROOT))
        .collect();
    let index: BTreeMap<ModId, usize> = ids.iter().enumerate().map(|(i, &m)| (m, i)).collect();

    // Aggregate edges between view nodes.
    struct Agg {
        count: usize,
        all_semantic: bool,
        /// Distinct paths: first location and how often each is used.
        paths: BTreeMap<String, (String, usize, usize)>,
    }
    let mut agg: BTreeMap<(usize, usize), Agg> = BTreeMap::new();
    for e in a.edges.iter().filter(|e| !a.is_barrel(e)) {
        let (Some(&f), Some(&t)) = (index.get(&node_of(e.from)), index.get(&node_of(e.to))) else {
            continue;
        };
        if f == t {
            continue;
        }
        let entry = agg.entry((f, t)).or_insert(Agg {
            count: 0,
            all_semantic: true,
            paths: BTreeMap::new(),
        });
        entry.count += 1;
        entry.all_semantic &= e.source == Source::Semantic;
        let slot = entry
            .paths
            .entry(e.path.clone())
            .or_insert_with(|| (e.loc.file.display().to_string(), e.loc.line, 0));
        slot.2 += 1;
    }

    let n = ids.len();
    let mut adj = vec![BTreeSet::new(); n];
    for &(f, t) in agg.keys() {
        adj[f].insert(t);
    }
    let comps = sccs(n, &adj);
    let mut scc_of = vec![0usize; n];
    for (ci, comp) in comps.iter().enumerate() {
        for &m in comp {
            scc_of[m] = ci;
        }
    }
    let levels = levels(&adj, &scc_of, comps.len());

    let layer_of =
        |m: ModId| -> Option<usize> { a.top_of(m).and_then(|t| cfg.layer_of(&a.name(t))) };
    let allowed = cfg.allowed_layers();

    let nodes: Vec<Value> = ids
        .iter()
        .enumerate()
        .map(|(i, &m)| {
            let module = &a.modules[m];
            let lines: usize = match v {
                View::File => module.lines,
                View::Top => (0..a.modules.len())
                    .filter(|&x| {
                        !a.modules[x].inline && (x == m || (m != ROOT && a.is_descendant(x, m)))
                    })
                    .map(|x| a.modules[x].lines)
                    .sum(),
            };
            json!({
                "id": i,
                "name": a.name(m),
                "file": module.file.display().to_string(),
                "lines": lines,
                "layer": layer_of(m),
                "level": levels[i],
                "cycle": comps[scc_of[i]].len() > 1,
            })
        })
        .collect();

    let edges: Vec<Value> = agg
        .into_iter()
        .map(|((f, t), g)| {
            let cycle = scc_of[f] == scc_of[t];
            let layer_violation = match (layer_of(ids[f]), layer_of(ids[t])) {
                (Some(lf), Some(lt)) => lf != lt && !allowed[lf].contains(&lt),
                _ => false,
            };
            json!({
                "from": f,
                "to": t,
                "count": g.count,
                "cycle": cycle,
                "layerViolation": layer_violation,
                "semanticOnly": g.all_semantic,
                "distinct": g.paths.len(),
                "samples": samples(&g.paths),
            })
        })
        .collect();

    json!({ "nodes": nodes, "edges": edges })
}

/// The most-used distinct paths, each with its first location.
fn samples(paths: &BTreeMap<String, (String, usize, usize)>) -> Vec<Value> {
    let mut list: Vec<_> = paths.iter().collect();
    list.sort_by(|a, b| b.1.2.cmp(&a.1.2).then_with(|| a.0.cmp(b.0)));
    list.into_iter()
        .take(MAX_SAMPLES)
        .map(|(path, (file, line, uses))| json!({ "path": path, "file": file, "line": line, "uses": uses }))
        .collect()
}

/// Dependency depth: modules that depend on nothing are level 0; a module sits
/// one level above the deepest thing it depends on. Cycles share a level.
fn levels(adj: &[BTreeSet<usize>], scc_of: &[usize], n_comps: usize) -> Vec<usize> {
    let mut cadj = vec![BTreeSet::new(); n_comps];
    for (f, targets) in adj.iter().enumerate() {
        for &t in targets {
            if scc_of[f] != scc_of[t] {
                cadj[scc_of[f]].insert(scc_of[t]);
            }
        }
    }
    let mut memo: Vec<Option<usize>> = vec![None; n_comps];
    fn depth(c: usize, cadj: &[BTreeSet<usize>], memo: &mut Vec<Option<usize>>) -> usize {
        if let Some(d) = memo[c] {
            return d;
        }
        let d = cadj[c]
            .iter()
            .map(|&x| depth(x, cadj, memo) + 1)
            .max()
            .unwrap_or(0);
        memo[c] = Some(d);
        d
    }
    scc_of.iter().map(|&c| depth(c, &cadj, &mut memo)).collect()
}
