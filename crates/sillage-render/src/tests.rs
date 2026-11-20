use std::collections::HashSet;
use std::sync::Arc;

use sillage_core::{Lang, Node, NodeKind, SimplifiedTree, Span};

use crate::{Style, layout, render};

const KEYWORDS: &[&str] = &[
    "try", "catch", "if", "for", "int", "return", "new", "with", "as", "def", "else", "while",
];

/// (start, end, is_keyword) terminals of one source line.
fn lex_line(src: &str, line_start: usize, line: &str) -> Vec<(usize, usize, bool)> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let s = i;
            while i < b.len() && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            let word = &line[s..i];
            out.push((line_start + s, line_start + i, KEYWORDS.contains(&word)));
        } else if c.is_ascii_digit() {
            let s = i;
            while i < b.len() && (b[i] as char).is_ascii_digit() {
                i += 1;
            }
            out.push((line_start + s, line_start + i, false));
        } else if c == '"' {
            let s = i;
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            i += 1;
            out.push((line_start + s, line_start + i.min(b.len()), false));
        } else if c == '&' && b.get(i + 1) == Some(&b'&') {
            out.push((line_start + i, line_start + i + 2, true));
            i += 2;
        } else {
            out.push((line_start + i, line_start + i + 1, true));
            i += 1;
        }
    }
    let _ = src;
    out
}

/// Builds a valid tree: root -> one internal node per line holding tokens
/// (keyword-only lines hang their keywords directly off the root).
fn build(lang: Lang, src: &str) -> SimplifiedTree {
    let source: Arc<str> = Arc::from(src);
    let mut groups: Vec<Vec<(usize, usize, bool)>> = Vec::new();
    let mut off = 0;
    for line in src.split('\n') {
        let terms = lex_line(src, off, line);
        off += line.len() + 1;
        if terms.is_empty() {
            continue;
        }
        let tokens = terms.iter().filter(|t| !t.2).count();
        if tokens == 0 || terms.len() == 1 {
            for t in terms {
                groups.push(vec![t]);
            }
        } else {
            groups.push(terms);
        }
    }

    // A single statement becomes the root itself (Def. 3 forbids wrapping it).
    if groups.len() == 1 {
        groups = groups[0].iter().map(|&t| vec![t]).collect();
    }

    let kind_of = |s: usize, e: usize, kw: bool| {
        let text: Box<str> = src[s..e].into();
        if kw {
            NodeKind::Keyword { text }
        } else {
            NodeKind::Token { text, var: None }
        }
    };

    let mut nodes = vec![Node {
        kind: NodeKind::Internal { label: "".into() },
        parent: None,
        index_in_parent: 0,
        children: vec![],
        span: Span::new(0, src.len() as u32),
        dropped_children: false,
    }];
    let mut leaves = Vec::new();
    let mut root_children = Vec::new();
    let mut root_kinds = Vec::new();
    let mut root_slot = 0u16;
    for g in groups {
        if g.len() == 1 {
            let (s, e, kw) = g[0];
            let id = nodes.len() as u32;
            let idx = if kw {
                0
            } else {
                root_slot += 1;
                root_slot
            };
            let kind = kind_of(s, e, kw);
            root_kinds.push(kind.clone());
            nodes.push(Node {
                kind,
                parent: Some(0),
                index_in_parent: idx,
                children: vec![],
                span: Span::new(s as u32, e as u32),
                dropped_children: false,
            });
            if !kw {
                leaves.push(id);
            }
            root_children.push(id);
            continue;
        }
        root_slot += 1;
        let gid = nodes.len() as u32;
        let kinds: Vec<NodeKind> = g.iter().map(|&(s, e, kw)| kind_of(s, e, kw)).collect();
        let refs: Vec<&NodeKind> = kinds.iter().collect();
        let label = SimplifiedTree::make_label(&refs);
        let gkind = NodeKind::Internal { label: label.into() };
        root_kinds.push(gkind.clone());
        nodes.push(Node {
            kind: gkind,
            parent: Some(0),
            index_in_parent: root_slot,
            children: vec![],
            span: Span::new(g[0].0 as u32, g.last().unwrap().1 as u32),
            dropped_children: false,
        });
        root_children.push(gid);
        let mut slot = 0u16;
        let mut kids = Vec::new();
        for (k, &(s, e, kw)) in kinds.into_iter().zip(g.iter()) {
            let id = nodes.len() as u32;
            let idx = if kw {
                0
            } else {
                slot += 1;
                slot
            };
            nodes.push(Node {
                kind: k,
                parent: Some(gid),
                index_in_parent: idx,
                children: vec![],
                span: Span::new(s as u32, e as u32),
                dropped_children: false,
            });
            if !kw {
                leaves.push(id);
            }
            kids.push(id);
        }
        nodes[gid as usize].children = kids;
    }
    let refs: Vec<&NodeKind> = root_kinds.iter().collect();
    nodes[0].kind = NodeKind::Internal {
        label: SimplifiedTree::make_label(&refs).into(),
    };
    nodes[0].children = root_children;
    let tree = SimplifiedTree {
        lang,
        source,
        nodes,
        leaves,
    };
    tree.validate().expect("valid test tree");
    tree
}

fn line_of(src: &str, off: u32) -> usize {
    src[..off as usize].matches('\n').count() + 1
}

/// Prunes `full` to tokens on lines not in `drop` (1-based); tokens on lines
/// in `extra` are marked as recommended additions.
fn prune(full: &SimplifiedTree, drop: &[usize], extra: &[usize]) -> (SimplifiedTree, Vec<bool>) {
    let src = full.source.clone();
    let keep: Vec<bool> = full
        .leaves
        .iter()
        .map(|&l| !drop.contains(&line_of(&src, full.node(l).span.start)))
        .collect();
    let pruned = full.retain(&keep);
    let extra_lines: HashSet<usize> = extra.iter().copied().collect();
    let qm = pruned
        .leaves
        .iter()
        .map(|&l| !extra_lines.contains(&line_of(&src, pruned.node(l).span.start)))
        .collect();
    (pruned, qm)
}

const JAVA: &str = "        try {
            InputStream is = am.open(fileName);
            Bitmap image = BitmapFactory.decodeStream(is);
            is.close();
            Log.d(TAG, \"decoded\");
        } catch (IOException e) {
            Log.e(TAG, \"failed\", e);
        }";

#[test]
fn java_plain_preserves_indentation_and_placeholders() {
    let full = build(Lang::Java, JAVA);
    let (tree, qm) = prune(&full, &[5, 7], &[4, 6]);
    let out = render(&tree, &qm, None, Style::Plain);
    assert_eq!(
        out,
        "try {
    InputStream is = am.open(fileName);
    Bitmap image = BitmapFactory.decodeStream(is);
    is.close();
    // ...
} catch (IOException e) {
    // ...
}
"
    );
    assert!(!out.contains('\x1b'));
}

#[test]
fn java_layout_marks_extra_lines() {
    let full = build(Lang::Java, JAVA);
    let (tree, qm) = prune(&full, &[5, 7], &[4, 6]);
    let lay = layout(&tree, &qm, None);
    let flags: Vec<(bool, bool)> = lay.lines.iter().map(|l| (l.extra, l.placeholder)).collect();
    assert_eq!(
        flags,
        vec![
            (false, false),
            (false, false),
            (false, false),
            (true, false),
            (true, true),
            (true, false),
            (true, true),
            (false, false),
        ]
    );
    assert_eq!(lay.extra_code_lines(), 2);
    assert_eq!(lay.lines[3].src_line, Some(4));
    assert_eq!(lay.lines[4].src_line, None);
}

#[test]
fn full_tree_renders_source_verbatim_modulo_dedent() {
    let full = build(Lang::Java, JAVA);
    let qm = vec![true; full.leaves.len()];
    let out = render(&full, &qm, None, Style::Plain);
    let expected: String = JAVA
        .lines()
        .map(|l| l.strip_prefix("        ").unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    assert_eq!(out, expected);
}

const PY: &str = "with open(path) as fh:
    data = json.load(fh)
    log.info(\"loaded\", path)
    cache[path] = data
return data";

#[test]
fn python_uses_ellipsis_placeholder() {
    let full = build(Lang::Python, PY);
    let (tree, qm) = prune(&full, &[3], &[5]);
    let out = render(&tree, &qm, None, Style::Plain);
    assert_eq!(
        out,
        "with open(path) as fh:
    data = json.load(fh)
    ...
    cache[path] = data
return data
"
    );
    assert!(!out.contains("//"));
    let lay = layout(&tree, &qm, None);
    assert!(!lay.lines[0].extra);
    assert!(lay.lines[2].placeholder && lay.lines[2].extra);
    assert!(lay.lines[4].extra && !lay.lines[4].placeholder);
}

#[test]
fn inline_drop_collapses_to_single_space() {
    let src = "String s = a + b;";
    let full = build(Lang::Java, src);
    // Leaves: String, s, a, b. Drop `a`.
    let pruned = full.retain(&[true, true, false, true]);
    let qm = vec![true; pruned.leaves.len()];
    let out = render(&pruned, &qm, None, Style::Plain);
    assert_eq!(out, "String s = + b;\n");
    assert!(!out.contains("  "));
}

#[test]
fn ansi_highlights_extra_lines_with_gutter() {
    let full = build(Lang::Java, JAVA);
    let (tree, qm) = prune(&full, &[5, 7], &[4, 6]);
    let out = render(&tree, &qm, None, Style::Ansi);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "  try {");
    assert_eq!(lines[3], "\x1b[1;32m+     is.close();\x1b[0m");
    assert_eq!(lines[4], "\x1b[36m+     // ...\x1b[0m");
    assert!(lines[5].starts_with("\x1b[1;32m+ } catch"));
    assert_eq!(lines[7], "  }");
}

#[test]
fn html_escapes_and_classes_lines() {
    let src = "if (a < b && c > \"x\") {\n    run(a);\n}";
    let full = build(Lang::Java, src);
    let (tree, qm) = prune(&full, &[], &[2]);
    let out = render(&tree, &qm, None, Style::Html);
    assert!(out.starts_with("<pre class=\"sillage-rec\" data-lang=\"java\">"));
    assert!(out.contains(
        "<span class=\"line matched\">if (a &lt; b &amp;&amp; c &gt; &quot;x&quot;) {</span>"
    ));
    assert!(out.contains("<span class=\"line extra\">    run(a);</span>"));
    assert!(out.trim_end().ends_with("</pre>"));
    assert!(!out.contains("a < b"));
    assert_eq!(crate::html::escape("<'&'>"), "&lt;&#39;&amp;&#39;&gt;");
}

#[test]
fn html_marks_placeholder_lines() {
    let full = build(Lang::Java, JAVA);
    let (tree, qm) = prune(&full, &[5, 7], &[]);
    let out = render(&tree, &qm, None, Style::Html);
    assert!(out.contains("<span class=\"line extra placeholder\">    // ...</span>"));
}

#[test]
fn json_has_structured_lines_and_diff() {
    let full = build(Lang::Java, JAVA);
    let (tree, qm) = prune(&full, &[5, 7], &[4, 6]);
    let query = "InputStream is = am.open(fileName);\nBitmap image = BitmapFactory.decodeStream(is);";
    let out = render(&tree, &qm, Some(query), Style::Json);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["lang"], "java");
    assert!(v["code"].as_str().unwrap().starts_with("try {\n    InputStream is"));
    let lines = v["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 8);
    assert_eq!(lines[0]["line_no"], 1);
    assert_eq!(lines[3]["text"], "    is.close();");
    assert_eq!(lines[3]["extra"], true);
    assert_eq!(lines[4]["placeholder"], true);
    assert_eq!(lines[1]["extra"], false);
    let diff = &v["diff"];
    assert_eq!(diff["unchanged"], 2);
    assert_eq!(diff["deleted"], 0);
    assert_eq!(diff["inserted"], 6);
    assert!(diff["unified"].as_str().unwrap().contains("+is.close();"));

    let no_diff = render(&tree, &qm, None, Style::Json);
    let v: serde_json::Value = serde_json::from_str(&no_diff).unwrap();
    assert!(v.get("diff").is_none());
}

#[test]
fn empty_tree_renders_empty() {
    let t = SimplifiedTree::empty(Lang::Go, Arc::from("func f() {}"));
    assert_eq!(render(&t, &[], None, Style::Plain), "");
    assert_eq!(render(&t, &[], None, Style::Ansi), "");
    let v: serde_json::Value = serde_json::from_str(&render(&t, &[], None, Style::Json)).unwrap();
    assert_eq!(v["lines"].as_array().unwrap().len(), 0);
}

#[test]
fn style_from_str() {
    assert_eq!("ANSI".parse::<Style>().unwrap(), Style::Ansi);
    assert_eq!("json".parse::<Style>().unwrap(), Style::Json);
    assert!("pdf".parse::<Style>().is_err());
}
