//! Turns raw path references into module-to-module edges, following
//! re-exports to the module that actually defines each item. Pure.

use std::collections::BTreeSet;

use crate::model::{Edge, Exports, ExternRef, ModId, Module, ROOT, RawRef, Source};

const MAX_DEPTH: usize = 32;

pub fn resolve(
    crate_name: &str,
    modules: &[Module],
    exports: &[Exports],
    raw: &[RawRef],
) -> (Vec<Edge>, Vec<ExternRef>) {
    let r = Resolver { modules, exports };
    let mut edges = Vec::new();
    let mut externs = Vec::new();

    for rr in raw {
        let first = rr.path[0].as_str();
        if first != "crate" && first != crate_name {
            externs.push(ExternRef {
                from: rr.from,
                path: rr.path.join("::"),
                loc: rr.loc.clone(),
            });
            continue;
        }
        let segs = &rr.path[1..];
        let (written_to, rest) = r.walk(ROOT, segs);
        let defined_at = rest
            .first()
            .and_then(|name| r.definer(written_to, name, &mut BTreeSet::new(), 0));
        if rr.candidate && !rest.is_empty() && defined_at.is_none() {
            continue; // glob guess that the glob source doesn't actually provide
        }
        let to = defined_at.unwrap_or(written_to);
        let mut display = vec!["crate".to_string()];
        display.extend(segs.iter().cloned());
        edges.push(Edge {
            from: rr.from,
            to,
            written_to,
            kind: rr.kind,
            path: display.join("::"),
            loc: rr.loc.clone(),
            source: Source::Syntax,
        });
    }
    (edges, externs)
}

struct Resolver<'a> {
    modules: &'a [Module],
    exports: &'a [Exports],
}

impl Resolver<'_> {
    /// Follow module names from `start` as far as they go. Returns the deepest
    /// module reached and the remaining (non-module) segments.
    fn walk<'s>(&self, start: ModId, segs: &'s [String]) -> (ModId, &'s [String]) {
        let mut cur = start;
        let mut i = 0;
        while let Some(&child) = segs.get(i).and_then(|s| self.modules[cur].children.get(s)) {
            cur = child;
            i += 1;
        }
        (cur, &segs[i..])
    }

    /// Module that defines `name` as seen from module `m`, following
    /// named and glob re-exports.
    fn definer(
        &self,
        m: ModId,
        name: &str,
        seen: &mut BTreeSet<(ModId, String)>,
        depth: usize,
    ) -> Option<ModId> {
        if depth > MAX_DEPTH || !seen.insert((m, name.to_string())) {
            return None;
        }
        let ex = &self.exports[m];
        if ex.defined.contains(name) {
            return Some(m);
        }
        if let Some(re) = ex
            .reexports
            .iter()
            .find(|re| re.name.as_deref() == Some(name))
        {
            if re.target.first().map(String::as_str) != Some("crate") {
                return None; // re-export of an external item
            }
            let (target_mod, rest) = self.walk(ROOT, &re.target[1..]);
            return match rest.first() {
                None => Some(target_mod),
                Some(item) => self
                    .definer(target_mod, item, seen, depth + 1)
                    .or(Some(target_mod)),
            };
        }
        for re in ex.reexports.iter().filter(|re| re.name.is_none()) {
            if re.target.first().map(String::as_str) != Some("crate") {
                continue;
            }
            let (glob_mod, rest) = self.walk(ROOT, &re.target[1..]);
            if rest.is_empty() {
                if let Some(found) = self.definer(glob_mod, name, seen, depth + 1) {
                    return Some(found);
                }
            }
        }
        None
    }
}
