//! Test-only helpers: a tiny tree-building DSL and local featurizers, so engine tests
//! never depend on `sillage-parse` / `sillage-index`.
#![allow(dead_code)]

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use sillage_core::{
    FeatureBag, Featurizer, Lang, LeafFeatures, MethodId, Node, NodeId, NodeKind, SimplifiedTree,
    Span,
};
use sillage_engine::{Ranked, prune};

pub use sillage_core::Featurizer as _;

/// Tree-building DSL node.
#[derive(Clone)]
pub enum B {
    Int(Vec<B>),
    Kw(&'static str),
    Tok(&'static str),
    Var(&'static str),
}

pub fn n(children: Vec<B>) -> B {
    B::Int(children)
}
pub fn k(s: &'static str) -> B {
    B::Kw(s)
}
pub fn t(s: &'static str) -> B {
    B::Tok(s)
}
pub fn v(s: &'static str) -> B {
    B::Var(s)
}

struct Ctx {
    source: String,
    nodes: Vec<Node>,
    leaves: Vec<NodeId>,
    vars: HashMap<&'static str, u32>,
}

/// Collapses single-child internal nodes without keywords (Definition 3).
fn normalize(b: B) -> B {
    match b {
        B::Int(ch) => {
            let mut ch: Vec<B> = ch.into_iter().map(normalize).collect();
            if ch.len() == 1 && !matches!(ch[0], B::Kw(_)) {
                ch.pop().unwrap()
            } else {
                B::Int(ch)
            }
        }
        other => other,
    }
}

fn emit(ctx: &mut Ctx, b: &B, parent: Option<NodeId>, idx: u16) -> NodeId {
    let id = ctx.nodes.len() as NodeId;
    let push_leaf = |ctx: &mut Ctx, kind: NodeKind, text: &str| {
        if !ctx.source.is_empty() {
            ctx.source.push(' ');
        }
        let start = ctx.source.len() as u32;
        ctx.source.push_str(text);
        let end = ctx.source.len() as u32;
        ctx.nodes.push(Node {
            kind,
            parent,
            index_in_parent: idx,
            children: vec![],
            span: Span::new(start, end),
            dropped_children: false,
        });
    };
    match b {
        B::Kw(s) => push_leaf(ctx, NodeKind::Keyword { text: (*s).into() }, s),
        B::Tok(s) => {
            push_leaf(ctx, NodeKind::Token { text: (*s).into(), var: None }, s);
            ctx.leaves.push(id);
        }
        B::Var(s) => {
            let next = ctx.vars.len() as u32;
            let var = *ctx.vars.entry(s).or_insert(next);
            push_leaf(ctx, NodeKind::Token { text: (*s).into(), var: Some(var) }, s);
            ctx.leaves.push(id);
        }
        B::Int(ch) => {
            ctx.nodes.push(Node {
                kind: NodeKind::Internal { label: "".into() },
                parent,
                index_in_parent: idx,
                children: vec![],
                span: Span::default(),
                dropped_children: false,
            });
            let mut slot = 0u16;
            let mut kids = vec![];
            for c in ch {
                let ci = if matches!(c, B::Kw(_)) {
                    0
                } else {
                    slot += 1;
                    slot
                };
                kids.push(emit(ctx, c, Some(id), ci));
            }
            let kinds: Vec<&NodeKind> = kids.iter().map(|&c| &ctx.nodes[c as usize].kind).collect();
            let label = SimplifiedTree::make_label(&kinds).into_boxed_str();
            let start = ctx.nodes[kids[0] as usize].span.start;
            let end = ctx.nodes[*kids.last().unwrap() as usize].span.end;
            let node = &mut ctx.nodes[id as usize];
            node.kind = NodeKind::Internal { label };
            node.children = kids;
            node.span = Span::new(start, end);
        }
    }
    id
}

pub fn build(b: B) -> SimplifiedTree {
    let b = normalize(b);
    let mut ctx = Ctx { source: String::new(), nodes: vec![], leaves: vec![], vars: HashMap::new() };
    emit(&mut ctx, &b, None, 0);
    let tree = SimplifiedTree {
        lang: Lang::Java,
        source: Arc::from(ctx.source),
        nodes: ctx.nodes,
        leaves: ctx.leaves,
    };
    tree.validate().expect("hand-built tree must be valid");
    tree
}

/// Flat tree of plain tokens (e.g. `["a", "x", "y"]`).
pub fn flat(tokens: &[&'static str]) -> SimplifiedTree {
    build(n(tokens.iter().map(|s| t(s)).collect()))
}

/// Keyword and token texts in preorder, space-separated.
pub fn text(tree: &SimplifiedTree) -> String {
    tree.nodes
        .iter()
        .filter(|n| !n.kind.is_internal())
        .map(|n| match &n.kind {
            NodeKind::Keyword { text } | NodeKind::Token { text, .. } => text.as_ref(),
            NodeKind::Internal { .. } => unreachable!(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn leaf_texts(tree: &SimplifiedTree) -> Vec<String> {
    tree.leaves.iter().map(|&l| tree.label(l).to_string()).collect()
}

/// Structural rendering (labels only, spans ignored).
pub fn sexpr(tree: &SimplifiedTree) -> String {
    fn go(t: &SimplifiedTree, id: NodeId, out: &mut String) {
        let node = t.node(id);
        if node.kind.is_internal() {
            out.push('(');
            out.push_str(t.label(id));
            for &c in &node.children {
                out.push(' ');
                go(t, c, out);
            }
            out.push(')');
        } else {
            out.push_str(t.label(id));
        }
    }
    let mut s = String::new();
    if !tree.is_empty() {
        go(tree, 0, &mut s);
    }
    s
}

fn h(parts: impl Hash) -> u64 {
    let mut s = DefaultHasher::new();
    parts.hash(&mut s);
    s.finish()
}

/// Aroma-style featurizer (token, parent, sibling and variable-usage features, §3.2),
/// implemented locally for tests.
pub struct TestFeaturizer;

impl Featurizer for TestFeaturizer {
    fn featurize(&self, tree: &SimplifiedTree) -> LeafFeatures {
        let name = |id: NodeId| match &tree.node(id).kind {
            NodeKind::Token { var: Some(_), .. } => "#VAR".to_string(),
            _ => tree.label(id).to_string(),
        };
        let context = |id: NodeId| {
            tree.ancestors(id)
                .next()
                .map(|(p, i)| (i, tree.label(p).to_string()))
                .unwrap_or((0, String::new()))
        };
        let mut last_use: HashMap<u32, NodeId> = HashMap::new();
        let per_leaf = tree
            .leaves
            .iter()
            .enumerate()
            .map(|(pos, &leaf)| {
                let me = name(leaf);
                let mut f = vec![h(("tok", &me))];
                for (p, i) in tree.ancestors(leaf).take(3) {
                    f.push(h(("par", &me, i, tree.label(p))));
                }
                if let Some(&next) = tree.leaves.get(pos + 1) {
                    f.push(h(("sib>", &me, name(next))));
                }
                if pos > 0 {
                    f.push(h(("sib<", name(tree.leaves[pos - 1]), &me)));
                }
                if let NodeKind::Token { var: Some(var), .. } = tree.node(leaf).kind {
                    if let Some(&prev) = last_use.get(&var) {
                        f.push(h(("var", context(prev), context(leaf))));
                    }
                    last_use.insert(var, leaf);
                }
                FeatureBag::from_unsorted(f)
            })
            .collect();
        LeafFeatures { per_leaf }
    }
}

/// One feature per leaf: its token text. Makes multiset arithmetic easy to reason about.
pub struct TokenFeaturizer;

impl Featurizer for TokenFeaturizer {
    fn featurize(&self, tree: &SimplifiedTree) -> LeafFeatures {
        LeafFeatures {
            per_leaf: tree
                .leaves
                .iter()
                .map(|&l| FeatureBag::from_unsorted([h(tree.label(l))]))
                .collect(),
        }
    }
}

pub fn fq(tree: &SimplifiedTree, fz: &dyn Featurizer) -> FeatureBag {
    fz.featurize(tree).total()
}

/// Builds a `Ranked` entry directly (bypassing rerank ordering).
pub fn ranked(id: u32, tree: SimplifiedTree, fq: &FeatureBag, fz: &dyn Featurizer) -> Ranked {
    let pruned = prune(fq, &tree, &fz.featurize(&tree));
    let total = fq.total();
    Ranked {
        id: MethodId(id),
        overlap: 0,
        sim: pruned.sim,
        full_sim: 0,
        size: 0,
        score: if total == 0 { 0.0 } else { pruned.sim as f64 / total as f64 },
        pruned,
        tree,
    }
}
