//! Lowers a tree-sitter CST into a [`SimplifiedTree`] (Definitions 3 and 4).
//!
//! - anonymous leaves become `Keyword`s (whitespace-only ones, e.g. Go's `\n`
//!   statement terminators, are dropped);
//! - named leaves and per-language atomic literals become `Token`s;
//! - comments and `MISSING` nodes are dropped, `ERROR` nodes are lowered normally;
//! - a node left with exactly one child is replaced by that child (Def. 3 for
//!   non-keyword children; for a lone keyword this makes e.g. Java's
//!   `integral_type` the keyword `int`, as in Fig. 4's `int#`).
//!
//! Lowering works on a possibly *wrapped* text (snippet/method wrappers): only
//! leaves that start inside the [`Mapping`]'s segments are kept, and their spans
//! are remapped into the original text.

use std::sync::Arc;

use rustc_hash::FxHashMap;
use sillage_core::{Lang, Node as SNode, NodeKind, SimplifiedTree, Span, VarId};
use tree_sitter::Node;

use crate::lang::{F_ARGS, F_ATOMIC, F_DROP, F_KW_LEAF, Tables};
use crate::scope::MAX_DEPTH;

/// Piecewise-linear map from offsets in a wrapped text to offsets in the original.
#[derive(Debug, Clone)]
pub(crate) struct Mapping {
    /// `(wrapped_start, original_start, len)`, sorted and non-overlapping.
    segs: Vec<(u32, u32, u32)>,
}

impl Mapping {
    pub(crate) fn new(segs: Vec<(u32, u32, u32)>) -> Self {
        Self { segs }
    }

    /// Maps a node's wrapped `[ws, we)` to the original text. The start must lie
    /// inside a segment; an end past the snippet is clamped.
    pub(crate) fn map(&self, ws: usize, we: usize) -> Option<Span> {
        let (ws, we) = (ws as u32, we as u32);
        let i = self.segs.partition_point(|s| s.0 <= ws).checked_sub(1)?;
        let (w, o, len) = self.segs[i];
        if ws >= w + len {
            return None;
        }
        let start = o + (ws - w);
        let j = self.segs.partition_point(|s| s.0 < we).checked_sub(1)?;
        let (w2, o2, len2) = self.segs[j];
        let end = if we <= w2 + len2 {
            o2 + (we - w2)
        } else {
            o2 + len2
        };
        Some(Span::new(start, end.max(start)))
    }
}

pub(crate) struct Draft {
    kind: NodeKind,
    span: Span,
    children: Vec<Draft>,
}

pub(crate) struct Lowerer<'a> {
    pub(crate) orig: &'a str,
    pub(crate) map: &'a Mapping,
    pub(crate) t: &'static Tables,
    pub(crate) vars: &'a FxHashMap<usize, VarId>,
}

impl Lowerer<'_> {
    fn leaf_text(&self, span: Span) -> &str {
        self.orig
            .get(span.start as usize..span.end as usize)
            .unwrap_or("")
    }

    fn leaf(&self, node: Node<'_>, keyword: bool) -> Option<Draft> {
        let span = self.map.map(node.start_byte(), node.end_byte())?;
        if span.is_empty() {
            return None;
        }
        let text = self.leaf_text(span);
        if text.trim().is_empty() {
            return None;
        }
        let kind = if keyword {
            NodeKind::Keyword { text: text.into() }
        } else {
            NodeKind::Token {
                text: text.into(),
                var: self.vars.get(&node.start_byte()).copied(),
            }
        };
        Some(Draft {
            kind,
            span,
            children: Vec::new(),
        })
    }

    pub(crate) fn lower(&self, node: Node<'_>, depth: usize) -> Option<Draft> {
        if node.is_missing() || node.start_byte() >= node.end_byte() {
            return None;
        }
        let info = self.t.info(node);
        let flags = info.flags;
        if flags & F_DROP != 0 || (node.is_extra() && node.is_named() && !node.is_error()) {
            return None;
        }
        let named = node.is_named();
        if node.child_count() == 0 {
            return self.leaf(node, !named || flags & F_KW_LEAF != 0);
        }
        if (flags & F_ATOMIC != 0 && !has_interpolation(node)) || depth > MAX_DEPTH {
            return self.leaf(node, false);
        }

        let mut children: Vec<Draft> = Vec::with_capacity(node.child_count());
        let call = info.call;
        let mut regroup_at: Option<usize> = None;
        let mut cur = node.walk();
        if cur.goto_first_child() {
            loop {
                let child = cur.node();
                let field = cur.field_name();
                if let Some(c) = call {
                    if c.regroup_from.is_some() && field == c.regroup_from && regroup_at.is_none()
                    {
                        regroup_at = Some(children.len());
                    }
                    if field == Some(c.args_field) && self.t.flags(child) & F_ARGS != 0 {
                        let mut gc = child.walk();
                        let grand: Vec<Node<'_>> = child.children(&mut gc).collect();
                        for g in grand {
                            if let Some(d) = self.lower(g, depth + 1) {
                                children.push(d);
                            }
                        }
                        if !cur.goto_next_sibling() {
                            break;
                        }
                        continue;
                    }
                }
                if let Some(d) = self.lower(child, depth + 1) {
                    children.push(d);
                }
                if !cur.goto_next_sibling() {
                    break;
                }
            }
        }
        if let Some(k) = regroup_at
            && k > 0
            && children.len() - k >= 2
            && children[..k].iter().any(|c| !c.kind.is_keyword())
        {
            let tail = children.split_off(k);
            if let Some(d) = finalize(tail) {
                children.push(d);
            }
        }
        if node.kind() == "for_statement" {
            group_for_control(&mut children);
        }
        finalize(children)
    }
}

fn is_kw(d: &Draft, s: &str) -> bool {
    matches!(&d.kind, NodeKind::Keyword { text } if &**text == s)
}

/// C-style `for (init; cond; update) body` (Java, JS/TS): groups the header into
/// one `#;#;#` node so the loop reads `for(#)#` as in the paper's Fig. 4. A
/// declaration initializer's own trailing `;` (tree-sitter includes it in
/// `local_variable_declaration` / `lexical_declaration`) is hoisted into the
/// header, so `int i = 0` reads `int#` (Table 2) and declaration and expression
/// initializers give the same loop label.
fn group_for_control(children: &mut Vec<Draft>) {
    let Some(open) = children.iter().position(|c| is_kw(c, "(")) else {
        return;
    };
    let Some(close) = children.iter().rposition(|c| is_kw(c, ")")) else {
        return;
    };
    if close <= open + 1 {
        return;
    }
    let mut header: Vec<Draft> = children.drain(open + 1..close).collect();
    // Hoist a trailing `;` out of a declaration initializer.
    if let Some(first) = header.first_mut()
        && first.kind.is_internal()
        && first.children.len() >= 2
        && first.children.last().is_some_and(|c| is_kw(c, ";"))
    {
        let semi = first.children.pop().expect("trailing `;`");
        let inner = std::mem::take(&mut first.children);
        let rebuilt = finalize(inner).expect("non-empty initializer");
        header[0] = rebuilt;
        header.insert(1, semi);
    }
    let semis = header.iter().filter(|c| is_kw(c, ";")).count();
    if semis != 2 || header.iter().all(|c| c.kind.is_keyword()) {
        // Not a classic three-part header (Python `for` has no parens), or `for (;;)`
        // whose `;`s must stay keywords of the loop itself: restore.
        let tail = children.split_off(open + 1);
        children.extend(header);
        children.extend(tail);
        return;
    }
    if let Some(group) = finalize(header) {
        children.insert(open + 1, group);
    }
}

/// Interpolated string literals (JS/TS template strings with `${…}`, Python
/// f-strings with `{…}`) are lowered structurally instead of atomically, so the
/// embedded expressions keep their structure and local variables (an atomic
/// `` `HTTP ${res.status}` `` would hide the renamable local `res`).
fn has_interpolation(node: Node<'_>) -> bool {
    let mut cur = node.walk();
    node.named_children(&mut cur)
        .any(|c| matches!(c.kind(), "template_substitution" | "interpolation"))
}

/// Keeps `#`-slot indices within `u16` for pathological flat nodes (huge literals).
const MAX_SLOTS: usize = 30_000;

fn finalize(mut children: Vec<Draft>) -> Option<Draft> {
    if children.len() > MAX_SLOTS {
        let mut grouped = Vec::new();
        let mut rest = children;
        while !rest.is_empty() {
            let tail = rest.split_off(rest.len().min(MAX_SLOTS));
            grouped.extend(finalize(rest));
            rest = tail;
        }
        children = grouped;
        if children.len() > MAX_SLOTS {
            return finalize(children);
        }
    }
    match children.len() {
        0 => None,
        1 => children.pop(),
        _ => {
            let kinds: Vec<&NodeKind> = children.iter().map(|c| &c.kind).collect();
            let label = SimplifiedTree::make_label(&kinds).into_boxed_str();
            let start = children.iter().map(|c| c.span.start).min().unwrap_or(0);
            let end = children.iter().map(|c| c.span.end).max().unwrap_or(start);
            Some(Draft {
                kind: NodeKind::Internal { label },
                span: Span::new(start, end),
                children,
            })
        }
    }
}

/// Flattens a draft into preorder `nodes` with source-order `leaves`.
pub(crate) fn build(lang: Lang, source: Arc<str>, root: Option<Draft>) -> SimplifiedTree {
    let mut tree = SimplifiedTree::empty(lang, source);
    if let Some(root) = root {
        flatten(root, None, 0, &mut tree.nodes, &mut tree.leaves);
    }
    tree
}

fn flatten(
    draft: Draft,
    parent: Option<u32>,
    index_in_parent: u16,
    nodes: &mut Vec<SNode>,
    leaves: &mut Vec<u32>,
) -> u32 {
    let id = nodes.len() as u32;
    if draft.kind.is_token() {
        leaves.push(id);
    }
    nodes.push(SNode {
        kind: draft.kind,
        parent,
        index_in_parent,
        children: Vec::new(),
        span: draft.span,
        dropped_children: false,
    });
    let mut ids = Vec::with_capacity(draft.children.len());
    let mut slot: u16 = 0;
    for child in draft.children {
        let idx = if child.kind.is_keyword() {
            0
        } else {
            slot = slot.saturating_add(1);
            slot
        };
        ids.push(flatten(child, Some(id), idx, nodes, leaves));
    }
    nodes[id as usize].children = ids;
    id
}
