//! Token-level helpers for renaming identifiers without touching strings,
//! comments or keywords.

use sillage_core::Lang;
use tree_sitter::Node;

use crate::parse_ts;

/// Byte spans of identifiers in `text` that stand for both a field / property
/// name and a variable (`Foo { x }` in Rust, `{ x }` in JS/TS objects and
/// destructuring). Renaming such a variable must expand it to `x: y`.
pub fn shorthand_spans(lang: Lang, text: &str) -> Vec<(u32, u32)> {
    let kinds: &[&str] = match lang {
        Lang::Rust => &["shorthand_field_initializer", "shorthand_field_identifier"],
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => {
            &["shorthand_property_identifier", "shorthand_property_identifier_pattern"]
        }
        _ => return Vec::new(),
    };
    let mut best: Option<Vec<(u32, u32)>> = None;
    // Class methods (`foo() { … }`) parse cleanly only inside a class body.
    let wraps: &[(&str, &str)] = match lang {
        Lang::Rust => &[("", ""), ("impl __W {\n", "\n}")],
        _ => &[("", ""), ("class __W {\n", "\n}")],
    };
    for (prefix, suffix) in wraps {
        let src = format!("{prefix}{text}{suffix}");
        let Ok(tree) = parse_ts(lang, &src) else { continue };
        let mut spans = Vec::new();
        collect(tree.root_node(), kinds, prefix.len(), text.len(), &mut spans);
        let clean = !tree.root_node().has_error();
        if clean {
            return spans;
        }
        if best.as_ref().is_none_or(|b| spans.len() > b.len()) {
            best = Some(spans);
        }
    }
    best.unwrap_or_default()
}

fn collect(n: Node<'_>, kinds: &[&str], offset: usize, len: usize, out: &mut Vec<(u32, u32)>) {
    if kinds.contains(&n.kind()) {
        // `shorthand_field_initializer` wraps the identifier; use the leaf span.
        let leaf = if n.child_count() == 1 { n.child(0).unwrap_or(n) } else { n };
        let (s, e) = (leaf.start_byte(), leaf.end_byte());
        if s >= offset && e <= offset + len {
            out.push(((s - offset) as u32, (e - offset) as u32));
        }
        return;
    }
    let mut cur = n.walk();
    for c in n.children(&mut cur) {
        collect(c, kinds, offset, len, out);
    }
}

/// `text` with every identifier token equal to `from` (variables, callees,
/// members, types) replaced by `to`; string contents and comments are kept.
pub fn rename_identifiers(lang: Lang, text: &str, from: &str, to: &str) -> String {
    if from.is_empty() || from == to || !text.contains(from) {
        return text.to_string();
    }
    let Ok(tree) = parse_ts(lang, text) else { return text.to_string() };
    let mut spans = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(n) = stack.pop() {
        if n.child_count() == 0 {
            if n.is_named() && n.kind().contains("identifier") && text.get(n.start_byte()..n.end_byte()) == Some(from) {
                spans.push((n.start_byte(), n.end_byte()));
            }
            continue;
        }
        if n.kind().contains("comment") {
            continue;
        }
        let mut cur = n.walk();
        stack.extend(n.children(&mut cur));
    }
    spans.sort_unstable();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (s, e) in spans {
        out.push_str(&text[last..s]);
        out.push_str(to);
        last = e;
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorthand_positions() {
        let rs = "fn f(x: u8) -> P { let P { y, .. } = p; P { x, z: y } }";
        let spans: Vec<&str> = shorthand_spans(Lang::Rust, rs).iter().map(|&(s, e)| &rs[s as usize..e as usize]).collect();
        assert_eq!(spans, ["y", "x"]);
        let js = "function f(a) { const { b } = a; return { a, c: b }; }";
        let spans: Vec<&str> = shorthand_spans(Lang::JavaScript, js).iter().map(|&(s, e)| &js[s as usize..e as usize]).collect();
        assert_eq!(spans, ["b", "a"]);
        let method = "render(a) {\n  return { a };\n}";
        assert_eq!(shorthand_spans(Lang::TypeScript, method).len(), 1);
        assert!(shorthand_spans(Lang::Java, "void f() {}").is_empty());
    }

    #[test]
    fn rename_skips_strings_and_comments() {
        let src = "fn t() {\n    // parse is great\n    let v = parse(\"parse\");\n    self.parse();\n}";
        assert_eq!(
            rename_identifiers(Lang::Rust, src, "parse", "load"),
            "fn t() {\n    // parse is great\n    let v = load(\"parse\");\n    self.load();\n}"
        );
        assert_eq!(rename_identifiers(Lang::Python, "x = parse_all(1)", "parse", "load"), "x = parse_all(1)");
    }
}
