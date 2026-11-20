//! Method extraction: methods, constructors, functions, impl/trait methods and
//! named function expressions (JS/TS), including nested ones.
//!
//! Each method is re-parsed from its own text with [`parse_method`](crate::parse_method),
//! so `ExtractedMethod::tree` is exactly what `parse_method(lang, text, body_span)`
//! returns.

use sillage_core::{Lang, Span};
use tree_sitter::Node;

use crate::ExtractedMethod;
use crate::lang::spec;
use crate::{parse_ts, snippet};

pub(crate) fn extract(lang: Lang, src: &str) -> Vec<ExtractedMethod> {
    let Ok(tree) = parse_ts(lang, src) else {
        return Vec::new();
    };
    let mut found: Vec<(Node<'_>, Node<'_>, Box<str>)> = Vec::new();
    let mut cur = tree.root_node().walk();
    loop {
        let node = cur.node();
        if let Some((body, name)) = method_parts(lang, node, src) {
            found.push((node, body, name));
        }
        if cur.goto_first_child() {
            continue;
        }
        let mut done = false;
        loop {
            if cur.goto_next_sibling() {
                break;
            }
            if !cur.goto_parent() {
                done = true;
                break;
            }
        }
        if done {
            break;
        }
    }

    let mut out = Vec::with_capacity(found.len());
    for (node, body, name) in found {
        let (start, end) = (node.start_byte(), node.end_byte());
        let Some(text) = src.get(start..end) else {
            continue;
        };
        let body_span = Span::new(
            (body.start_byte() - start) as u32,
            (body.end_byte().min(end) - start) as u32,
        );
        let Ok(tree) = snippet::parse_method(lang, text, body_span) else {
            continue;
        };
        out.push(ExtractedMethod {
            name,
            line_start: node.start_position().row as u32 + 1,
            text_span: Span::new(start as u32, end as u32),
            body_span,
            tree,
        });
    }
    out
}

fn text_of<'s>(n: Node<'_>, src: &'s str) -> &'s str {
    src.get(n.start_byte()..n.end_byte()).unwrap_or("")
}

/// `(body, name)` when `node` is an extractable method with a body.
fn method_parts<'t>(lang: Lang, node: Node<'t>, src: &str) -> Option<(Node<'t>, Box<str>)> {
    if node.is_error() || node.is_missing() {
        return None;
    }
    let s = spec(lang);
    let kind = node.kind();
    if let Some(rule) = s.methods.iter().find(|m| m.kind == kind) {
        let body = node.child_by_field_name(rule.body_field)?;
        let name = node
            .child_by_field_name(rule.name_field)
            .map(|n| text_of(n, src))
            .filter(|n| !n.is_empty())
            .unwrap_or("<anonymous>");
        return Some((body, name.into()));
    }
    if s.anon_fns.contains(&kind) {
        let body = node.child_by_field_name("body")?;
        let parent = node.parent()?;
        let (_, field) = s.named_parents.iter().find(|(k, _)| *k == parent.kind())?;
        let name_node = parent.child_by_field_name(field)?;
        if name_node.id() == node.id() {
            return None;
        }
        let name = text_of(name_node, src);
        if name.is_empty() {
            return None;
        }
        return Some((body, name.into()));
    }
    None
}
