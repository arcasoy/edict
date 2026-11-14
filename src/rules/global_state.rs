//! Axiom 5: no global mutable state outside explicitly allowed modules.

use crate::config::Config;
use crate::model::{Analysis, Note, Violation};

const INTERIOR_MUTABILITY: &[&str] = &[
    "Mutex",
    "RwLock",
    "RefCell",
    "Cell",
    "UnsafeCell",
    "SyncUnsafeCell",
];

fn mutable_marker(tokens: &str) -> Option<&str> {
    tokens
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .find(|t| INTERIOR_MUTABILITY.contains(t) || t.starts_with("Atomic"))
}

pub fn check(a: &Analysis, cfg: &Config) -> Vec<Violation> {
    let Some(severity) = cfg.global_state.level.severity() else {
        return Vec::new();
    };
    let allowed = |m| {
        let name = a.name(m);
        cfg.global_state
            .allow
            .iter()
            .any(|p| name == *p || name.starts_with(&format!("{p}::")))
    };
    let mut out = Vec::new();

    for s in a.statics.iter().filter(|s| !allowed(s.module)) {
        let why = if s.is_mut {
            Some("is `static mut`".to_string())
        } else {
            mutable_marker(&s.ty).map(|m| format!("holds `{m}` (interior mutability)"))
        };
        if let Some(why) = why {
            out.push(Violation {
                rule: "global-state",
                severity,
                message: format!(
                    "global mutable state: static `{}` in `{}` {why}",
                    s.name,
                    a.name(s.module)
                ),
                notes: vec![Note {
                    loc: s.loc.clone(),
                    message: "pass state explicitly, or allow this module in [global_state]".into(),
                }],
            });
        }
    }

    for m in a.macros.iter().filter(|m| !allowed(m.module)) {
        let why = match m.name.as_str() {
            "thread_local" => Some("`thread_local!`".to_string()),
            "lazy_static" => {
                mutable_marker(&m.tokens).map(|x| format!("`lazy_static!` holding `{x}`"))
            }
            _ => None,
        };
        if let Some(why) = why {
            out.push(Violation {
                rule: "global-state",
                severity,
                message: format!("global mutable state: {why} in `{}`", a.name(m.module)),
                notes: vec![Note {
                    loc: m.loc.clone(),
                    message: "declared here".into(),
                }],
            });
        }
    }
    out
}
