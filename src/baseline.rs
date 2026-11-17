//! Baselines: record today's violations so only new ones fail, and only ever
//! let the record shrink (a ratchet). Pure: parses and renders strings.
//!
//! Matching is by each violation's stable `key` (no line numbers), counted:
//! two identical findings need two baseline entries. Cycles match by
//! membership: a cycle is known if its modules are a subset of a recorded
//! cycle's, so breaking part of a tangle never reads as a new violation.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::model::Violation;

pub const FILE_NAME: &str = "edict-baseline.json";
const VERSION: u32 = 1;
const CYCLE_PREFIX: &str = "cycles:";

/// Recorded violations per crate: key -> count.
pub type Entries = BTreeMap<String, usize>;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Baseline {
    pub version: u32,
    pub crates: BTreeMap<String, Entries>,
}

impl Baseline {
    pub fn parse(src: &str) -> Result<Baseline, String> {
        let b: Baseline =
            serde_json::from_str(src).map_err(|e| format!("invalid baseline: {e}"))?;
        if b.version != VERSION {
            return Err(format!(
                "unsupported baseline version {} (expected {VERSION})",
                b.version
            ));
        }
        Ok(b)
    }

    pub fn render(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("baseline serializes");
        s.push('\n');
        s
    }

    pub fn new(crates: BTreeMap<String, Entries>) -> Baseline {
        Baseline {
            version: VERSION,
            crates,
        }
    }
}

/// Today's violations as baseline entries.
pub fn record(violations: &[Violation]) -> Entries {
    let mut out = Entries::new();
    for v in violations {
        *out.entry(v.key.clone()).or_default() += 1;
    }
    out
}

fn cycle_members(key: &str) -> Option<BTreeSet<&str>> {
    key.strip_prefix(CYCLE_PREFIX)
        .map(|m| m.split(',').collect())
}

/// Is cycle `key` covered by some recorded cycle (same or fewer members)?
fn covered_cycle(entries: &Entries, key: &str) -> Option<String> {
    let members = cycle_members(key)?;
    entries
        .keys()
        .find(|k| cycle_members(k).is_some_and(|recorded| members.is_subset(&recorded)))
        .cloned()
}

/// Mark violations the baseline already knows about. Returns how many
/// recorded entries no longer occur (fixed since the baseline was taken).
pub fn apply(entries: &Entries, violations: &mut [Violation]) -> usize {
    let mut remaining = entries.clone();
    let mut cycles_seen = BTreeSet::new();
    for v in violations.iter_mut() {
        if v.key.starts_with(CYCLE_PREFIX) {
            if let Some(recorded) = covered_cycle(entries, &v.key) {
                v.baselined = true;
                cycles_seen.insert(recorded);
            }
        } else if let Some(n) = remaining.get_mut(&v.key).filter(|n| **n > 0) {
            *n -= 1;
            v.baselined = true;
        }
    }
    let stale_cycles = entries
        .keys()
        .filter(|k| k.starts_with(CYCLE_PREFIX) && !cycles_seen.contains(*k))
        .count();
    let stale_other: usize = remaining
        .iter()
        .filter(|(k, _)| !k.starts_with(CYCLE_PREFIX))
        .map(|(_, n)| n)
        .sum();
    stale_cycles + stale_other
}

/// The ratchet: a new baseline from the old one and today's entries that keeps
/// only what still occurs and never grows. Returns the new entries and the
/// keys that today has but the old baseline didn't cover.
pub fn ratchet(old: &Entries, current: &Entries) -> (Entries, Vec<String>) {
    let mut next = Entries::new();
    let mut added = Vec::new();
    for (key, &n) in current {
        if key.starts_with(CYCLE_PREFIX) {
            if covered_cycle(old, key).is_some() {
                next.insert(key.clone(), n);
            } else {
                added.push(key.clone());
            }
        } else {
            let allowed = old.get(key).copied().unwrap_or(0);
            if allowed > 0 {
                next.insert(key.clone(), n.min(allowed));
            }
            if n > allowed {
                added.push(key.clone());
            }
        }
    }
    (next, added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Severity;

    fn v(key: &str) -> Violation {
        Violation {
            rule: "x",
            key: key.into(),
            severity: Severity::Error,
            message: key.into(),
            notes: Vec::new(),
            baselined: false,
        }
    }

    #[test]
    fn counts_must_match() {
        let base = record(&[v("purity:a:std::fs::read")]);
        let mut now = vec![v("purity:a:std::fs::read"), v("purity:a:std::fs::read")];
        assert_eq!(apply(&base, &mut now), 0);
        assert!(
            now[0].baselined && !now[1].baselined,
            "second occurrence is new"
        );
    }

    #[test]
    fn shrinking_cycle_stays_known_growing_one_does_not() {
        let base = record(&[v("cycles:a,b,c")]);
        let mut shrunk = vec![v("cycles:a,b")];
        assert_eq!(apply(&base, &mut shrunk), 0);
        assert!(shrunk[0].baselined);
        let mut grown = vec![v("cycles:a,b,c,d")];
        apply(&base, &mut grown);
        assert!(!grown[0].baselined);
    }

    #[test]
    fn reports_fixed_entries() {
        let base = record(&[v("layers:a->b"), v("cycles:x,y")]);
        let mut now = vec![];
        assert_eq!(apply(&base, &mut now), 2);
    }

    #[test]
    fn ratchet_only_shrinks() {
        let old = record(&[v("k"), v("k"), v("cycles:a,b,c"), v("gone")]);
        let current = record(&[v("k"), v("cycles:a,b"), v("new")]);
        let (next, added) = ratchet(&old, &current);
        assert_eq!(next, record(&[v("k"), v("cycles:a,b")]));
        assert_eq!(added, vec!["new".to_string()]);
    }

    #[test]
    fn round_trips() {
        let b = Baseline::new([("c".to_string(), record(&[v("k")]))].into());
        assert_eq!(Baseline::parse(&b.render()).unwrap().crates, b.crates);
    }
}
