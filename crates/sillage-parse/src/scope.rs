//! Lexical scoping: assigns a [`VarId`] to every identifier that denotes a local
//! variable, keyed by the identifier's start byte in the parsed text.
//!
//! The resolver is generic and driven by the [`LangSpec`](crate::lang::LangSpec)
//! tables. Each scope node gets its own table, filled by a pre-pass over the
//! declarations it owns (not descending into nested scopes, except to hoist JS
//! `var`s into function scopes). This gives Python its function-level scoping with
//! `global`/`nonlocal`, and block scoping for the other languages (a name declared
//! anywhere in a block is visible throughout that block, which only differs from
//! the exact rules in code that would not compile).
//!
//! In snippet mode (Decision 8) free identifiers in value position become locals,
//! one `VarId` per name, while callees, members, types, builtins, capitalised
//! names and language-specific global positions stay `None`.

use rustc_hash::{FxHashMap, FxHashSet};
use sillage_core::VarId;
use tree_sitter::Node;

use crate::lang::{
    F_BIND_IDENT, F_CONTAINER, F_REF_IDENT, F_SCOPE, F_SCOPE_CLASS, F_SCOPE_FN, Hoist, Require,
    Tables, field_matches,
};

/// Recursion guard for pathological nesting.
pub(crate) const MAX_DEPTH: usize = 1200;

#[derive(Clone, Copy)]
enum Binding {
    Var(VarId),
    Global,
}

#[derive(Default)]
struct Scope<'a> {
    names: FxHashMap<&'a str, Binding>,
}

#[derive(Clone, Copy)]
struct ScopeKind {
    function: bool,
    class: bool,
    root: bool,
}

struct Resolver<'a> {
    src: &'a str,
    t: &'static Tables,
    snippet: bool,
    next: VarId,
    out: FxHashMap<usize, VarId>,
    decl_sites: FxHashSet<usize>,
    free: FxHashMap<&'a str, VarId>,
    stack: Vec<Scope<'a>>,
}

/// Resolves local variables in the tree rooted at `root` (parsed from `src`).
pub(crate) fn resolve(
    root: Node<'_>,
    src: &str,
    t: &'static Tables,
    snippet: bool,
) -> FxHashMap<usize, VarId> {
    let mut r = Resolver {
        src,
        t,
        snippet,
        next: 0,
        out: FxHashMap::default(),
        decl_sites: FxHashSet::default(),
        free: FxHashMap::default(),
        stack: Vec::new(),
    };
    let root_kind = ScopeKind {
        function: true,
        class: false,
        root: true,
    };
    let scope = r.open_scope(root, root_kind, false);
    r.stack.push(scope);
    r.walk_children(root, 0);
    r.out
}

fn uppercase_initial(name: &str) -> bool {
    name.chars()
        .find(|c| *c != '_' && *c != '$')
        .is_some_and(|c| c.is_uppercase())
}

impl<'a> Resolver<'a> {
    fn text(&self, n: Node<'_>) -> &'a str {
        self.src.get(n.start_byte()..n.end_byte()).unwrap_or("")
    }

    fn fresh(&mut self) -> VarId {
        let id = self.next;
        self.next += 1;
        id
    }

    /// Builds the scope table for scope node `node` (or the root).
    fn open_scope(&mut self, node: Node<'a>, kind: ScopeKind, own_rules: bool) -> Scope<'a> {
        let mut decls: Vec<(Node<'a>, Hoist)> = Vec::new();
        if own_rules {
            self.apply_rules(node, false, kind.function, true, &mut decls);
        }
        let mut cur = node.walk();
        let children: Vec<Node<'a>> = node.children(&mut cur).collect();
        for child in children {
            self.collect(child, false, kind.function, &mut decls, 0);
        }

        let mut scope = Scope::default();
        let mut nonlocal: FxHashSet<&'a str> = FxHashSet::default();
        for (ident, hoist) in &decls {
            let name = self.text(*ident);
            match hoist {
                Hoist::Global => {
                    scope.names.insert(name, Binding::Global);
                    self.decl_sites.insert(ident.start_byte());
                }
                Hoist::Nonlocal => {
                    nonlocal.insert(name);
                }
                _ => {}
            }
        }
        for (ident, hoist) in decls {
            if matches!(hoist, Hoist::Global | Hoist::Nonlocal) {
                continue;
            }
            let name = self.text(ident);
            if name.is_empty() || nonlocal.contains(name) {
                continue;
            }
            let start = ident.start_byte();
            self.decl_sites.insert(start);
            let global = kind.class
                || (kind.root && (!self.snippet || matches!(hoist, Hoist::Enclosing)));
            if global {
                scope.names.entry(name).or_insert(Binding::Global);
                continue;
            }
            let binding = match scope.names.get(name) {
                Some(b) => *b,
                None => {
                    let b = Binding::Var(self.fresh());
                    scope.names.insert(name, b);
                    b
                }
            };
            if let Binding::Var(id) = binding {
                self.out.insert(start, id);
            }
        }
        scope
    }

    /// Collects declarations owned by the scope being opened.
    fn collect(
        &mut self,
        node: Node<'a>,
        through_block: bool,
        collecting_fn: bool,
        out: &mut Vec<(Node<'a>, Hoist)>,
        depth: usize,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        let flags = self.t.flags(node);
        if flags & F_SCOPE != 0 {
            if !through_block {
                // The nested scope's own name binds here.
                let t: &'static Tables = self.t;
                for &rule in &t.info(node).decls {
                    if rule.hoist == Hoist::Enclosing {
                        self.apply_rule(node, rule, Hoist::Current, out);
                    }
                }
            }
            if flags & F_SCOPE_FN != 0 || !collecting_fn {
                return;
            }
            let mut cur = node.walk();
            let children: Vec<Node<'a>> = node.children(&mut cur).collect();
            for child in children {
                self.collect(child, true, collecting_fn, out, depth + 1);
            }
            return;
        }
        self.apply_rules(node, through_block, collecting_fn, false, out);
        let mut cur = node.walk();
        let children: Vec<Node<'a>> = node.children(&mut cur).collect();
        for child in children {
            self.collect(child, through_block, collecting_fn, out, depth + 1);
        }
    }

    fn apply_rules(
        &mut self,
        node: Node<'a>,
        through_block: bool,
        collecting_fn: bool,
        owner: bool,
        out: &mut Vec<(Node<'a>, Hoist)>,
    ) {
        let t: &'static Tables = self.t;
        for &rule in &t.info(node).decls {
            let hoist = match rule.hoist {
                Hoist::FunctionIfParent(p) => {
                    if node.parent().is_some_and(|par| par.kind() == p) {
                        Hoist::Function
                    } else {
                        Hoist::Current
                    }
                }
                h => h,
            };
            let take = match hoist {
                Hoist::Enclosing => !owner && !through_block,
                Hoist::Function => collecting_fn,
                _ => !through_block,
            };
            if take {
                self.apply_rule(node, rule, hoist, out);
            }
        }
    }

    fn apply_rule(
        &mut self,
        node: Node<'a>,
        rule: &crate::lang::DeclRule,
        hoist: Hoist,
        out: &mut Vec<(Node<'a>, Hoist)>,
    ) {
        match rule.require {
            Require::None => {}
            Require::ChildToken(tok) => {
                let mut cur = node.walk();
                let ok = node
                    .children(&mut cur)
                    .any(|c| !c.is_named() && c.kind() == tok);
                if !ok {
                    return;
                }
            }
            Require::Field(f) => {
                if node.child_by_field_name(f).is_none() {
                    return;
                }
            }
        }
        let mut targets: Vec<Node<'a>> = Vec::new();
        let mut cur = node.walk();
        match rule.field {
            Some(f) => targets.extend(node.children_by_field_name(f, &mut cur)),
            None => {
                if cur.goto_first_child() {
                    loop {
                        let child = cur.node();
                        if child.is_named() && !self.t.skipped(node, cur.field_name()) {
                            targets.push(child);
                        }
                        if !cur.goto_next_sibling() {
                            break;
                        }
                    }
                }
            }
        }
        let mut idents = Vec::new();
        for target in targets {
            self.pattern(target, &mut idents, 0);
        }
        out.extend(idents.into_iter().map(|i| (i, hoist)));
    }

    /// Binding identifiers within a pattern.
    fn pattern(&self, node: Node<'a>, out: &mut Vec<Node<'a>>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let flags = self.t.flags(node);
        if flags & F_BIND_IDENT != 0 && node.child_count() == 0 {
            if !(self.t.spec.upper_pattern_not_binding && uppercase_initial(self.text(node))) {
                out.push(node);
            }
            return;
        }
        if flags & F_CONTAINER == 0 {
            return;
        }
        let mut cur = node.walk();
        if cur.goto_first_child() {
            loop {
                let child = cur.node();
                if child.is_named() && !self.t.skipped(node, cur.field_name()) {
                    self.pattern(child, out, depth + 1);
                }
                if !cur.goto_next_sibling() {
                    break;
                }
            }
        }
    }

    fn walk_children(&mut self, node: Node<'a>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let mut cur = node.walk();
        if !cur.goto_first_child() {
            return;
        }
        loop {
            let child = cur.node();
            let field = cur.field_name();
            self.walk(child, node, field, depth + 1);
            if !cur.goto_next_sibling() {
                break;
            }
        }
    }

    fn walk(&mut self, node: Node<'a>, parent: Node<'a>, field: Option<&'static str>, depth: usize) {
        let flags = self.t.flags(node);
        if flags & (F_REF_IDENT | F_BIND_IDENT) != 0 && node.child_count() == 0 {
            self.classify(node, parent, field);
            return;
        }
        if flags & F_SCOPE != 0 {
            let kind = ScopeKind {
                function: flags & F_SCOPE_FN != 0,
                class: flags & F_SCOPE_CLASS != 0,
                root: false,
            };
            let scope = self.open_scope(node, kind, true);
            self.stack.push(scope);
            self.walk_children(node, depth);
            self.stack.pop();
        } else {
            self.walk_children(node, depth);
        }
    }

    fn lookup(&self, name: &str) -> Option<Binding> {
        self.stack
            .iter()
            .rev()
            .find_map(|s| s.names.get(name).copied())
    }

    fn classify(&mut self, node: Node<'a>, parent: Node<'a>, field: Option<&'static str>) {
        let start = node.start_byte();
        if self.decl_sites.contains(&start) || self.out.contains_key(&start) {
            return;
        }
        if self.t.flags(node) & F_REF_IDENT == 0 {
            return;
        }
        let pinfo = self.t.info(parent);
        if field_matches(&pinfo.non_var, field) || (self.t.spec.non_var_hook)(node, parent, field)
        {
            return;
        }
        let name = self.text(node);
        if name.is_empty() {
            return;
        }
        match self.lookup(name) {
            Some(Binding::Var(id)) => {
                self.out.insert(start, id);
            }
            Some(Binding::Global) => {}
            None => {
                // Decision 8 (revised): free value identifiers are locals in snippets
                // *and* in corpus methods, so a field used as `player` in a method
                // body and the same `player` in a query snippet both read `#VAR`.
                if field_matches(&pinfo.callee, field)
                    || self.t.spec.builtins.contains(&name)
                    || uppercase_initial(name)
                    || (self.t.spec.free_global_hook)(node, parent, field)
                {
                    return;
                }
                let id = match self.free.get(name) {
                    Some(id) => *id,
                    None => {
                        let id = self.fresh();
                        self.free.insert(name, id);
                        id
                    }
                };
                self.out.insert(start, id);
            }
        }
    }
}
