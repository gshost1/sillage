//! Error-tolerant parsing of methods and query snippets.
//!
//! Text is parsed under a list of per-language wrappers (e.g. a dummy class and
//! method for Java statements); the first error-free candidate wins, otherwise the
//! one with the fewest `ERROR`/`MISSING` nodes. Wrapper tokens are discarded during
//! lowering and all spans are remapped into the caller's text.

use std::sync::Arc;

use anyhow::Result;
use sillage_core::{Lang, Node as SNode, NodeId, SimplifiedTree, Span};
use tree_sitter::{Node, Tree};

use crate::lang::tables;
use crate::lower::{Lowerer, Mapping, build};
use crate::{parse_ts, scope};

pub(crate) struct Wrapped {
    text: String,
    map: Mapping,
}

impl Wrapped {
    fn raw(s: &str) -> Self {
        Self::around("", s, "")
    }

    fn around(prefix: &str, s: &str, suffix: &str) -> Self {
        let mut text = String::with_capacity(prefix.len() + s.len() + suffix.len());
        text.push_str(prefix);
        text.push_str(s);
        text.push_str(suffix);
        Self {
            text,
            map: Mapping::new(vec![(prefix.len() as u32, 0, s.len() as u32)]),
        }
    }

    /// `def __m():` with the snippet dedented and re-indented one level.
    fn python_block(s: &str) -> Self {
        let lead = |l: &str| l.len() - l.trim_start_matches([' ', '\t']).len();
        let common = s
            .split_inclusive('\n')
            .filter(|l| !l.trim().is_empty())
            .map(lead)
            .min()
            .unwrap_or(0);
        let mut text = String::from("def __m():\n");
        let mut segs = Vec::new();
        let mut orig = 0usize;
        for line in s.split_inclusive('\n') {
            let strip = lead(line).min(common);
            let body = &line[strip..];
            if !body.trim().is_empty() {
                text.push_str("    ");
            }
            segs.push((text.len() as u32, (orig + strip) as u32, body.len() as u32));
            text.push_str(body);
            orig += line.len();
        }
        if !text.ends_with('\n') {
            text.push('\n');
        }
        Self {
            text,
            map: Mapping::new(segs),
        }
    }
}

/// `(ERROR + MISSING node count, ERROR bytes)`; `(0, 0)` for a clean parse.
fn error_score(root: Node<'_>) -> (usize, usize) {
    if !root.has_error() {
        return (0, 0);
    }
    let (mut count, mut bytes) = (0, 0);
    let mut cur = root.walk();
    loop {
        let n = cur.node();
        if n.is_error() {
            count += 1;
            bytes += n.end_byte() - n.start_byte();
        } else if n.is_missing() {
            count += 1;
        }
        if n.has_error() && cur.goto_first_child() {
            continue;
        }
        loop {
            if cur.goto_next_sibling() {
                break;
            }
            if !cur.goto_parent() {
                return (count, bytes);
            }
        }
    }
}

fn parse_best(
    lang: Lang,
    original: &str,
    candidates: Vec<Wrapped>,
    snippet: bool,
) -> Result<SimplifiedTree> {
    let mut best: Option<((usize, usize), Wrapped, Tree)> = None;
    for cand in candidates {
        let tree = parse_ts(lang, &cand.text)?;
        let score = error_score(tree.root_node());
        let better = best.as_ref().is_none_or(|(s, _, _)| score < *s);
        if better {
            best = Some((score, cand, tree));
        }
        if score == (0, 0) {
            break;
        }
    }
    let Some((_, wrapped, tree)) = best else {
        return Ok(SimplifiedTree::empty(lang, Arc::from(original)));
    };
    let t = tables(lang);
    let root = tree.root_node();
    let vars = scope::resolve(root, &wrapped.text, t, snippet);
    let lowerer = Lowerer {
        orig: original,
        map: &wrapped.map,
        t,
        vars: &vars,
    };
    let draft = lowerer.lower(root, 0);
    Ok(build(lang, Arc::from(original), draft))
}

/// Full method text (as extracted): undeclared names stay globals.
///
/// The whole method is parsed (so parameters resolve as locals), but the returned
/// tree is the subtree of the method **body** (`body`, relative to `text`), as in
/// the paper, where the corpus and recommendations are method bodies: signature
/// tokens (modifiers, name, parameter list) never enter features or recommendations.
/// Spans still index `text`. An empty or out-of-range `body` keeps the whole tree.
pub(crate) fn parse_method(lang: Lang, text: &str, body: Span) -> Result<SimplifiedTree> {
    let full = parse_method_full(lang, text)?;
    Ok(body_subtree(full, body))
}

/// Root of the body: the lowest common ancestor of every terminal inside `body`.
fn body_root(tree: &SimplifiedTree, body: Span) -> Option<NodeId> {
    let inside = |sp: Span| sp.start >= body.start && sp.end <= body.end && !sp.is_empty();
    let mut lca: Option<Vec<NodeId>> = None;
    for (id, node) in tree.nodes.iter().enumerate() {
        if !node.children.is_empty() || !inside(node.span) {
            continue;
        }
        let mut path: Vec<NodeId> = vec![id as NodeId];
        path.extend(tree.ancestors(id as NodeId).map(|(a, _)| a));
        path.reverse();
        lca = Some(match lca {
            None => path,
            Some(prev) => prev.iter().zip(&path).take_while(|(a, b)| a == b).map(|(a, _)| *a).collect(),
        });
    }
    lca.and_then(|p| p.last().copied())
}

fn body_subtree(tree: SimplifiedTree, body: Span) -> SimplifiedTree {
    if tree.is_empty() || body.is_empty() || body.end as usize > tree.source.len() {
        return tree;
    }
    let Some(root) = body_root(&tree, body) else {
        return SimplifiedTree::empty(tree.lang, Arc::clone(&tree.source));
    };
    if root == 0 {
        return tree;
    }
    let mut out = SimplifiedTree::empty(tree.lang, Arc::clone(&tree.source));
    fn copy(src: &SimplifiedTree, id: NodeId, parent: Option<NodeId>, out: &mut SimplifiedTree) -> NodeId {
        let n = &src.nodes[id as usize];
        let new_id = out.nodes.len() as NodeId;
        if n.kind.is_token() {
            out.leaves.push(new_id);
        }
        out.nodes.push(SNode {
            kind: n.kind.clone(),
            parent,
            index_in_parent: if parent.is_some() { n.index_in_parent } else { 0 },
            children: Vec::new(),
            span: n.span,
            dropped_children: n.dropped_children,
        });
        let kids: Vec<NodeId> = n.children.iter().map(|&c| copy(src, c, Some(new_id), out)).collect();
        out.nodes[new_id as usize].children = kids;
        new_id
    }
    copy(&tree, root, None, &mut out);
    out
}

fn parse_method_full(lang: Lang, text: &str) -> Result<SimplifiedTree> {
    let candidates = match lang {
        Lang::Java => vec![Wrapped::around("class __S {\n", text, "\n}"), Wrapped::raw(text)],
        Lang::Python => vec![Wrapped::raw(text)],
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => vec![
            Wrapped::around("class __S {\n", text, "\n}"),
            Wrapped::raw(text),
            Wrapped::around("(\n", text, "\n)"),
        ],
        Lang::Go => vec![Wrapped::raw(text), Wrapped::around("package p\n", text, "\n")],
        Lang::Rust => vec![Wrapped::raw(text), Wrapped::around("impl __S {\n", text, "\n}")],
    };
    parse_best(lang, text, candidates, false)
}

/// Query snippet (statements, expression, member or partial fragment):
/// free value identifiers count as locals (Decision 8).
pub(crate) fn parse_snippet(lang: Lang, s: &str) -> Result<SimplifiedTree> {
    if s.trim().is_empty() {
        return Ok(SimplifiedTree::empty(lang, Arc::from(s)));
    }
    let candidates = match lang {
        Lang::Java => vec![
            Wrapped::around("class __S { void __m() {\n", s, "\n} }"),
            Wrapped::around("class __S {\n", s, "\n}"),
            Wrapped::raw(s),
            Wrapped::around("class __S { Object __m() { return (\n", s, "\n); } }"),
        ],
        Lang::Python => vec![Wrapped::raw(s), Wrapped::python_block(s)],
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => vec![
            Wrapped::raw(s),
            Wrapped::around("function __m() {\n", s, "\n}"),
            Wrapped::around("class __S {\n", s, "\n}"),
            Wrapped::around("(\n", s, "\n);"),
        ],
        Lang::Go => vec![
            Wrapped::around("package p\nfunc __m() {\n", s, "\n}\n"),
            Wrapped::raw(s),
            Wrapped::around("package p\nvar __x = (\n", s, "\n)\n"),
        ],
        Lang::Rust => vec![
            Wrapped::around("fn __m() {\n", s, "\n}\n"),
            Wrapped::raw(s),
            Wrapped::around("impl __S {\n", s, "\n}\n"),
        ],
    };
    parse_best(lang, s, candidates, true)
}
