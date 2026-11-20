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
use crate::lower::{HOLE, Lowerer, Mapping, build};
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

    /// `prefix + s + suffix` with a [`HOLE`] inserted at each offset of `holes`
    /// (sorted) and, when non-empty, `closers` appended as phantom tokens.
    fn with_holes(prefix: &str, s: &str, holes: &[usize], closers: &str, suffix: &str) -> Self {
        let mut text = String::with_capacity(prefix.len() + s.len() + holes.len() * HOLE.len() + closers.len() + suffix.len() + 1);
        text.push_str(prefix);
        let (mut segs, mut ranges) = (Vec::new(), Vec::new());
        let mut last = 0usize;
        for &h in holes {
            segs.push((text.len() as u32, last as u32, (h - last) as u32));
            text.push_str(&s[last..h]);
            let start = text.len() as u32;
            text.push_str(HOLE);
            ranges.push((start, text.len() as u32));
            last = h;
        }
        segs.push((text.len() as u32, last as u32, (s.len() - last) as u32));
        text.push_str(&s[last..]);
        let mut map = Mapping::new(segs).with_holes(ranges);
        if !closers.is_empty() {
            text.push('\n');
            let start = text.len() as u32;
            text.push_str(closers);
            map = map.with_phantom(start, text.len() as u32);
        }
        text.push_str(suffix);
        Self { text, map }
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
            // `this(…)` / `super(…)` only parse in a constructor body.
            ("class __S { __S() {\n", "\n} }"),
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
    //
    // Missing operands get holes first (see `holes`): `x =` reads `#=#` and a
    // leading-dot `.build();` reads `#.#()`, as in the complete code.
    //
    // Python: an odd number of triple quotes means one end of a multi-line string
    // (`"""` alone, or `conftest="""`) was sampled without the other; left in,
    // it opens a string that swallows the rest of the query. A variant with the
    // odd lines blanked (same length, so spans are unchanged) is tried first.
    let mut variants: Vec<std::borrow::Cow<'_, str>> = Vec::with_capacity(2);
    if lang == Lang::Python
        && let Some(blanked) = blank_open_triple_quotes(s)
    {
        variants.push(blanked.into());
    }
    variants.push(s.into());
    let mut candidates = Vec::with_capacity(variants.len() * (pairs.len() * 3 + 2));
    for v in &variants {
        let v: &str = v;
        let cl = closers(lang, v);
        let holes = holes(lang, v);
        for &(prefix, suffix) in pairs {
            if !holes.is_empty() {
                candidates.push(Wrapped::with_holes(prefix, v, &holes, &cl, suffix));
            }
            if !cl.is_empty() {
                candidates.push(Wrapped::closed(prefix, v, &cl, suffix));
            }
            candidates.push(Wrapped::around(prefix, v, suffix));
        }
        if lang == Lang::Python {
            if !cl.is_empty() {
                candidates.push(Wrapped::python_block_closed(v, &cl));
            }
            candidates.push(Wrapped::python_block(v));
        }
    }
    parse_best(lang, s, candidates, true)
}

/// When `s` holds an odd number of `"""`/`'''` (a multi-line string is left
/// open), `s` with every line holding an odd number of them replaced by spaces.
fn blank_open_triple_quotes(s: &str) -> Option<String> {
    let count = |l: &str| l.matches("\"\"\"").count() + l.matches("'''").count();
    let odd = |l: &str| count(l) % 2 == 1;
    if count(s) % 2 == 0 {
        return None;
    }
    let out: Vec<String> = s
        .split('\n')
        .map(|l| if odd(l) { " ".repeat(l.len()) } else { l.to_string() })
        .collect();
    Some(out.join("\n"))
}

/// Closers for the brackets `s` leaves open, innermost first (e.g. `)}` for
/// `if (x) {\n  foo(a,`). Strings, char literals and comments are skipped.
pub(crate) fn closers(lang: Lang, s: &str) -> String {
    scan(lang, s).0.iter().rev().map(|&c| c as char).collect()
}

/// Operators that leave a snippet's last expression without its right operand,
/// longest first. `++`/`--` (complete postfix) are checked before `+`/`-`.
///
/// A trailing `,` is deliberately absent: `foo(a, b,` cut from a longer call
/// would read `#(#,#,#)` with a hole (or `#(#,#)` with the comma stripped) and
/// then match *other* methods' 3-argument (2-argument) calls exactly, while the
/// original's call has an unknown arity. Left as is, the label `#(#,#,)` matches
/// nothing. Measured on the Java benchmark, holes after `,` cost 0.2–0.4 points
/// of contiguous Recall@1.
const DANGLING: &[&str] = &[
    ">>>=", ">>=", "<<=", "===", "!==", "**=", "&&", "||", "??", "?.", "==", "!=", "<=", ">=", "+=", "-=",
    "*=", "/=", "%=", "&=", "|=", "^=", "->", "=>", "::", ":=", "<<", ".", "=", "+", "-", "*", "/", "%",
    "&", "|", "^", "?", ":", "!", "~",
];

/// Offsets where a [`HOLE`] stands in for a missing operand of `s`:
/// - after a trailing dangling operator (`x =`, `a &&`, `obj.`), ignoring
///   trailing comments and whitespace (not after `,`: see [`DANGLING`]);
/// - before a leading-dot member access (`.build();`) that starts the snippet or
///   follows a line ending a statement or opening a list (`;`, `{`, `}`, `(`, `,`),
///   i.e. a fluent-chain line whose receiver was not sampled.
pub(crate) fn holes(lang: Lang, s: &str) -> Vec<usize> {
    let (_, last_code, line_ends) = scan(lang, s);
    let b = s.as_bytes();
    let mut out = Vec::new();
    if matches!(lang, Lang::Java | Lang::JavaScript | Lang::TypeScript | Lang::Tsx | Lang::Rust) {
        let mut prev_end: Option<u8> = None;
        let mut line_start = 0usize;
        for line in s.split_inclusive('\n') {
            let lead = line.len() - line.trim_start().len();
            let at = line_start + lead;
            let next = b.get(at + 1).copied().unwrap_or(b' ');
            if b.get(at) == Some(&b'.')
                && (next.is_ascii_alphabetic() || next == b'_' || next == b'$')
                && prev_end.is_none_or(|c| matches!(c, b';' | b'{' | b'}' | b'(' | b','))
            {
                out.push(at);
            }
            let end = line_start + line.len();
            if let Some(&e) = line_ends.iter().rfind(|&&e| e > line_start && e <= end) {
                prev_end = Some(b[e - 1]);
            }
            line_start = end;
        }
    }
    let t = &s[..last_code];
    let dangling = !t.ends_with("++")
        && !t.ends_with("--")
        && DANGLING.iter().find(|op| t.ends_with(**op)).is_some_and(|&op| match op {
            "?" => lang != Lang::Rust,  // Rust's `?` is the (complete) try operator
            ":" => lang != Lang::Python, // a Python block header
            "!" => lang != Lang::Rust,  // `vec!` / `println!` macro bang
            "*" | "/" => !t.ends_with("*/"),
            _ => true,
        });
    if dangling && out.last().is_none_or(|&h| h < last_code) {
        out.push(last_code);
    }
    out
}

/// `(open-bracket stack, end of the last code byte, end of every code byte)` of `s`,
/// skipping strings, char literals and comments.
fn scan(lang: Lang, s: &str) -> (Vec<u8>, usize, Vec<usize>) {
    let b = s.as_bytes();
    let mut last_code = 0usize;
    let mut code_ends: Vec<usize> = Vec::new();
    let hash_comments = lang == Lang::Python;
    let mut stack: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let mut code = !c.is_ascii_whitespace();
        match c {
            b'/' if !hash_comments && b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if !hash_comments && b.get(i + 1) == Some(&b'*') => {
                code = false;
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
                continue;
            }
            b'\'' if !matches!(lang, Lang::Python | Lang::JavaScript | Lang::TypeScript | Lang::Tsx) => {
                // A char literal (`'a'`, `'\''`, `'\u{1F600}'`) — unless it is a
                // Rust lifetime / label (`'a`), which has no closing quote.
                if let Some(end) = char_literal_end(b, i) {
                    i = end;
                }
            }
            b'"' | b'`' | b'\'' => {
                let q = c;
                // Python `"""` / `'''` strings and Java `"""` text blocks.
                let triple = (hash_comments || (lang == Lang::Java && q == b'"'))
                    && b.get(i + 1) == Some(&q)
                    && b.get(i + 2) == Some(&q);
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
                    let multiline = triple || q == b'`' || (q == b'"' && matches!(lang, Lang::Rust | Lang::Go));
                    if b[i] == b'\n' && !multiline {
                        break; // unterminated single-line string
                    }
                    i += 1;
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
        if code {
            last_code = (i + 1).min(b.len());
            code_ends.push(last_code);
        }
        i += 1;
    }
    (stack, last_code, code_ends)
}

/// Index of the closing quote of a char literal opening at `b[i] == '\''`, or
/// `None` when the quote opens a Rust lifetime / label instead. Escapes
/// (`'\''`, `'\\'`, `'\u{10FFFF}'`) are skipped; an unescaped literal holds
/// exactly one (UTF-8) character.
fn char_literal_end(b: &[u8], i: usize) -> Option<usize> {
    match b.get(i + 1)? {
        b'\\' => {
            let mut j = i + 3; // past the escaped byte
            while j < b.len() && j <= i + 12 {
                match b[j] {
                    b'\'' => return Some(j),
                    b'\n' => return None,
                    _ => j += 1,
                }
            }
            None
        }
        b'\'' | b'\n' => None,
        &first => {
            let len = match first {
                0x00..=0x7f => 1,
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                _ => 4,
            };
            (b.get(i + 1 + len) == Some(&b'\'')).then_some(i + 1 + len)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stage 5: `\\` escapes inside single-quoted strings and char literals do
    /// not end the literal, so brackets after them are still balanced.
    #[test]
    fn scan_handles_single_quote_escapes() {
        // JS / Python string with an escaped quote: the `(` inside is not code.
        let (stack, _, _) = scan(Lang::JavaScript, "f('it\\'s (', x)");
        assert!(stack.is_empty(), "{stack:?}");
        let (stack, _, _) = scan(Lang::Python, "f('it\\'s [', x)");
        assert!(stack.is_empty(), "{stack:?}");
        // Java / Rust char literals with escapes.
        let (stack, _, _) = scan(Lang::Java, "if (c == '\\'' || c == '(') {");
        assert_eq!(stack, b"}");
        let (stack, _, _) = scan(Lang::Rust, "let v = ['\\\\', '\\u{1F600}', '('];");
        assert!(stack.is_empty(), "{stack:?}");
        // A Rust lifetime is not a literal: the `(` after it is code.
        let (stack, _, _) = scan(Lang::Rust, "fn f<'a>(x: &'a str");
        assert_eq!(stack, b")");
        assert_eq!(char_literal_end(b"'\\u{10FFFF}'", 0), Some(11));
        assert_eq!(char_literal_end("'é'".as_bytes(), 0), Some(3));
        assert_eq!(char_literal_end(b"'a>", 0), None);
    }

    /// Stage 5: a Java `"""` text block is one string literal, even with
    /// brackets and lone quotes on its interior lines.
    #[test]
    fn scan_treats_java_text_blocks_as_one_literal() {
        let src = "String s = \"\"\"\n    { \"(\" [\n    \"\"\";\nuse(s);";
        let (stack, last, _) = scan(Lang::Java, src);
        assert!(stack.is_empty(), "{stack:?}");
        assert_eq!(last, src.len());
        let open = "String s = \"\"\"\n    { (\n";
        let (stack, _, _) = scan(Lang::Java, open);
        assert!(stack.is_empty(), "unterminated text block swallows the rest: {stack:?}");
    }

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
