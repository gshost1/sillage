//! Hand-built simplified parse trees for tests (no dependency on `sillage-parse`).
#![allow(dead_code)]

use std::sync::Arc;

use sillage_core::{Lang, Node, NodeId, NodeKind, SimplifiedTree, Span, VarId};

pub enum T {
    /// Keyword token.
    K(&'static str),
    /// Non-keyword token; `Some(v)` marks a local variable.
    Tok(&'static str, Option<VarId>),
    /// Simplified parse tree (list).
    N(Vec<T>),
}

pub fn k(s: &'static str) -> T {
    T::K(s)
}
pub fn g(s: &'static str) -> T {
    T::Tok(s, None)
}
pub fn v(s: &'static str, id: VarId) -> T {
    T::Tok(s, Some(id))
}
pub fn n(children: Vec<T>) -> T {
    T::N(children)
}

/// Flattens `root` into a preorder `SimplifiedTree`; source is the tokens joined by spaces.
pub fn build(root: T) -> SimplifiedTree {
    let mut nodes = Vec::new();
    let mut leaves = Vec::new();
    let mut src = String::new();
    flatten(&root, None, 0, &mut nodes, &mut leaves, &mut src);
    let tree = SimplifiedTree { lang: Lang::Java, source: Arc::from(src.as_str()), nodes, leaves };
    tree.validate().expect("hand-built tree is valid");
    tree
}

fn kind_of(t: &T) -> NodeKind {
    match t {
        T::K(s) => NodeKind::Keyword { text: (*s).into() },
        T::Tok(s, var) => NodeKind::Token { text: (*s).into(), var: *var },
        T::N(children) => {
            let kinds: Vec<NodeKind> = children.iter().map(kind_of).collect();
            let refs: Vec<&NodeKind> = kinds.iter().collect();
            NodeKind::Internal { label: SimplifiedTree::make_label(&refs).into() }
        }
    }
}

fn flatten(
    t: &T,
    parent: Option<NodeId>,
    index_in_parent: u16,
    nodes: &mut Vec<Node>,
    leaves: &mut Vec<NodeId>,
    src: &mut String,
) -> NodeId {
    let id = nodes.len() as NodeId;
    let start = src.len() as u32;
    nodes.push(Node {
        kind: kind_of(t),
        parent,
        index_in_parent,
        children: Vec::new(),
        span: Span::default(),
        dropped_children: false,
    });
    match t {
        T::K(s) | T::Tok(s, _) => {
            if matches!(t, T::Tok(..)) {
                leaves.push(id);
            }
            src.push_str(s);
            src.push(' ');
        }
        T::N(children) => {
            let mut slot = 0u16;
            let mut ids = Vec::new();
            for c in children {
                let idx = if matches!(c, T::K(_)) {
                    0
                } else {
                    slot += 1;
                    slot
                };
                ids.push(flatten(c, Some(id), idx, nodes, leaves, src));
            }
            nodes[id as usize].children = ids;
        }
    }
    nodes[id as usize].span = Span::new(start, src.len() as u32);
    id
}

/// The tree of paper Fig. 4 for Listing 4:
///
/// ```java
/// if (view instanceof ViewGroup) {
///   for (int i = 0; i < ((ViewGroup) view).getChildCount(); i++) {
///     View innerView = ((ViewGroup) view).getChildAt(i);
///   }
/// }
/// ```
///
/// Locals: `view` = 0, `i` = 1, `innerView` = 2.
pub fn listing4() -> SimplifiedTree {
    // `((ViewGroup) view)`: parenthesized `(#)` around the cast `(#)#`.
    let cast_view = || n(vec![k("("), n(vec![k("("), g("ViewGroup"), k(")"), v("view", 0)]), k(")")]);
    let cond = n(vec![k("("), n(vec![v("view", 0), k("instanceof"), g("ViewGroup")]), k(")")]);
    let for_control = n(vec![
        n(vec![k("int"), n(vec![v("i", 1), k("="), g("0")])]),
        k(";"),
        n(vec![v("i", 1), k("<"), n(vec![cast_view(), k("."), n(vec![g("getChildCount"), k("("), k(")")])])]),
        k(";"),
        n(vec![v("i", 1), k("++")]),
    ]);
    let decl = n(vec![
        n(vec![
            g("View"),
            n(vec![
                v("innerView", 2),
                k("="),
                n(vec![cast_view(), k("."), n(vec![g("getChildAt"), k("("), v("i", 1), k(")")])]),
            ]),
        ]),
        k(";"),
    ]);
    let for_body = n(vec![k("{"), decl, k("}")]);
    let for_stmt = n(vec![k("for"), k("("), for_control, k(")"), for_body]);
    build(n(vec![k("if"), cond, n(vec![k("{"), for_stmt, k("}")])]))
}

/// Index into `tree.leaves` of the `nth` (0-based) leaf with text `text`.
pub fn leaf_index(tree: &SimplifiedTree, text: &str, nth: usize) -> usize {
    tree.leaves
        .iter()
        .enumerate()
        .filter(|&(_, &id)| tree.label(id) == text)
        .nth(nth)
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("no leaf #{nth} with text {text:?}"))
}
