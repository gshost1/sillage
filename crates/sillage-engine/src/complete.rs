//! Presentation post-pass for recommendations (`Params::complete_calls`).
//!
//! Intersection keeps a leaf only when its features occur in every cluster member,
//! and every feature of a non-variable token embeds the token text. Two calls that
//! differ only in the callee (`setupUI(x)` vs `setupUIToHideKeyBoardOnTouch(x)`)
//! therefore keep the argument but drop the callee, rendering `(x)`. This pass
//! restores, from the first cluster member, a small dropped child (at most
//! [`MAX_HOLE_TOKENS`] tokens) of a kept node that sits next to one of the node's
//! keywords: a callee before `(`, a receiver or member around `.`, the type after
//! `instanceof` / `new` / in a cast, so the rendering has no syntactic holes.
//!
//! A larger dropped member after `.` / `?.` / `::` (a whole regrouped call such
//! as `setOnTouchListener(new OnTouchListener() { … })`) gets only its head token
//! back (`setOnTouchListener`), so the receiver never dangles as `view.`; the
//! dropped arguments render as a placeholder.

use rustc_hash::FxHashSet;
use sillage_core::{NodeKind, SimplifiedTree, Span};

/// Largest dropped child (in tokens) the completion pass restores.
pub const MAX_HOLE_TOKENS: usize = 3;

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
            let child = &base.nodes[c as usize];
            if child.kind.is_keyword() || live[c as usize] {
                continue;
            }
            // Statement / element delimiters do not mark a hole: a dropped statement
            // next to a block's `{` or a dropped list element is a real removal.
            let anchor = |x: u32| match &base.node(x).kind {
                NodeKind::Keyword { text } => !matches!(&**text, "{" | "}" | ";" | ","),
                _ => false,
            };
            let next_kw = n.children.get(k + 1).is_some_and(|&x| anchor(x));
            let prev_kw = k > 0 && anchor(n.children[k - 1]);
            let after_member = k > 0
                && matches!(&base.node(n.children[k - 1]).kind,
                    NodeKind::Keyword { text } if matches!(&**text, "." | "?." | "::"));
            if !(next_kw || prev_kw) {
                continue;
            }
            // Leaves of the dropped child; restore only small holes.
            let mut leaves = Vec::new();
            let mut stack = vec![c];
            while let Some(x) = stack.pop() {
                let xn = &base.nodes[x as usize];
                if xn.kind.is_token() {
                    leaves.push(leaf_pos[x as usize]);
                }
                stack.extend(xn.children.iter().copied());
            }
            if leaves.is_empty() {
                continue;
            }
            if leaves.len() > MAX_HOLE_TOKENS {
                if !after_member {
                    continue;
                }
                // Restore the member's head (its first token in source order).
                let head = *leaves.iter().min().expect("non-empty");
                leaves = vec![head];
            }
            for li in leaves {
                keep[li] = true;
            }
            added = true;
        }
    }
    if added { base.retain(&keep) } else { rec.clone() }
}
