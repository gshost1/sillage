//! Presentation post-pass for recommendations (`Params::complete_calls`).
//!
//! Intersection keeps a leaf only when its features occur in every cluster member,
//! and every feature of a non-variable token embeds the token text. Two calls that
//! differ only in the callee (`setupUI(x)` vs `setupUIToHideKeyBoardOnTouch(x)`)
//! therefore keep the argument but drop the callee, rendering `(x)`. This pass
//! restores, from the first cluster member, a dropped token that is the direct
//! child of a kept node and sits right before `(` (callee, constructed type) or
//! right after `.` (member name).

use rustc_hash::FxHashSet;
use sillage_core::{NodeKind, SimplifiedTree, Span};

fn is_kw(tree: &SimplifiedTree, id: u32, s: &str) -> bool {
    matches!(&tree.node(id).kind, NodeKind::Keyword { text } if &**text == s)
}

/// `rec` must be derived from `base` by `retain` (spans are shared). Returns `rec`
/// unchanged when nothing needs restoring.
pub fn complete_calls(rec: &SimplifiedTree, base: &SimplifiedTree) -> SimplifiedTree {
    if rec.leaves.is_empty() || base.leaves.is_empty() {
        return rec.clone();
    }
    let kept_spans: FxHashSet<Span> = rec.leaves.iter().map(|&l| rec.node(l).span).collect();
    let mut leaf_pos = vec![usize::MAX; base.nodes.len()];
    for (i, &l) in base.leaves.iter().enumerate() {
        leaf_pos[l as usize] = i;
    }
    let mut keep: Vec<bool> = base.leaves.iter().map(|&l| kept_spans.contains(&base.node(l).span)).collect();

    // live[n]: subtree of `n` holds a kept leaf (reverse preorder = children first).
    let mut live = vec![false; base.nodes.len()];
    for id in (0..base.nodes.len()).rev() {
        let n = &base.nodes[id];
        live[id] = match n.kind {
            NodeKind::Token { .. } => keep[leaf_pos[id]],
            NodeKind::Keyword { .. } => false,
            NodeKind::Internal { .. } => n.children.iter().any(|&c| live[c as usize]),
        };
    }

    let mut added = false;
    for (id, n) in base.nodes.iter().enumerate() {
        if !n.kind.is_internal() || !live[id] {
            continue;
        }
        for (k, &c) in n.children.iter().enumerate() {
            let li = leaf_pos[c as usize];
            if li == usize::MAX || keep[li] {
                continue;
            }
            let before_paren = n.children.get(k + 1).is_some_and(|&nx| is_kw(base, nx, "("));
            let after_dot = k > 0 && is_kw(base, n.children[k - 1], ".");
            if before_paren || after_dot {
                keep[li] = true;
                added = true;
            }
        }
    }
    if added { base.retain(&keep) } else { rec.clone() }
}
