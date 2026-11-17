//! `edict.toml` schema, defaults and validation. Pure: parses from a string.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::graph;
use crate::model::Severity;

/// Side-effect paths banned in `pure` layers unless overridden.
pub const DEFAULT_DENY: &[&str] = &[
    "std::fs",
    "std::net",
    "std::env",
    "std::process",
    "std::thread",
    "std::io::stdin",
    "std::io::stdout",
    "std::io::stderr",
    "std::time::Instant",
    "std::time::SystemTime",
    "tokio",
    "async_std",
    "reqwest",
    "hyper",
    "rand",
    "chrono::Utc::now",
    "chrono::Local::now",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Off,
    Warn,
    Error,
}

impl Level {
    pub fn severity(self) -> Option<Severity> {
        match self {
            Level::Off => None,
            Level::Warn => Some(Severity::Warning),
            Level::Error => Some(Severity::Error),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    /// Every `.rs` file is a node.
    File,
    /// Submodules roll up into their top-level module.
    Top,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CyclesCfg {
    pub level: Level,
    pub granularity: Granularity,
    /// Ignore cycles between a module and its direct parent (`mod.rs` uses a
    /// child, the child uses `super::Type`). An adoption knob; off by default.
    pub allow_parent_child: bool,
}

impl Default for CyclesCfg {
    fn default() -> Self {
        Self {
            level: Level::Error,
            granularity: Granularity::File,
            allow_parent_child: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FacadeCfg {
    pub level: Level,
    /// Library crates only: allow `pub mod` below top-level modules, for a
    /// deliberately nested public API (e.g. `mylib::parse::Parser`).
    /// `pub(crate)` and other crate-internal leaks still fail.
    pub allow_public_modules: bool,
}

impl Default for FacadeCfg {
    fn default() -> Self {
        Self {
            level: Level::Error,
            allow_public_modules: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GlobalStateCfg {
    pub level: Level,
    /// Module path prefixes allowed to hold global mutable state.
    pub allow: Vec<String>,
}

impl Default for GlobalStateCfg {
    fn default() -> Self {
        Self {
            level: Level::Error,
            allow: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PurityCfg {
    pub level: Level,
    /// Replaces the default deny list when set.
    pub deny: Option<Vec<String>>,
    /// Added on top of the deny list.
    pub extra_deny: Vec<String>,
}

impl Default for PurityCfg {
    fn default() -> Self {
        Self {
            level: Level::Error,
            deny: None,
            extra_deny: Vec::new(),
        }
    }
}

impl PurityCfg {
    pub fn deny_list(&self) -> Vec<String> {
        let mut list: Vec<String> = match &self.deny {
            Some(d) => d.clone(),
            None => DEFAULT_DENY.iter().map(|s| s.to_string()).collect(),
        };
        list.extend(self.extra_deny.iter().cloned());
        list
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LimitsCfg {
    pub level: Level,
    /// Max distinct in-crate files a file may depend on.
    pub max_fan_out: usize,
    /// Max lines per file.
    pub max_lines: usize,
}

impl Default for LimitsCfg {
    fn default() -> Self {
        Self {
            level: Level::Warn,
            max_fan_out: 10,
            max_lines: 1500,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub name: String,
    /// Top-level module names (children of the crate root) in this layer.
    pub modules: Vec<String>,
    /// Layers this layer may depend on. Transitive.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Ban side-effect paths (see `[purity]`) in this layer.
    #[serde(default)]
    pub pure: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LayersCfg {
    pub level: Level,
    /// Fail if a top-level module is not assigned to any layer.
    pub require_all: bool,
}

impl Default for LayersCfg {
    fn default() -> Self {
        Self {
            level: Level::Error,
            require_all: false,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub cycles: CyclesCfg,
    pub facade: FacadeCfg,
    pub global_state: GlobalStateCfg,
    pub purity: PurityCfg,
    pub limits: LimitsCfg,
    pub layers: LayersCfg,
    #[serde(rename = "layer")]
    pub layer: Vec<Layer>,
}

impl Config {
    pub fn parse(src: &str) -> Result<Config, String> {
        let cfg: Config = toml::from_str(src).map_err(|e| e.to_string())?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), String> {
        let mut index = BTreeMap::new();
        for (i, l) in self.layer.iter().enumerate() {
            if index.insert(l.name.as_str(), i).is_some() {
                return Err(format!("layer `{}` is defined twice", l.name));
            }
        }
        let mut owner: BTreeMap<&str, &str> = BTreeMap::new();
        for l in &self.layer {
            for m in &l.modules {
                if let Some(prev) = owner.insert(m.as_str(), l.name.as_str()) {
                    return Err(format!(
                        "module `{m}` is in both layer `{prev}` and layer `{}`",
                        l.name
                    ));
                }
            }
        }
        let mut adj = vec![BTreeSet::new(); self.layer.len()];
        for (i, l) in self.layer.iter().enumerate() {
            for d in &l.depends_on {
                let j = *index
                    .get(d.as_str())
                    .ok_or_else(|| format!("layer `{}` depends on unknown layer `{d}`", l.name))?;
                adj[i].insert(j);
            }
        }
        if !graph::is_acyclic(self.layer.len(), &adj) {
            return Err("layer `depends_on` declarations form a cycle".to_string());
        }
        Ok(())
    }

    /// Layer index for a top-level module name.
    pub fn layer_of(&self, module: &str) -> Option<usize> {
        self.layer
            .iter()
            .position(|l| l.modules.iter().any(|m| m == module))
    }

    /// For each layer, the set of layers it may depend on (transitive, excluding itself).
    pub fn allowed_layers(&self) -> Vec<BTreeSet<usize>> {
        let n = self.layer.len();
        let direct: Vec<BTreeSet<usize>> = self
            .layer
            .iter()
            .map(|l| {
                l.depends_on
                    .iter()
                    .filter_map(|d| self.layer.iter().position(|x| &x.name == d))
                    .collect()
            })
            .collect();
        (0..n)
            .map(|start| {
                let mut seen = BTreeSet::new();
                let mut stack: Vec<usize> = direct[start].iter().copied().collect();
                while let Some(v) = stack.pop() {
                    if seen.insert(v) {
                        stack.extend(direct[v].iter().copied());
                    }
                }
                seen
            })
            .collect()
    }
}

pub const TEMPLATE: &str = r#"# edict.toml — architecture laws for this crate.
# Every rule: level = "error" | "warn" | "off"

[cycles]
level = "error"
# "file": no cycles between any two .rs files. "top": only between top-level modules.
granularity = "file"
# Adoption knob: ignore cycles between a module and its direct parent
# (mod.rs calls a child, the child uses `super::Type`). Fix them instead by
# moving shared types out of mod.rs into a child, e.g. types.rs.
allow_parent_child = false

[facade]
# Every module inside a top-level module stays private: `mod tax;`, never
# `pub(crate) mod tax;`. Expose items with `pub use tax::rate;` at the root.
# Code outside may only use the root's exports: crate::billing::Invoice is
# fine, crate::billing::tax::rate is not.
level = "error"
# Library crates only: allow nested `pub mod` as deliberate public API.
allow_public_modules = false

[global_state]
# No `static mut`, no statics holding Mutex/RwLock/RefCell/Cell/atomics, no thread_local!.
level = "error"
allow = []  # e.g. ["infra::runtime"]

[purity]
# Applies to layers marked `pure = true`. Default deny list covers fs, net, env,
# process, threads, clocks, randomness, tokio, reqwest.
level = "error"
extra_deny = []

[limits]
level = "warn"
max_fan_out = 10
max_lines = 1500

[layers]
level = "error"
require_all = false

# Declare layers bottom-up. `depends_on` is transitive. The crate root
# (lib.rs / main.rs) is the composition root and is exempt.
#
# [[layer]]
# name = "domain"
# modules = ["domain"]
# pure = true
#
# [[layer]]
# name = "app"
# modules = ["app"]
# depends_on = ["domain"]
#
# [[layer]]
# name = "infra"
# modules = ["db", "http"]
# depends_on = ["app"]
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses() {
        Config::parse(TEMPLATE).unwrap();
    }

    #[test]
    fn rejects_layer_cycle() {
        let src = r#"
            [[layer]]
            name = "a"
            modules = ["a"]
            depends_on = ["b"]
            [[layer]]
            name = "b"
            modules = ["b"]
            depends_on = ["a"]
        "#;
        assert!(Config::parse(src).unwrap_err().contains("cycle"));
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(Config::parse("[cycles]\nlevle = \"warn\"").is_err());
    }

    #[test]
    fn transitive_layers() {
        let src = r#"
            [[layer]]
            name = "domain"
            modules = ["domain"]
            [[layer]]
            name = "app"
            modules = ["app"]
            depends_on = ["domain"]
            [[layer]]
            name = "api"
            modules = ["api"]
            depends_on = ["app"]
        "#;
        let cfg = Config::parse(src).unwrap();
        assert_eq!(cfg.allowed_layers()[2], [0, 1].into());
    }
}
