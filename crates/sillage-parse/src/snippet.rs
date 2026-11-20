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
///
/// Nodes are numbered in preorder, so the LCA of a node set is the LCA of its
/// smallest and largest member.
fn body_root(tree: &SimplifiedTree, body: Span) -> Option<NodeId> {
    let inside = |sp: Span| sp.start >= body.start && sp.end <= body.end && !sp.is_empty();
    let mut ends: Option<(NodeId, NodeId)> = None;
    for (id, node) in tree.nodes.iter().enumerate() {
        if node.children.is_empty() && inside(node.span) {
            let id = id as NodeId;
            ends = Some(ends.map_or((id, id), |(lo, _)| (lo, id)));
        }
    }
    let (lo, hi) = ends?;
    let path = |n: NodeId| {
        let mut p: Vec<NodeId> = vec![n];
        p.extend(tree.ancestors(n).map(|(a, _)| a));
        p.reverse();
        p
    };
    let (a, b) = (path(lo), path(hi));
    a.iter().zip(&b).take_while(|(x, y)| x == y).map(|(x, _)| *x).last()
}

/// `true` when every node's children have larger IDs that form, with their own
/// subtrees, consecutive preorder ranges (what [`crate::lower::build`] produces).
fn is_preorder(tree: &SimplifiedTree) -> bool {
    let mut next: NodeId = 0;
    let mut stack: Vec<NodeId> = vec![0];
    while let Some(n) = stack.pop() {
        if n != next {
            return false;
        }
        next += 1;
        stack.extend(tree.nodes[n as usize].children.iter().rev());
    }
    next as usize == tree.nodes.len()
}

fn body_subtree(mut tree: SimplifiedTree, body: Span) -> SimplifiedTree {
    if tree.is_empty() || body.is_empty() || body.end as usize > tree.source.len() {
        return tree;
    }
    let Some(root) = body_root(&tree, body) else {
        return SimplifiedTree::empty(tree.lang, Arc::clone(&tree.source));
    };
    if root == 0 {
        return tree;
    }
    if !is_preorder(&tree) {
        return copy_subtree(&tree, root);
    }
    // The subtree of `root` is the preorder range `root..end`: move it out and
    // shift every ID down by `root`.
    let mut end = root;
    loop {
        match tree.nodes[end as usize].children.last() {
            Some(&c) => end = c,
            None => break,
        }
    }
    let (lo, hi) = (root as usize, end as usize + 1);
    let mut nodes: Vec<SNode> = tree.nodes.drain(lo..hi).collect();
    nodes[0].parent = None;
    nodes[0].index_in_parent = 0;
    for n in &mut nodes {
        n.parent = n.parent.map(|p| p - root);
        for c in &mut n.children {
            *c -= root;
        }
    }
    let leaves = tree.leaves.iter().filter(|&&l| (lo..hi).contains(&(l as usize))).map(|&l| l - root).collect();
    SimplifiedTree { lang: tree.lang, source: tree.source, nodes, leaves }
}

/// Deep copy of the subtree at `root` (fallback for non-preorder trees, and the
/// reference for [`body_subtree`]'s range move).
fn copy_subtree(tree: &SimplifiedTree, root: NodeId) -> SimplifiedTree {
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
    copy(tree, root, None, &mut out);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The preorder range move equals a deep copy of the body subtree, for
    /// methods in every language (incl. nested functions and lambdas).
    #[test]
    fn body_range_move_equals_copy() {
        let cases: &[(Lang, &str)] = &[
            (Lang::Java, "class A {\n  int f(int a) {\n    int b = a + 1;\n    for (int i = 0; i < b; i++) { g(i, () -> { return i * 2; }); }\n    return b;\n  }\n  A() { this(1); }\n}\n"),
            (Lang::Python, "def f(a, b=2):\n    def inner(x):\n        return x + a\n    return [inner(y) for y in range(b)]\n"),
            (Lang::JavaScript, "function f(a) {\n  const g = (x) => x + a;\n  return `v ${g(1)}`;\n}\nclass C { m(y) { return y.z(); } }\n"),
            (Lang::TypeScript, "function f(a: number): number {\n  let b: number = a * 2;\n  return b;\n}\n"),
            (Lang::Go, "package p\nfunc f(a int) int {\n\tb := a + 1\n\tfor i := 0; i < b; i++ {\n\t\tb += i\n\t}\n\treturn b\n}\n"),
            (Lang::Rust, "fn f(a: u32) -> u32 {\n    let b = a + 1;\n    match b { 0 => 1, n => n * 2 }\n}\nimpl S { fn g(&self) -> u8 { self.0 } }\n"),
        ];
        let mut checked = 0;
        for &(lang, src) in cases {
            for m in crate::extract_methods(lang, src) {
                let text = &src[m.text_span.start as usize..m.text_span.end as usize];
                let full = parse_method_full(lang, text).unwrap();
                let root = body_root(&full, m.body_span).unwrap();
                assert!(is_preorder(&full), "{lang}: {}", m.name);
                let moved = body_subtree(full.clone(), m.body_span);
                let copied = if root == 0 { full.clone() } else { copy_subtree(&full, root) };
                assert_eq!(moved, copied, "{lang}: {}", m.name);
                moved.validate().unwrap();
                checked += 1;
            }
        }
        assert!(checked >= 10, "{checked}");
    }

    /// Same check over every method of a real corpus (`SILLAGE_CORPUS`, default
    /// `../../data/corpus`): `cargo test -p sillage-parse --release -- --ignored`.
    #[test]
    #[ignore]
    fn body_range_move_equals_copy_on_corpus() {
        let root = std::env::var("SILLAGE_CORPUS").unwrap_or_else(|_| "../../data/corpus".into());
        let mut stack = vec![std::path::PathBuf::from(root)];
        let (mut files, mut methods) = (0usize, 0usize);
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let Some(lang) = crate::lang_from_path(&p) else { continue };
                let Ok(src) = std::fs::read_to_string(&p) else { continue };
                if src.len() > 2 * 1024 * 1024 {
                    continue;
                }
                files += 1;
                for m in crate::extract_methods(lang, &src) {
                    let text = &src[m.text_span.start as usize..m.text_span.end as usize];
                    let full = parse_method_full(lang, text).unwrap();
                    let copied = match body_root(&full, m.body_span) {
                        _ if full.is_empty() || m.body_span.is_empty() => full.clone(),
                        None => SimplifiedTree::empty(lang, Arc::clone(&full.source)),
                        Some(0) => full.clone(),
                        Some(r) => copy_subtree(&full, r),
                    };
                    assert_eq!(body_subtree(full, m.body_span), copied, "{}: {}", p.display(), m.name);
                    methods += 1;
                }
            }
        }
        eprintln!("{files} files, {methods} methods checked");
    }
}
