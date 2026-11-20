use std::sync::Arc;

use anyhow::{bail, ensure};
use serde::{Deserialize, Serialize};

use crate::method::Lang;

pub type NodeId = u32;
pub type VarId = u32;

/// Byte offsets `[start, end)` into `SimplifiedTree::source`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    pub fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(self) -> bool {
        self.start >= self.end
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    /// Internal simplified parse tree node with label `L(t)` (e.g. `"if##"`, `"#.#"`, `"(#)"`).
    Internal { label: Box<str> },
    /// Fixed language vocabulary token (keywords, operators, punctuation).
    Keyword { text: Box<str> },
    /// Non-keyword leaf token (identifiers, method/field names, literals).
    /// `var == Some(id)` marks a local variable; equal `VarId`s denote the same binding.
    Token { text: Box<str>, var: Option<VarId> },
}

impl NodeKind {
    pub fn is_internal(&self) -> bool {
        matches!(self, NodeKind::Internal { .. })
    }

    pub fn is_keyword(&self) -> bool {
        matches!(self, NodeKind::Keyword { .. })
    }

    pub fn is_token(&self) -> bool {
        matches!(self, NodeKind::Token { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub kind: NodeKind,
    pub parent: Option<NodeId>,
    /// 1-based `#`-slot index among non-`Keyword` (`Internal` or `Token`) children of `parent`.
    /// `0` for the root node or for `Keyword` nodes.
    pub index_in_parent: u16,
    pub children: Vec<NodeId>,
    pub span: Span,
    pub dropped_children: bool,
}

/// Simplified Parse Tree (Section 3.1, Definition 3).
///
/// Nodes are stored in preorder (`nodes[0]` is root when non-empty).
/// `leaves` holds all `NodeKind::Token` node IDs in left-to-right source order (`N(t)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimplifiedTree {
    pub lang: Lang,
    pub source: Arc<str>,
    pub nodes: Vec<Node>,
    pub leaves: Vec<NodeId>,
}

pub struct Ancestors<'a> {
    tree: &'a SimplifiedTree,
    cur: NodeId,
}

impl<'a> Iterator for Ancestors<'a> {
    type Item = (NodeId, u16);

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.tree.nodes.get(self.cur as usize)?;
        let parent = node.parent?;
        let idx = node.index_in_parent;
        self.cur = parent;
        Some((parent, idx))
    }
}

impl SimplifiedTree {
    pub fn empty(lang: Lang, source: Arc<str>) -> Self {
        Self {
            lang,
            source,
            nodes: Vec::new(),
            leaves: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn node(&self, n: NodeId) -> &Node {
        &self.nodes[n as usize]
    }

    /// Returns the label `L(t)` for internal nodes, or the token text for leaves.
    pub fn label(&self, n: NodeId) -> &str {
        match &self.nodes[n as usize].kind {
            NodeKind::Internal { label } => label,
            NodeKind::Keyword { text } => text,
            NodeKind::Token { text, .. } => text,
        }
    }

    /// Walks up from `n`, yielding `(ancestor_id, child_slot_index_on_path)`.
    ///
    /// For a leaf `n`:
    /// - 1st item: `(t_1, i_1)` where `n` is the `i_1`-th `#`-slot child of `t_1`
    /// - 2nd item: `(t_2, i_2)` where `t_1` is the `i_2`-th `#`-slot child of `t_2`
    /// - 3rd item: `(t_3, i_3)` where `t_2` is the `i_3`-th `#`-slot child of `t_3`
    pub fn ancestors(&self, n: NodeId) -> Ancestors<'_> {
        Ancestors { tree: self, cur: n }
    }

    /// Definition 4: concatenates `Keyword` text verbatim and `#` for `Token` or `Internal`.
    pub fn make_label(children: &[&NodeKind]) -> String {
        let mut out = String::new();
        for kind in children {
            match kind {
                NodeKind::Keyword { text } => out.push_str(text),
                NodeKind::Internal { .. } | NodeKind::Token { .. } => out.push('#'),
            }
        }
        out
    }

    /// Builds a pruned `SimplifiedTree` retaining the leaves selected by `keep` (`keep.len() == self.leaves.len()`),
    /// every internal node on a root-to-leaf path, and the `Keyword` children of kept internal nodes.
    ///
    /// Single-child internal nodes containing no keywords are collapsed per Definition 3,
    /// labels and 1-based `#`-slot indices are recomputed, `dropped_children` is marked on
    /// internal nodes that lost children, and the original `source` `Arc<str>` is shared.
    pub fn retain(&self, keep: &[bool]) -> SimplifiedTree {
        assert_eq!(
            keep.len(),
            self.leaves.len(),
            "keep mask length must match tree.leaves.len()"
        );
        if self.nodes.is_empty() || !keep.iter().any(|&b| b) {
            return SimplifiedTree::empty(self.lang, Arc::clone(&self.source));
        }

        let mut marked = vec![false; self.nodes.len()];
        for (&leaf_id, &k) in self.leaves.iter().zip(keep.iter()) {
            if !k {
                continue;
            }
            let mut cur = Some(leaf_id);
            while let Some(id) = cur {
                if marked[id as usize] {
                    break;
                }
                marked[id as usize] = true;
                cur = self.nodes[id as usize].parent;
            }
        }

        // Keep token-less subtrees (e.g. `{}`, `[]`, `()` literals) of kept nodes
        // whole: they behave like compound keywords and carry no features.
        let mut has_token = vec![false; self.nodes.len()];
        for id in (0..self.nodes.len()).rev() {
            let n = &self.nodes[id];
            has_token[id] = n.kind.is_token() || n.children.iter().any(|&c| has_token[c as usize]);
        }
        for id in 0..self.nodes.len() {
            if marked[id] && self.nodes[id].kind.is_internal() {
                for &child in &self.nodes[id].children {
                    let c = child as usize;
                    if self.nodes[c].kind.is_internal() && !has_token[c] {
                        let mut stack = vec![child];
                        while let Some(x) = stack.pop() {
                            marked[x as usize] = true;
                            stack.extend(self.nodes[x as usize].children.iter().copied());
                        }
                    }
                }
            }
        }

        // Also mark Keyword children of every kept Internal node.
        for id in 0..self.nodes.len() {
            if marked[id] && self.nodes[id].kind.is_internal() {
                for &child in &self.nodes[id].children {
                    if self.nodes[child as usize].kind.is_keyword() {
                        marked[child as usize] = true;
                    }
                }
            }
        }

        // Rebuild in preorder, collapsing single-child non-keyword internals (Def. 3).
        #[derive(Clone)]
        struct DraftNode {
            kind: NodeKind,
            children: Vec<DraftNode>,
            span: Span,
            dropped_children: bool,
        }

        fn build_draft(tree: &SimplifiedTree, marked: &[bool], id: NodeId) -> Option<DraftNode> {
            if !marked[id as usize] {
                return None;
            }
            let node = &tree.nodes[id as usize];
            match &node.kind {
                NodeKind::Keyword { .. } | NodeKind::Token { .. } => Some(DraftNode {
                    kind: node.kind.clone(),
                    children: Vec::new(),
                    span: node.span,
                    dropped_children: false,
                }),
                NodeKind::Internal { .. } => {
                    let mut kept_children = Vec::new();
                    let mut dropped = node.dropped_children;
                    for &child_id in &node.children {
                        if let Some(child_draft) = build_draft(tree, marked, child_id) {
                            kept_children.push(child_draft);
                        } else {
                            dropped = true;
                        }
                    }
                    if kept_children.is_empty() {
                        return None;
                    }
                    // Definition 3: an internal node cannot wrap a single child without keywords.
                    if kept_children.len() == 1 && !kept_children[0].kind.is_keyword() {
                        let mut only = kept_children.pop().unwrap();
                        if only.kind.is_internal() {
                            only.dropped_children |= dropped;
                        }
                        return Some(only);
                    }
                    let kind_refs: Vec<&NodeKind> =
                        kept_children.iter().map(|c| &c.kind).collect();
                    let label = SimplifiedTree::make_label(&kind_refs).into_boxed_str();
                    let start = kept_children
                        .iter()
                        .map(|c| c.span.start)
                        .min()
                        .unwrap_or(node.span.start);
                    let end = kept_children
                        .iter()
                        .map(|c| c.span.end)
                        .max()
                        .unwrap_or(node.span.end);
                    Some(DraftNode {
                        kind: NodeKind::Internal { label },
                        children: kept_children,
                        span: Span::new(start, end),
                        dropped_children: dropped,
                    })
                }
            }
        }

        let Some(root) = build_draft(self, &marked, 0) else {
            return SimplifiedTree::empty(self.lang, Arc::clone(&self.source));
        };

        let mut nodes = Vec::new();
        let mut leaves = Vec::new();

        fn flatten(
            draft: DraftNode,
            parent: Option<NodeId>,
            index_in_parent: u16,
            nodes: &mut Vec<Node>,
            leaves: &mut Vec<NodeId>,
        ) -> NodeId {
            let my_id = nodes.len() as NodeId;
            let is_token = draft.kind.is_token();
            nodes.push(Node {
                kind: draft.kind,
                parent,
                index_in_parent,
                children: Vec::with_capacity(draft.children.len()),
                span: draft.span,
                dropped_children: draft.dropped_children,
            });
            if is_token {
                leaves.push(my_id);
            }
            let mut slot: u16 = 0;
            let mut child_ids = Vec::with_capacity(draft.children.len());
            for child in draft.children {
                let child_idx = if child.kind.is_keyword() {
                    0
                } else {
                    slot += 1;
                    slot
                };
                let cid = flatten(child, Some(my_id), child_idx, nodes, leaves);
                child_ids.push(cid);
            }
            nodes[my_id as usize].children = child_ids;
            my_id
        }

        flatten(root, None, 0, &mut nodes, &mut leaves);

        SimplifiedTree {
            lang: self.lang,
            source: Arc::clone(&self.source),
            nodes,
            leaves,
        }
    }

    /// Validates all structural invariants of a `SimplifiedTree`:
    /// - preorder root at 0
    /// - valid parent/child links and 1-based `#`-slot `index_in_parent`
    /// - no single-child internal nodes wrapping a single non-keyword child (Def. 3)
    /// - internal node labels match `make_label` (Def. 4)
    /// - `leaves` lists every `Token` node in preorder and non-decreasing span start order
    /// - all spans within `0..=source.len()` and `start <= end`
    pub fn validate(&self) -> anyhow::Result<()> {
        let src_len = self.source.len() as u32;
        if self.nodes.is_empty() {
            ensure!(self.leaves.is_empty(), "empty tree cannot have leaves");
            return Ok(());
        }

        ensure!(
            self.nodes[0].parent.is_none(),
            "root node 0 must have parent == None"
        );
        ensure!(
            self.nodes[0].index_in_parent == 0,
            "root node 0 must have index_in_parent == 0"
        );

        let mut expected_leaves = Vec::new();
        for (idx, node) in self.nodes.iter().enumerate() {
            let id = idx as NodeId;
            ensure!(
                node.span.start <= node.span.end && node.span.end <= src_len,
                "node {id} has invalid span {:?} for source len {src_len}",
                node.span
            );
            if idx > 0 {
                let p = node
                    .parent
                    .ok_or_else(|| anyhow::anyhow!("non-root node {id} missing parent"))?;
                ensure!(
                    (p as usize) < idx,
                    "node {id} parent {p} violates preorder invariant"
                );
            }

            match &node.kind {
                NodeKind::Internal { label } => {
                    ensure!(!node.children.is_empty(), "internal node {id} has no children");
                    if node.children.len() == 1 {
                        let only = &self.nodes[node.children[0] as usize];
                        if !only.kind.is_keyword() {
                            bail!(
                                "internal node {id} violates Def. 3: wraps a single non-keyword child"
                            );
                        }
                    }
                    let mut child_kinds = Vec::with_capacity(node.children.len());
                    let mut slot: u16 = 0;
                    for &cid in &node.children {
                        ensure!(
                            (cid as usize) < self.nodes.len(),
                            "node {id} references out-of-bounds child {cid}"
                        );
                        ensure!(
                            cid > id,
                            "child {cid} of node {id} violates preorder invariant"
                        );
                        let child = &self.nodes[cid as usize];
                        ensure!(
                            child.parent == Some(id),
                            "child {cid} parent {:?} does not match {id}",
                            child.parent
                        );
                        if child.kind.is_keyword() {
                            ensure!(
                                child.index_in_parent == 0,
                                "keyword child {cid} must have index_in_parent == 0"
                            );
                        } else {
                            slot += 1;
                            ensure!(
                                child.index_in_parent == slot,
                                "child {cid} has index_in_parent {}, expected {slot}",
                                child.index_in_parent
                            );
                        }
                        child_kinds.push(&child.kind);
                    }
                    let expected = Self::make_label(&child_kinds);
                    ensure!(
                        label.as_ref() == expected,
                        "internal node {id} label {label:?} != make_label {expected:?}"
                    );
                }
                NodeKind::Keyword { .. } => {
                    ensure!(node.children.is_empty(), "keyword node {id} has children");
                }
                NodeKind::Token { .. } => {
                    ensure!(node.children.is_empty(), "token node {id} has children");
                    expected_leaves.push(id);
                }
            }
        }

        ensure!(
            self.leaves == expected_leaves,
            "leaves {:?} do not match preorder tokens {:?}",
            self.leaves,
            expected_leaves
        );

        for pair in self.leaves.windows(2) {
            let a = self.nodes[pair[0] as usize].span;
            let b = self.nodes[pair[1] as usize].span;
            ensure!(
                a.start <= b.start,
                "leaves {} ({:?}) and {} ({:?}) out of source order",
                pair[0],
                a,
                pair[1],
                b
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tree() -> SimplifiedTree {
        // Represents: `x = 1; y = 2;`
        let source: Arc<str> = Arc::from("x = 1; y = 2;");
        let nodes = vec![
            // 0: "##"
            Node {
                kind: NodeKind::Internal { label: "##".into() },
                parent: None,
                index_in_parent: 0,
                children: vec![1, 7],
                span: Span::new(0, 13),
                dropped_children: false,
            },
            // 1: "#;"
            Node {
                kind: NodeKind::Internal { label: "#;".into() },
                parent: Some(0),
                index_in_parent: 1,
                children: vec![2, 6],
                span: Span::new(0, 6),
                dropped_children: false,
            },
            // 2: "#=#"
            Node {
                kind: NodeKind::Internal {
                    label: "#=#".into(),
                },
                parent: Some(1),
                index_in_parent: 1,
                children: vec![3, 4, 5],
                span: Span::new(0, 5),
                dropped_children: false,
            },
            // 3: x
            Node {
                kind: NodeKind::Token {
                    text: "x".into(),
                    var: Some(0),
                },
                parent: Some(2),
                index_in_parent: 1,
                children: vec![],
                span: Span::new(0, 1),
                dropped_children: false,
            },
            // 4: =
            Node {
                kind: NodeKind::Keyword { text: "=".into() },
                parent: Some(2),
                index_in_parent: 0,
                children: vec![],
                span: Span::new(2, 3),
                dropped_children: false,
            },
            // 5: 1
            Node {
                kind: NodeKind::Token {
                    text: "1".into(),
                    var: None,
                },
                parent: Some(2),
                index_in_parent: 2,
                children: vec![],
                span: Span::new(4, 5),
                dropped_children: false,
            },
            // 6: ;
            Node {
                kind: NodeKind::Keyword { text: ";".into() },
                parent: Some(1),
                index_in_parent: 0,
                children: vec![],
                span: Span::new(5, 6),
                dropped_children: false,
            },
            // 7: "#;"
            Node {
                kind: NodeKind::Internal { label: "#;".into() },
                parent: Some(0),
                index_in_parent: 2,
                children: vec![8, 12],
                span: Span::new(7, 13),
                dropped_children: false,
            },
            // 8: "#=#"
            Node {
                kind: NodeKind::Internal {
                    label: "#=#".into(),
                },
                parent: Some(7),
                index_in_parent: 1,
                children: vec![9, 10, 11],
                span: Span::new(7, 12),
                dropped_children: false,
            },
            // 9: y
            Node {
                kind: NodeKind::Token {
                    text: "y".into(),
                    var: Some(1),
                },
                parent: Some(8),
                index_in_parent: 1,
                children: vec![],
                span: Span::new(7, 8),
                dropped_children: false,
            },
            // 10: =
            Node {
                kind: NodeKind::Keyword { text: "=".into() },
                parent: Some(8),
                index_in_parent: 0,
                children: vec![],
                span: Span::new(9, 10),
                dropped_children: false,
            },
            // 11: 2
            Node {
                kind: NodeKind::Token {
                    text: "2".into(),
                    var: None,
                },
                parent: Some(8),
                index_in_parent: 2,
                children: vec![],
                span: Span::new(11, 12),
                dropped_children: false,
            },
            // 12: ;
            Node {
                kind: NodeKind::Keyword { text: ";".into() },
                parent: Some(7),
                index_in_parent: 0,
                children: vec![],
                span: Span::new(12, 13),
                dropped_children: false,
            },
        ];

        SimplifiedTree {
            lang: Lang::Java,
            source,
            nodes,
            leaves: vec![3, 5, 9, 11],
        }
    }

    #[test]
    fn validate_and_ancestors_work() {
        let t = sample_tree();
        t.validate().unwrap();
        let ancs: Vec<(NodeId, u16)> = t.ancestors(11).collect();
        assert_eq!(ancs, vec![(8, 2), (7, 1), (0, 2)]);
    }

    #[test]
    fn retain_prunes_and_collapses_root() {
        let t = sample_tree();
        let pruned = t.retain(&[false, false, true, true]);
        pruned.validate().unwrap();
        assert_eq!(pruned.label(0), "#;");
        assert!(pruned.nodes[0].dropped_children);
        assert_eq!(pruned.leaves.len(), 2);
        assert_eq!(pruned.label(pruned.leaves[0]), "y");
        assert_eq!(pruned.label(pruned.leaves[1]), "2");
        assert!(Arc::ptr_eq(&t.source, &pruned.source));
    }

    #[test]
    fn retain_empty_returns_empty_tree() {
        let t = sample_tree();
        let pruned = t.retain(&[false, false, false, false]);
        assert!(pruned.is_empty());
        pruned.validate().unwrap();
    }
}
