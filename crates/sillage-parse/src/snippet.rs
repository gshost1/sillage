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

    /// Like [`Wrapped::around`], with `closers` (see [`closers`]) appended after the
    /// snippet as phantom tokens.
    fn closed(prefix: &str, s: &str, closers: &str, suffix: &str) -> Self {
        let mut w = Self::around(prefix, s, "");
        w.text.push('\n');
        let start = w.text.len() as u32;
        w.text.push_str(closers);
        let end = w.text.len() as u32;
        w.text.push_str(suffix);
        w.map = w.map.with_phantom(start, end);
        w
    }

    fn python_block_closed(s: &str, closers: &str) -> Self {
        let mut w = Self::python_block(s);
        let start = w.text.len() as u32;
        w.text.push_str("    ");
        w.text.push_str(closers);
        let end = w.text.len() as u32;
        w.text.push('\n');
        w.map = w.map.with_phantom(start, end);
        w
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
    let pairs: &[(&str, &str)] = match lang {
        Lang::Java => &[
            ("class __S { void __m() {\n", "\n} }"),
            ("class __S {\n", "\n}"),
            ("", ""),
            ("class __S { Object __m() { return (\n", "\n); } }"),
        ],
        Lang::Python => &[("", "")],
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => &[
            ("", ""),
            ("function __m() {\n", "\n}"),
            ("class __S {\n", "\n}"),
            ("(\n", "\n);"),
        ],
        Lang::Go => &[
            ("package p\nfunc __m() {\n", "\n}\n"),
            ("", ""),
            ("package p\nvar __x = (\n", "\n)\n"),
        ],
        Lang::Rust => &[("fn __m() {\n", "\n}\n"), ("", ""), ("impl __S {\n", "\n}\n")],
    };
    // A partial query often stops inside a block or call (`if (x) {` …, `foo(a,`):
    // try each wrapper first with the missing closers appended (kept as zero-width
    // keywords), so the snippet's own constructs are closed by itself instead of by
    // the wrapper's braces.
    let cl = closers(lang, s);
    let mut candidates = Vec::with_capacity(pairs.len() * 2 + 2);
    for &(prefix, suffix) in pairs {
        if !cl.is_empty() {
            candidates.push(Wrapped::closed(prefix, s, &cl, suffix));
        }
        candidates.push(Wrapped::around(prefix, s, suffix));
    }
    if lang == Lang::Python {
        if !cl.is_empty() {
            candidates.push(Wrapped::python_block_closed(s, &cl));
        }
        candidates.push(Wrapped::python_block(s));
    }
    parse_best(lang, s, candidates, true)
}

/// Closers for the brackets `s` leaves open, innermost first (e.g. `)}` for
/// `if (x) {\n  foo(a,`). Strings, char literals and comments are skipped.
pub(crate) fn closers(lang: Lang, s: &str) -> String {
    let b = s.as_bytes();
    let hash_comments = lang == Lang::Python;
    let mut stack: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'/' if !hash_comments && b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if !hash_comments && b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 1;
            }
            b'#' if hash_comments => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' | b'`' => {
                let q = c;
                let triple = hash_comments && b.get(i + 1) == Some(&q) && b.get(i + 2) == Some(&q);
                i += if triple { 3 } else { 1 };
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == q && (!triple || (b.get(i + 1) == Some(&q) && b.get(i + 2) == Some(&q))) {
                        i += if triple { 2 } else { 0 };
                        break;
                    }
                    if b[i] == b'\n' && q == b'"' && !triple && !matches!(lang, Lang::Rust | Lang::Go) {
                        break; // unterminated single-line string
                    }
                    i += 1;
                }
            }
            b'\'' => {
                // A char literal / Python or JS string — unless it is a Rust lifetime.
                let close = b[i + 1..].iter().take(if matches!(lang, Lang::Python | Lang::JavaScript | Lang::TypeScript | Lang::Tsx) { usize::MAX } else { 4 }).position(|&x| x == b'\'' || x == b'\n');
                match close {
                    Some(k) if b[i + 1 + k] == b'\'' => i += 1 + k,
                    _ => {}
                }
            }
            b'(' => stack.push(b')'),
            b'[' => stack.push(b']'),
            b'{' => stack.push(b'}'),
            b')' | b']' | b'}' => {
                if stack.last() == Some(&c) {
                    stack.pop();
                }
            }
            _ => {}
        }
        i += 1;
    }
    stack.iter().rev().map(|&c| c as char).collect()
}
