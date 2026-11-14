//! Walks each module's syntax tree and extracts what the rules need:
//! defined names, re-exports, path references, statics and macro calls.
//! Pure: operates on already-parsed `syn` items.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::{Spacing, Span, TokenTree};
use quote::ToTokens;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};

use crate::attrs::{is_test_fn, is_test_only};
use crate::model::{Exports, Loc, MacroUse, ModId, Module, RawRef, Reexport, RefKind, StaticItem};

pub struct Extracted {
    pub exports: Vec<Exports>,
    pub raw: Vec<RawRef>,
    pub statics: Vec<StaticItem>,
    pub macros: Vec<MacroUse>,
}

pub fn extract(modules: &[Module], items: &[Vec<syn::Item>]) -> Extracted {
    let mut out = Extracted {
        exports: vec![Exports::default(); modules.len()],
        raw: Vec::new(),
        statics: Vec::new(),
        macros: Vec::new(),
    };

    for (id, module_items) in items.iter().enumerate() {
        let defined: BTreeSet<String> = module_items
            .iter()
            .filter(|i| !is_test_only(item_attrs(i)))
            .filter_map(item_name)
            .collect();
        let scope = Scope {
            modules,
            id,
            defined: &defined,
        };

        // Pass 1: `use` statements (including ones nested in function bodies).
        let mut uses = UseCollector {
            scope: &scope,
            imports: BTreeMap::new(),
            globs: Vec::new(),
            raw: Vec::new(),
            reexports: Vec::new(),
        };
        for item in module_items {
            uses.visit_item(item);
        }

        // Pass 2: every other path, resolved against the imports from pass 1.
        let mut paths = PathCollector {
            scope: &scope,
            imports: &uses.imports,
            globs: &uses.globs,
            in_call: false,
            raw: Vec::new(),
            statics: Vec::new(),
            macros: Vec::new(),
        };
        for item in module_items {
            paths.visit_item(item);
        }

        out.exports[id] = Exports {
            defined: defined.clone(),
            reexports: uses.reexports,
        };
        out.raw.extend(uses.raw);
        out.raw.extend(paths.raw);
        out.statics.extend(paths.statics);
        out.macros.extend(paths.macros);
    }
    // The same path can be seen twice on one line (e.g. via a macro's
    // expression parse and its token scan).
    let mut seen = BTreeSet::new();
    out.raw
        .retain(|r| seen.insert((r.from, r.path.clone(), r.loc.line, r.candidate)));
    out
}

struct Scope<'a> {
    modules: &'a [Module],
    id: ModId,
    defined: &'a BTreeSet<String>,
}

impl Scope<'_> {
    fn loc(&self, span: Span) -> Loc {
        Loc {
            file: self.modules[self.id].file.clone(),
            line: span.start().line,
        }
    }

    fn module_path(&self, id: ModId) -> Vec<String> {
        let mut p = vec!["crate".to_string()];
        p.extend(self.modules[id].segments.iter().cloned());
        p
    }

    /// Absolute form of a path written in this module, or `None` if it can't
    /// be resolved (e.g. `super` past the root).
    fn absolutize(&self, segs: &[String]) -> Option<Vec<String>> {
        let first = segs.first()?;
        if first == "crate" {
            return Some(segs.to_vec());
        }
        if first == "self" || first == "super" {
            let mut base = self.id;
            let mut i = usize::from(first == "self");
            while segs.get(i).is_some_and(|s| s == "super") {
                base = self.modules[base].parent?;
                i += 1;
            }
            let mut p = self.module_path(base);
            p.extend(segs[i..].iter().cloned());
            return Some(p);
        }
        if self.modules[self.id].children.contains_key(first) || self.defined.contains(first) {
            let mut p = self.module_path(self.id);
            p.extend(segs.iter().cloned());
            return Some(p);
        }
        Some(segs.to_vec()) // external crate
    }
}

enum UseLeaf {
    Named { path: Vec<String>, name: String },
    Glob { path: Vec<String> },
}

fn flatten_use(tree: &syn::UseTree, prefix: &mut Vec<String>, out: &mut Vec<UseLeaf>) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            flatten_use(&p.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            if n.ident == "self" {
                if let Some(last) = prefix.last() {
                    out.push(UseLeaf::Named {
                        path: prefix.clone(),
                        name: last.clone(),
                    });
                }
            } else {
                let mut path = prefix.clone();
                path.push(n.ident.to_string());
                out.push(UseLeaf::Named {
                    path,
                    name: n.ident.to_string(),
                });
            }
        }
        syn::UseTree::Rename(r) => {
            let mut path = prefix.clone();
            if r.ident != "self" {
                path.push(r.ident.to_string());
            }
            out.push(UseLeaf::Named {
                path,
                name: r.rename.to_string(),
            });
        }
        syn::UseTree::Glob(_) => out.push(UseLeaf::Glob {
            path: prefix.clone(),
        }),
        syn::UseTree::Group(g) => {
            for t in &g.items {
                flatten_use(t, prefix, out);
            }
        }
    }
}

struct UseCollector<'a> {
    scope: &'a Scope<'a>,
    imports: BTreeMap<String, Vec<String>>,
    /// In-crate glob imports (`use crate::foo::*`), as absolute module paths.
    globs: Vec<Vec<String>>,
    raw: Vec<RawRef>,
    reexports: Vec<Reexport>,
}

impl<'ast> Visit<'ast> for UseCollector<'_> {
    fn visit_item(&mut self, i: &'ast syn::Item) {
        if !is_test_only(item_attrs(i)) {
            visit::visit_item(self, i);
        }
    }

    fn visit_item_mod(&mut self, _: &'ast syn::ItemMod) {}

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        if !is_test_fn(&f.attrs) {
            visit::visit_impl_item_fn(self, f);
        }
    }

    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        let is_pub = !matches!(u.vis, syn::Visibility::Inherited);
        let kind = if is_pub {
            RefKind::PubUse
        } else {
            RefKind::Use
        };
        let loc = self.scope.loc(u.use_token.span);
        let mut leaves = Vec::new();
        flatten_use(&u.tree, &mut Vec::new(), &mut leaves);

        for leaf in leaves {
            let (written, name) = match &leaf {
                UseLeaf::Named { path, name } => (path, Some(name.clone())),
                UseLeaf::Glob { path } => (path, None),
            };
            let abs = if u.leading_colon.is_some() {
                Some(written.clone())
            } else {
                self.scope.absolutize(written)
            };
            let Some(abs) = abs else { continue };
            if abs.len() <= 1 {
                continue;
            }
            if let Some(n) = &name {
                if n != "_" {
                    self.imports.insert(n.clone(), abs.clone());
                }
            }
            if name.is_none() && abs[0] == "crate" {
                self.globs.push(abs.clone());
            }
            if is_pub {
                self.reexports.push(Reexport {
                    name: name.filter(|n| n != "_"),
                    target: abs.clone(),
                });
            }
            self.raw.push(RawRef {
                from: self.scope.id,
                path: abs,
                kind,
                loc: loc.clone(),
                candidate: false,
            });
        }
    }
}

struct PathCollector<'a> {
    scope: &'a Scope<'a>,
    imports: &'a BTreeMap<String, Vec<String>>,
    globs: &'a [Vec<String>],
    /// The next path visited is the callee of a call expression.
    in_call: bool,
    raw: Vec<RawRef>,
    statics: Vec<StaticItem>,
    macros: Vec<MacroUse>,
}

impl PathCollector<'_> {
    /// Record a path written in this module.
    ///
    /// `glob_single`: a single-segment name may be resolved through glob
    /// imports. Only allowed where it is almost certainly an item, not a local
    /// variable: callees and capitalized names (types, consts, variants).
    fn record(&mut self, segs: Vec<String>, span: Span, leading_colon: bool, glob_single: bool) {
        let Some(first) = segs.first() else { return };
        let push = |raw: &mut Vec<RawRef>, path: Vec<String>, candidate: bool| {
            raw.push(RawRef {
                from: self.scope.id,
                path,
                kind: RefKind::Path,
                loc: self.scope.loc(span),
                candidate,
            });
        };

        if leading_colon {
            if segs.len() > 1 {
                push(&mut self.raw, segs, false);
            }
            return;
        }
        if first == "Self" {
            return;
        }
        if let Some(imported) = self.imports.get(first) {
            let mut p = imported.clone();
            p.extend(segs[1..].iter().cloned());
            if p.len() > 1 {
                push(&mut self.raw, p, false);
            }
            return;
        }
        let module_relative = first == "crate"
            || first == "self"
            || first == "super"
            || self.scope.modules[self.scope.id]
                .children
                .contains_key(first);
        if module_relative {
            if let Some(abs) = self.scope.absolutize(&segs).filter(|p| p.len() > 1) {
                push(&mut self.raw, abs, false);
            }
            return;
        }
        if self.scope.defined.contains(first) {
            return; // local item, e.g. `Foo::new`
        }

        // Not imported by name, not local: maybe it came in through a glob.
        if segs.len() > 1 || glob_single {
            for glob in self.globs {
                let mut p = glob.clone();
                p.extend(segs.iter().cloned());
                push(&mut self.raw, p, true);
            }
        }
        if segs.len() > 1 {
            push(&mut self.raw, segs, false); // external path, e.g. `std::fs::read`
        }
    }

    /// Find `a::b::c` sequences in raw macro tokens, at any nesting depth.
    fn scan_tokens(&mut self, tokens: proc_macro2::TokenStream) {
        let toks: Vec<TokenTree> = tokens.into_iter().collect();
        let mut i = 0;
        while i < toks.len() {
            match &toks[i] {
                TokenTree::Group(g) => {
                    self.scan_tokens(g.stream());
                    i += 1;
                }
                TokenTree::Ident(id) => {
                    let after_colons = i >= 2 && is_path_sep(&toks, i - 2);
                    let leading_colon =
                        after_colons && (i < 3 || !matches!(toks[i - 3], TokenTree::Ident(_)));
                    let mut segs = vec![id.to_string()];
                    let mut j = i + 1;
                    while is_path_sep(&toks, j) {
                        let Some(TokenTree::Ident(next)) = toks.get(j + 2) else {
                            break;
                        };
                        segs.push(next.to_string());
                        j += 3;
                    }
                    if segs.len() > 1 {
                        self.record(segs, id.span(), leading_colon, false);
                    }
                    i = j;
                }
                _ => i += 1,
            }
        }
    }
}

/// `::` starting at `toks[i]`.
fn is_path_sep(toks: &[TokenTree], i: usize) -> bool {
    matches!(
        (toks.get(i), toks.get(i + 1)),
        (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b)))
            if a.as_char() == ':' && a.spacing() == Spacing::Joint && b.as_char() == ':'
    )
}

impl<'ast> Visit<'ast> for PathCollector<'_> {
    fn visit_item(&mut self, i: &'ast syn::Item) {
        if !is_test_only(item_attrs(i)) {
            visit::visit_item(self, i);
        }
    }

    fn visit_item_mod(&mut self, _: &'ast syn::ItemMod) {}

    fn visit_item_use(&mut self, _: &'ast syn::ItemUse) {}

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        if !is_test_fn(&f.attrs) {
            visit::visit_impl_item_fn(self, f);
        }
    }

    fn visit_path(&mut self, p: &'ast syn::Path) {
        let segs: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
        let span = p
            .segments
            .first()
            .map(|s| s.ident.span())
            .unwrap_or_else(Span::call_site);
        let capitalized = segs
            .first()
            .and_then(|f| f.chars().next())
            .is_some_and(char::is_uppercase);
        let glob_single = std::mem::take(&mut self.in_call) || capitalized;
        self.record(segs, span, p.leading_colon.is_some(), glob_single);
        visit::visit_path(self, p);
    }

    fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
        if let syn::Expr::Path(_) = &*c.func {
            self.in_call = true;
        }
        self.visit_expr(&c.func);
        self.in_call = false;
        for arg in &c.args {
            self.visit_expr(arg);
        }
    }

    fn visit_item_static(&mut self, s: &'ast syn::ItemStatic) {
        self.statics.push(StaticItem {
            module: self.scope.id,
            name: s.ident.to_string(),
            is_mut: matches!(s.mutability, syn::StaticMutability::Mut(_)),
            ty: s.ty.to_token_stream().to_string(),
            loc: self.scope.loc(s.ident.span()),
        });
        visit::visit_item_static(self, s);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let name = mac
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        let span = mac
            .path
            .segments
            .first()
            .map(|s| s.ident.span())
            .unwrap_or_else(Span::call_site);
        self.macros.push(MacroUse {
            module: self.scope.id,
            name,
            tokens: mac.tokens.to_string(),
            loc: self.scope.loc(span),
        });
        self.visit_path(&mac.path);
        // Best effort: most macros (format!, assert!, vec!) take comma-separated
        // expressions. Parse them so paths inside macro calls are not missed.
        if let Ok(exprs) =
            Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated.parse2(mac.tokens.clone())
        {
            for e in &exprs {
                <Self as Visit<'_>>::visit_expr(self, e);
            }
        }
        // Catch `a::b::c` in macros whose arguments are not plain expressions
        // (`matches!`, `vec![x; n]`, DSLs, `$crate::` in macro_rules bodies).
        self.scan_tokens(mac.tokens.clone());
    }
}

fn item_name(item: &syn::Item) -> Option<String> {
    use syn::Item::*;
    let ident = match item {
        Const(i) => &i.ident,
        Enum(i) => &i.ident,
        Fn(i) => &i.sig.ident,
        Macro(i) => i.ident.as_ref()?,
        Mod(i) => &i.ident,
        Static(i) => &i.ident,
        Struct(i) => &i.ident,
        Trait(i) => &i.ident,
        TraitAlias(i) => &i.ident,
        Type(i) => &i.ident,
        Union(i) => &i.ident,
        ExternCrate(i) => i.rename.as_ref().map(|(_, r)| r).unwrap_or(&i.ident),
        _ => return None,
    };
    Some(ident.to_string())
}

fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    use syn::Item::*;
    match item {
        Const(i) => &i.attrs,
        Enum(i) => &i.attrs,
        ExternCrate(i) => &i.attrs,
        Fn(i) => &i.attrs,
        ForeignMod(i) => &i.attrs,
        Impl(i) => &i.attrs,
        Macro(i) => &i.attrs,
        Mod(i) => &i.attrs,
        Static(i) => &i.attrs,
        Struct(i) => &i.attrs,
        Trait(i) => &i.attrs,
        TraitAlias(i) => &i.attrs,
        Type(i) => &i.attrs,
        Union(i) => &i.attrs,
        Use(i) => &i.attrs,
        _ => &[],
    }
}
