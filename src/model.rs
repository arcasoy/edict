//! Core data types shared by every stage of the analysis.
//!
//! This module is pure: no I/O, no global state, no dependencies on other
//! modules in this crate.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::Serialize;

/// Index into [`Analysis::modules`].
pub type ModId = usize;

/// The crate root module (`lib.rs` / `main.rs`) is always id 0.
pub const ROOT: ModId = 0;

#[derive(Debug, Clone)]
pub struct Module {
    /// Path segments below `crate`, e.g. `["billing", "tax"]`. Empty for the root.
    pub segments: Vec<String>,
    /// File the module's items live in (relative to the crate directory).
    pub file: PathBuf,
    pub parent: Option<ModId>,
    pub children: BTreeMap<String, ModId>,
    /// `mod x { ... }` declared inline rather than in its own file.
    pub inline: bool,
    /// Line count of the file (0 for inline modules).
    pub lines: usize,
}

impl Module {
    pub fn display(&self) -> String {
        if self.segments.is_empty() {
            "crate".to_string()
        } else {
            self.segments.join("::")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Loc {
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    /// A private `use` statement.
    Use,
    /// A `pub use` / `pub(crate) use` re-export.
    PubUse,
    /// A path written inline in code, e.g. `crate::billing::total()`.
    Path,
}

/// A path reference before it is resolved to a module.
#[derive(Debug, Clone)]
pub struct RawRef {
    pub from: ModId,
    /// Absolute path. In-crate paths start with `"crate"`; external paths
    /// start with the external crate name (e.g. `["std", "fs", "read"]`).
    pub path: Vec<String>,
    pub kind: RefKind,
    pub loc: Loc,
    /// A guess produced by a glob import (`use crate::foo::*`): keep it only
    /// if `foo` really defines or re-exports the name.
    pub candidate: bool,
}

/// A `pub use` re-export, used to follow paths to the module that actually
/// defines an item.
#[derive(Debug, Clone)]
pub struct Reexport {
    /// Exported name, or `None` for a glob (`pub use foo::*`).
    pub name: Option<String>,
    /// Absolute target path (item path for named, module path for glob).
    pub target: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Exports {
    pub defined: BTreeSet<String>,
    pub reexports: Vec<Reexport>,
}

#[derive(Debug, Clone)]
pub struct StaticItem {
    pub module: ModId,
    pub name: String,
    pub is_mut: bool,
    /// Type rendered as tokens, e.g. `Mutex < Vec < u8 > >`.
    pub ty: String,
    pub loc: Loc,
}

#[derive(Debug, Clone)]
pub struct MacroUse {
    pub module: ModId,
    /// Last segment of the macro path, e.g. `thread_local`.
    pub name: String,
    pub tokens: String,
    pub loc: Loc,
}

/// A resolved in-crate dependency.
#[derive(Debug, Clone)]
pub struct Edge {
    pub from: ModId,
    /// Module that defines the referenced item (re-exports followed).
    pub to: ModId,
    /// Module named in the path as written (re-exports not followed).
    pub written_to: ModId,
    pub kind: RefKind,
    pub path: String,
    pub loc: Loc,
    pub source: Source,
}

/// Which engine found an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Fast mode: paths as written in the source.
    Syntax,
    /// Full mode: rust-analyzer's SCIP index (macros expanded, types inferred).
    Semantic,
}

/// A reference to something outside the crate (std, tokio, ...).
#[derive(Debug, Clone)]
pub struct ExternRef {
    pub from: ModId,
    pub path: String,
    pub loc: Loc,
}

#[derive(Debug, Clone)]
pub struct Analysis {
    pub crate_name: String,
    pub modules: Vec<Module>,
    pub edges: Vec<Edge>,
    pub externs: Vec<ExternRef>,
    pub statics: Vec<StaticItem>,
    pub macros: Vec<MacroUse>,
}

impl Analysis {
    /// Top-level module containing `id` (a direct child of the root), or
    /// `None` for the root itself.
    pub fn top_of(&self, id: ModId) -> Option<ModId> {
        let mut cur = id;
        loop {
            match self.modules[cur].parent {
                None => return None,
                Some(ROOT) => return Some(cur),
                Some(p) => cur = p,
            }
        }
    }

    /// Nearest enclosing module that has its own file (inline modules roll up).
    pub fn file_module_of(&self, id: ModId) -> ModId {
        let mut cur = id;
        while self.modules[cur].inline {
            match self.modules[cur].parent {
                Some(p) => cur = p,
                None => break,
            }
        }
        cur
    }

    /// True if `id` is a strict descendant of `ancestor`.
    pub fn is_descendant(&self, id: ModId, ancestor: ModId) -> bool {
        let mut cur = self.modules[id].parent;
        while let Some(p) = cur {
            if p == ancestor {
                return true;
            }
            cur = self.modules[p].parent;
        }
        false
    }

    /// Edges that represent real dependencies for graph rules. A parent's
    /// `pub use child::Thing` is a barrel re-export, not a dependency: callers
    /// are already resolved through it to `child`.
    pub fn is_barrel(&self, e: &Edge) -> bool {
        e.kind == RefKind::PubUse && self.is_descendant(e.to, e.from)
    }

    pub fn name(&self, id: ModId) -> String {
        self.modules[id].display()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct Note {
    pub loc: Loc,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Violation {
    pub rule: &'static str,
    pub severity: Severity,
    pub message: String,
    pub notes: Vec<Note>,
}
