//! Aroma structural featurization (paper §3.2).
//!
//! For each non-keyword leaf `n` four feature families are extracted:
//! token, parent (up to three ancestors, depth deliberately omitted), sibling
//! (`Prev`/`Next` in source order) and variable-usage (contexts of consecutive
//! uses of the same local variable).

use std::fmt;

use rustc_hash::FxHashMap;
use sillage_core::{FeatureBag, FeatureId, Featurizer, LeafFeatures, NodeId, NodeKind, SimplifiedTree};

use crate::hash::{CTX_MEMBER, CTX_SLOT, Encoder, TAG_PARENT, TAG_SIBLING, TAG_TOKEN, TAG_VAR_USAGE};

const VAR: &str = "#VAR";
const MEMBER_LABEL: &str = "#.#";
const PARENT_DEPTH: usize = 3;

/// The Aroma featurizer of paper §3.2, with statement-sequence normalisation.
///
/// A parent or usage context whose label is a statement sequence (`{###}`, or a
/// brace-less `###` such as a Python block or a multi-statement query root) is
/// emitted as `(n, 0, {#})`: the sequence's arity and the statement's position in
/// it are dropped. Otherwise the 5 top-level statements of a partial query (root
/// `#####`, slots 1..5) never share a parent feature with the same statements in
/// the corpus method body (`{############}`, slots 7..11). [`PaperFeaturizer`] keeps
/// the literal labels.
#[derive(Debug, Clone, Default)]
pub struct AromaFeaturizer;

/// The literal §3.2 featurizer (no statement-sequence normalisation), for ablation.
/// Indexes built with it must be queried with it.
#[derive(Debug, Clone, Default)]
pub struct PaperFeaturizer;

impl Featurizer for AromaFeaturizer {
    fn featurize(&self, tree: &SimplifiedTree) -> LeafFeatures {
        featurize_with(tree, true)
    }
}

impl Featurizer for PaperFeaturizer {
    fn featurize(&self, tree: &SimplifiedTree) -> LeafFeatures {
        featurize_with(tree, false)
    }
}

const SEQ_LABEL: &str = "{#}";

/// `{#…#}` or `##…#` (two or more): a statement sequence / block.
fn is_sequence(label: &str) -> bool {
    let inner = label.strip_prefix('{').and_then(|l| l.strip_suffix('}'));
    match inner {
        Some(i) => !i.is_empty() && i.bytes().all(|b| b == b'#'),
        None => label.len() >= 2 && label.bytes().all(|b| b == b'#'),
    }
}

#[inline]
fn norm_slot(normalize: bool, i: u16, label: &str) -> (u16, &str) {
    if normalize && is_sequence(label) { (0, SEQ_LABEL) } else { (i, label) }
}

fn featurize_with(tree: &SimplifiedTree, normalize: bool) -> LeafFeatures {
    let view = TreeView::new(tree, normalize);
    let mut enc = Encoder::default();
    let mut feats = Vec::new();
    let per_leaf = (0..tree.leaves.len())
        .map(|idx| {
            feats.clear();
            view.leaf_features(idx, &mut feats);
            FeatureBag::from_unsorted(feats.iter().map(|f| f.id(&mut enc)))
        })
        .collect();
    LeafFeatures { per_leaf }
}

/// Human-readable features of leaf `leaf` (an index into `tree.leaves`), in the
/// notation of paper Table 2: token, parent, sibling, then variable-usage features.
pub fn feature_debug(tree: &SimplifiedTree, leaf: usize) -> Vec<String> {
    if leaf >= tree.leaves.len() {
        return Vec::new();
    }
    let view = TreeView::new(tree, true);
    let mut feats = Vec::new();
    view.leaf_features(leaf, &mut feats);
    feats.iter().map(ToString::to_string).collect()
}

/// Usage context `C(x)` of a local-variable leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ctx<'a> {
    /// `(i, L(t))`: `x` is the `i`-th child of a parent labelled `L(t) != "#.#"`.
    Slot(u16, &'a str),
    /// First non-local token in `N(t)` when `L(t) == "#.#"` (e.g. `foo` in `x.foo()`).
    Member(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Feature<'a> {
    Token(&'a str),
    Parent(&'a str, u16, &'a str),
    Sibling(&'a str, &'a str),
    VarUsage(Ctx<'a>, Ctx<'a>),
}

impl Feature<'_> {
    pub(crate) fn id(&self, enc: &mut Encoder) -> FeatureId {
        match *self {
            Feature::Token(n) => {
                enc.tag(TAG_TOKEN).str(n);
            }
            Feature::Parent(n, i, l) => {
                enc.tag(TAG_PARENT).str(n).num(i).str(l);
            }
            Feature::Sibling(a, b) => {
                enc.tag(TAG_SIBLING).str(a).str(b);
            }
            Feature::VarUsage(a, b) => {
                enc.tag(TAG_VAR_USAGE);
                encode_ctx(enc, a);
                encode_ctx(enc, b);
            }
        }
        enc.finish()
    }
}

fn encode_ctx(enc: &mut Encoder, ctx: Ctx<'_>) {
    match ctx {
        Ctx::Slot(i, l) => {
            enc.tag(CTX_SLOT).num(i).str(l);
        }
        Ctx::Member(m) => {
            enc.tag(CTX_MEMBER).str(m);
        }
    }
}

impl fmt::Display for Ctx<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ctx::Slot(i, l) => write!(f, "({i}, {l})"),
            Ctx::Member(m) => f.write_str(m),
        }
    }
}

impl fmt::Display for Feature<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Feature::Token(n) => f.write_str(n),
            Feature::Parent(n, i, l) => write!(f, "({n}, {i}, {l})"),
            Feature::Sibling(a, b) => write!(f, "({a}, {b})"),
            Feature::VarUsage(a, b) => write!(f, "({a}, {b})"),
        }
    }
}

/// Per-tree precomputation shared by all leaves.
struct TreeView<'a> {
    tree: &'a SimplifiedTree,
    /// Token string per leaf (`#VAR` for locals).
    names: Vec<&'a str>,
    /// Usage context per leaf; `Some` only for local variables.
    ctx: Vec<Option<Ctx<'a>>>,
    prev_use: Vec<Option<usize>>,
    next_use: Vec<Option<usize>>,
    normalize: bool,
}

impl<'a> TreeView<'a> {
    fn new(tree: &'a SimplifiedTree, normalize: bool) -> Self {
        let n = tree.leaves.len();
        let mut names = Vec::with_capacity(n);
        let mut ctx = Vec::with_capacity(n);
        let mut prev_use = vec![None; n];
        let mut next_use = vec![None; n];
        let mut last: FxHashMap<u32, usize> = FxHashMap::default();
        let mut member_cache: FxHashMap<NodeId, Option<&'a str>> = FxHashMap::default();

        for (idx, &node_id) in tree.leaves.iter().enumerate() {
            let var = leaf_var(tree, node_id);
            names.push(if var.is_some() { VAR } else { tree.label(node_id) });
            match var {
                Some(vid) => {
                    if let Some(p) = last.insert(vid, idx) {
                        prev_use[idx] = Some(p);
                        next_use[p] = Some(idx);
                    }
                    ctx.push(Some(context(tree, node_id, &mut member_cache, normalize)));
                }
                None => ctx.push(None),
            }
        }
        Self { tree, names, ctx, prev_use, next_use, normalize }
    }

    fn leaf_features(&self, idx: usize, out: &mut Vec<Feature<'a>>) {
        let tree = self.tree;
        let node_id = tree.leaves[idx];
        let n = self.names[idx];

        out.push(Feature::Token(n));

        for (anc, i) in tree.ancestors(node_id).take(PARENT_DEPTH) {
            let (i, l) = norm_slot(self.normalize, i, tree.label(anc));
            out.push(Feature::Parent(n, i, l));
        }

        if idx > 0 {
            out.push(Feature::Sibling(self.names[idx - 1], n));
        }
        if idx + 1 < tree.leaves.len() {
            out.push(Feature::Sibling(n, self.names[idx + 1]));
        }

        if let Some(c) = self.ctx[idx] {
            if let Some(p) = self.prev_use[idx] {
                out.push(Feature::VarUsage(self.ctx[p].expect("prev use is a var"), c));
            }
            if let Some(nx) = self.next_use[idx] {
                out.push(Feature::VarUsage(c, self.ctx[nx].expect("next use is a var")));
            }
        }
    }
}

fn leaf_var(tree: &SimplifiedTree, node_id: NodeId) -> Option<u32> {
    match &tree.node(node_id).kind {
        NodeKind::Token { var, .. } => *var,
        _ => None,
    }
}

fn context<'a>(
    tree: &'a SimplifiedTree,
    node_id: NodeId,
    member_cache: &mut FxHashMap<NodeId, Option<&'a str>>,
    normalize: bool,
) -> Ctx<'a> {
    let node = tree.node(node_id);
    let slot = node.index_in_parent;
    let Some(parent) = node.parent else {
        return Ctx::Slot(slot, "");
    };
    let label = tree.label(parent);
    if label != MEMBER_LABEL {
        let (slot, label) = norm_slot(normalize, slot, label);
        return Ctx::Slot(slot, label);
    }
    let member = *member_cache
        .entry(parent)
        .or_insert_with(|| first_non_local_token(tree, parent));
    match member {
        Some(m) => Ctx::Member(m),
        None => Ctx::Slot(slot, label),
    }
}

/// First non-local token of `N(t)` in source (preorder) order.
fn first_non_local_token(tree: &SimplifiedTree, t: NodeId) -> Option<&str> {
    let mut stack = vec![t];
    while let Some(id) = stack.pop() {
        let node = tree.node(id);
        match &node.kind {
            NodeKind::Token { text, var: None } => return Some(text),
            NodeKind::Internal { .. } => stack.extend(node.children.iter().rev().copied()),
            NodeKind::Token { .. } | NodeKind::Keyword { .. } => {}
        }
    }
    None
}
