//! Per-language tables (`LangSpec`) that drive the generic lowering, scoping and
//! extraction passes, plus kind-id indexed lookup tables built once per language.

use std::sync::OnceLock;

use sillage_core::Lang;
use tree_sitter::{Language, Node};

pub mod go;
pub mod java;
pub mod javascript;
pub mod python;
pub mod rust;
pub mod typescript;

/// Where a binding introduced by a [`DeclRule`] lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hoist {
    /// The innermost scope enclosing the declaration.
    Current,
    /// The innermost function scope (JS `var`).
    Function,
    /// `Function` when the declaring node's parent has this kind, else `Current`.
    FunctionIfParent(&'static str),
    /// Name of a nested scope node (function/class name): binds in the scope that contains it.
    Enclosing,
    /// Python `global`: the name refers to a global in this function scope.
    Global,
    /// Python `nonlocal`: the name is not local to this function scope.
    Nonlocal,
}

/// Extra condition for a [`DeclRule`] to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Require {
    None,
    /// The node has an anonymous child token with this text (Go `:=`).
    ChildToken(&'static str),
    /// The node has a child under this field (JS `for (let x of ..)`).
    Field(&'static str),
}

/// `kind` declares the binding identifiers found in the pattern(s) under `field`
/// (or under each named child when `field == None`).
#[derive(Debug, Clone, Copy)]
pub struct DeclRule {
    pub kind: &'static str,
    pub field: Option<&'static str>,
    pub hoist: Hoist,
    pub require: Require,
}

pub const fn decl(kind: &'static str, field: &'static str) -> DeclRule {
    DeclRule {
        kind,
        field: Some(field),
        hoist: Hoist::Current,
        require: Require::None,
    }
}

pub const fn decl_children(kind: &'static str) -> DeclRule {
    DeclRule {
        kind,
        field: None,
        hoist: Hoist::Current,
        require: Require::None,
    }
}

impl DeclRule {
    pub const fn hoist(mut self, hoist: Hoist) -> Self {
        self.hoist = hoist;
        self
    }

    pub const fn require(mut self, require: Require) -> Self {
        self.require = require;
        self
    }
}

/// Scope-introducing node kind.
#[derive(Debug, Clone, Copy)]
pub struct ScopeRule {
    pub kind: &'static str,
    pub function: bool,
    pub class: bool,
}

pub const fn block(kind: &'static str) -> ScopeRule {
    ScopeRule {
        kind,
        function: false,
        class: false,
    }
}

pub const fn func(kind: &'static str) -> ScopeRule {
    ScopeRule {
        kind,
        function: true,
        class: false,
    }
}

pub const fn class(kind: &'static str) -> ScopeRule {
    ScopeRule {
        kind,
        function: true,
        class: true,
    }
}

/// Call node whose argument list is spliced into the call (`#(#)`), optionally
/// regrouping `name(args)` under a synthetic node when a receiver precedes it
/// (Java ANTLR shape `#.#` over `#(#)`, paper Fig. 4).
#[derive(Debug, Clone, Copy)]
pub struct CallRule {
    pub kind: &'static str,
    pub args_field: &'static str,
    pub regroup_from: Option<&'static str>,
}

/// A method-like node to extract.
#[derive(Debug, Clone, Copy)]
pub struct MethodRule {
    pub kind: &'static str,
    pub name_field: &'static str,
    pub body_field: &'static str,
}

pub type NodeHook = fn(node: Node<'_>, parent: Node<'_>, field: Option<&str>) -> bool;

pub fn no_hook(_: Node<'_>, _: Node<'_>, _: Option<&str>) -> bool {
    false
}

pub struct LangSpec {
    /// Named kinds lowered as one atomic `Token` (literals).
    pub atomic: &'static [&'static str],
    /// Named leaf kinds that are lexical keywords (`this`, `void`, `mut`).
    pub keyword_leaves: &'static [&'static str],
    /// Kinds dropped entirely (comments).
    pub drop: &'static [&'static str],
    pub calls: &'static [CallRule],
    /// Argument-list kinds that `calls` may splice.
    pub args_kinds: &'static [&'static str],
    /// Identifier kinds that may reference a variable.
    pub ref_idents: &'static [&'static str],
    /// Identifier kinds that may introduce a binding inside a pattern.
    pub bind_idents: &'static [&'static str],
    pub scopes: &'static [ScopeRule],
    pub decls: &'static [DeclRule],
    /// Pattern container kinds descended into when collecting bindings.
    pub containers: &'static [&'static str],
    /// `(kind, field)` pairs skipped while collecting bindings (`"*"` = any kind).
    pub pattern_skip: &'static [(&'static str, &'static str)],
    /// `(parent kind, field)` positions whose identifiers are never variables (`"*"` = any field).
    pub non_var: &'static [(&'static str, &'static str)],
    /// `(parent kind, field)` callee positions: variables only when resolved to a local binding.
    pub callee: &'static [(&'static str, &'static str)],
    /// Free names that are never treated as snippet locals.
    pub builtins: &'static [&'static str],
    /// Capitalised identifiers in patterns are constructors/constants, not bindings (Rust).
    pub upper_pattern_not_binding: bool,
    /// Language-specific never-a-variable positions.
    pub non_var_hook: NodeHook,
    /// Language-specific positions where an *unresolved* name is a global (Go package operands).
    pub free_global_hook: NodeHook,
    pub methods: &'static [MethodRule],
    /// Anonymous function kinds extracted when bound to a name by `named_parents`.
    pub anon_fns: &'static [&'static str],
    /// `(parent kind, name field)` naming an anonymous function child.
    pub named_parents: &'static [(&'static str, &'static str)],
}

pub fn spec(lang: Lang) -> &'static LangSpec {
    match lang {
        Lang::Java => &java::SPEC,
        Lang::Python => &python::SPEC,
        Lang::JavaScript => &javascript::SPEC,
        Lang::TypeScript | Lang::Tsx => &typescript::SPEC,
        Lang::Go => &go::SPEC,
        Lang::Rust => &rust::SPEC,
    }
}

pub fn ts_language(lang: Lang) -> Language {
    match lang {
        Lang::Java => tree_sitter_java::LANGUAGE.into(),
        Lang::Python => tree_sitter_python::LANGUAGE.into(),
        Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Lang::Go => tree_sitter_go::LANGUAGE.into(),
        Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
    }
}

pub fn lang_index(lang: Lang) -> usize {
    match lang {
        Lang::Java => 0,
        Lang::Python => 1,
        Lang::JavaScript => 2,
        Lang::TypeScript => 3,
        Lang::Tsx => 4,
        Lang::Go => 5,
        Lang::Rust => 6,
    }
}

pub const F_ATOMIC: u16 = 1 << 0;
pub const F_KW_LEAF: u16 = 1 << 1;
pub const F_DROP: u16 = 1 << 2;
pub const F_SCOPE: u16 = 1 << 3;
pub const F_SCOPE_FN: u16 = 1 << 4;
pub const F_SCOPE_CLASS: u16 = 1 << 5;
pub const F_REF_IDENT: u16 = 1 << 6;
pub const F_BIND_IDENT: u16 = 1 << 7;
pub const F_CONTAINER: u16 = 1 << 8;
pub const F_ARGS: u16 = 1 << 9;

#[derive(Default)]
pub struct KindInfo {
    pub flags: u16,
    pub decls: Vec<&'static DeclRule>,
    pub non_var: Vec<&'static str>,
    pub skip: Vec<&'static str>,
    pub callee: Vec<&'static str>,
    pub call: Option<&'static CallRule>,
}

/// Kind-id indexed view of a [`LangSpec`].
pub struct Tables {
    pub spec: &'static LangSpec,
    kinds: Vec<KindInfo>,
    skip_any: Vec<&'static str>,
    empty: KindInfo,
}

impl Tables {
    fn build(lang: Lang) -> Tables {
        let spec = spec(lang);
        let language = ts_language(lang);
        let count = language.node_kind_count();
        let mut kinds: Vec<KindInfo> = (0..count).map(|_| KindInfo::default()).collect();
        for (id, info) in kinds.iter_mut().enumerate() {
            let id = id as u16;
            let Some(name) = language.node_kind_for_id(id) else {
                continue;
            };
            let has = |list: &[&str]| list.contains(&name);
            if !language.node_kind_is_named(id) {
                // Hidden aliases such as Python's `as_pattern_target` can be anonymous.
                if has(spec.containers) {
                    info.flags = F_CONTAINER;
                }
                continue;
            }
            let mut f = 0;
            if has(spec.atomic) {
                f |= F_ATOMIC;
            }
            if has(spec.keyword_leaves) {
                f |= F_KW_LEAF;
            }
            if has(spec.drop) {
                f |= F_DROP;
            }
            if has(spec.ref_idents) {
                f |= F_REF_IDENT;
            }
            if has(spec.bind_idents) {
                f |= F_BIND_IDENT;
            }
            if has(spec.containers) {
                f |= F_CONTAINER;
            }
            if has(spec.args_kinds) {
                f |= F_ARGS;
            }
            if let Some(s) = spec.scopes.iter().find(|s| s.kind == name) {
                f |= F_SCOPE;
                if s.function {
                    f |= F_SCOPE_FN;
                }
                if s.class {
                    f |= F_SCOPE_CLASS;
                }
            }
            info.flags = f;
            info.decls = spec.decls.iter().filter(|d| d.kind == name).collect();
            info.non_var = pairs_for(spec.non_var, name);
            info.skip = pairs_for(spec.pattern_skip, name);
            info.callee = pairs_for(spec.callee, name);
            info.call = spec.calls.iter().find(|c| c.kind == name);
        }
        let skip_any = pairs_for(spec.pattern_skip, "*");
        Tables {
            spec,
            kinds,
            skip_any,
            empty: KindInfo::default(),
        }
    }

    pub fn info(&self, node: Node<'_>) -> &KindInfo {
        self.kinds
            .get(node.kind_id() as usize)
            .unwrap_or(&self.empty)
    }

    pub fn flags(&self, node: Node<'_>) -> u16 {
        self.info(node).flags
    }

    /// Is `field` of a `parent` pattern container skipped while collecting bindings?
    pub fn skipped(&self, parent: Node<'_>, field: Option<&str>) -> bool {
        let Some(field) = field else { return false };
        self.skip_any.contains(&field) || self.info(parent).skip.contains(&field)
    }
}

fn pairs_for(pairs: &[(&'static str, &'static str)], kind: &str) -> Vec<&'static str> {
    pairs
        .iter()
        .filter(|(k, _)| *k == kind)
        .map(|(_, f)| *f)
        .collect()
}

/// Matches a `(kind, field)` list entry against a concrete field (`"*"` matches all).
pub fn field_matches(fields: &[&str], field: Option<&str>) -> bool {
    fields
        .iter()
        .any(|f| *f == "*" || Some(*f) == field)
}

static TABLES: [OnceLock<Tables>; 7] = [const { OnceLock::new() }; 7];

pub fn tables(lang: Lang) -> &'static Tables {
    TABLES[lang_index(lang)].get_or_init(|| Tables::build(lang))
}
