#![allow(dead_code)]

use sillage_core::{NodeKind, SimplifiedTree};

/// Indented dump: internal labels, `kw:` keywords, tokens with `$id` for variables.
pub fn dump(t: &SimplifiedTree) -> String {
    let mut out = String::new();
    if !t.nodes.is_empty() {
        go(t, 0, 0, &mut out);
    }
    out
}

fn go(t: &SimplifiedTree, id: u32, depth: usize, out: &mut String) {
    let n = t.node(id);
    let pad = "  ".repeat(depth);
    match &n.kind {
        NodeKind::Internal { label } => out.push_str(&format!("{pad}[{label}]\n")),
        NodeKind::Keyword { text } => out.push_str(&format!("{pad}kw:{text}\n")),
        NodeKind::Token { text, var } => match var {
            Some(v) => out.push_str(&format!("{pad}{text} ${v}\n")),
            None => out.push_str(&format!("{pad}{text}\n")),
        },
    }
    for &c in &n.children {
        go(t, c, depth + 1, out);
    }
}

/// `(text, var)` for every leaf token, in source order.
pub fn tokens(t: &SimplifiedTree) -> Vec<(String, Option<u32>)> {
    t.leaves
        .iter()
        .map(|&l| match &t.node(l).kind {
            NodeKind::Token { text, var } => (text.to_string(), *var),
            _ => unreachable!(),
        })
        .collect()
}

pub fn labels(t: &SimplifiedTree) -> Vec<String> {
    t.nodes
        .iter()
        .filter_map(|n| match &n.kind {
            NodeKind::Internal { label } => Some(label.to_string()),
            _ => None,
        })
        .collect()
}

/// Every span's text matches the node's token/keyword text.
pub fn check_spans(t: &SimplifiedTree) {
    for n in &t.nodes {
        let s = &t.source[n.span.start as usize..n.span.end as usize];
        match &n.kind {
            // Zero-width keywords are synthesized closers / MISSING tokens.
            NodeKind::Keyword { text } if n.span.is_empty() => {
                assert!(matches!(text.as_ref(), ")" | "]" | "}") || !text.trim().is_empty(), "{text:?}")
            }
            NodeKind::Keyword { text } | NodeKind::Token { text, .. } => {
                assert_eq!(s, text.as_ref(), "span text mismatch")
            }
            NodeKind::Internal { .. } => {}
        }
    }
}
