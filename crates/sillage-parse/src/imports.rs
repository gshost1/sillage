//! File-level imports: extraction ([`extract_imports`]) and the subset a
//! recommended snippet actually needs ([`relevant_imports`]).
//!
//! Imports are stored as single-line statements (whitespace runs collapsed,
//! Rust visibility dropped, one statement per Go import spec), so each one can
//! be re-parsed on its own. [`relevant_imports`] parses the extra code with
//! tree-sitter, collects the identifiers it references (not member / field
//! names, strings or comments), and keeps only the import statements (or, for
//! multi-name statements, the import items) that bind one of them.

use rustc_hash::FxHashSet;
use sillage_core::Lang;
use tree_sitter::Node;

use crate::parse_ts;

/// Every import statement in `src`, deduplicated, in source order.
pub fn extract_imports(lang: Lang, src: &str) -> Vec<Box<str>> {
    let Ok(tree) = parse_ts(lang, src) else {
        return Vec::new();
    };
    let mut out: Vec<Box<str>> = Vec::new();
    let mut seen: FxHashSet<Box<str>> = FxHashSet::default();
    let mut push = |s: String| {
        let s: Box<str> = s.into();
        if !s.is_empty() && seen.insert(s.clone()) {
            out.push(s);
        }
    };
    let root = tree.root_node();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let mut descend = true;
        match (lang, node.kind()) {
            (Lang::Java, "import_declaration") => {
                push(collapse(text(node, src)));
                descend = false;
            }
            (Lang::Python, "import_statement" | "import_from_statement") => {
                push(collapse(text(node, src)));
                descend = false;
            }
            (Lang::JavaScript | Lang::TypeScript | Lang::Tsx, "import_statement") => {
                push(with_semicolon(collapse(text(node, src))));
                descend = false;
            }
            (Lang::JavaScript | Lang::TypeScript | Lang::Tsx, "lexical_declaration" | "variable_declaration")
                if node.parent().is_some_and(|p| p.id() == root.id()) && is_require_decl(node, src) =>
            {
                push(with_semicolon(collapse(text(node, src))));
                descend = false;
            }
            (Lang::Go, "import_spec") => {
                push(format!("import {}", collapse(text(node, src))));
                descend = false;
            }
            (Lang::Rust, "use_declaration" | "extern_crate_declaration") => {
                let mut s = collapse(text(node, src));
                if let Some(vis) = child_of_kind(node, "visibility_modifier") {
                    let v = collapse(text(vis, src));
                    if let Some(rest) = s.strip_prefix(v.as_str()) {
                        s = rest.trim_start().to_string();
                    }
                }
                push(s);
                descend = false;
            }
            _ => {}
        }
        if descend {
            let mut cur = node.walk();
            let children: Vec<Node<'_>> = node.children(&mut cur).collect();
            stack.extend(children.into_iter().rev());
        }
    }
    out
}

/// The imports of `file_imports` that the identifiers referenced by
/// `extra_code` need. Multi-name statements are narrowed to the referenced
/// names (`from m import a` out of `from m import a, b`). A wildcard import is
/// kept only when `extra_code` references a capitalised name that no specific
/// import binds; side-effect imports (`import './x.css'`, Go `_`) never are.
pub fn relevant_imports(lang: Lang, file_imports: &[Box<str>], extra_code: &str) -> Vec<String> {
    relevant_imports_excluding(lang, file_imports, extra_code, &[])
}

/// [`relevant_imports`] without the imports the caller already has: an item is
/// dropped when its text is in `existing` or every name it binds is bound by
/// an `existing` import.
pub fn relevant_imports_excluding(
    lang: Lang,
    file_imports: &[Box<str>],
    extra_code: &str,
    existing: &[String],
) -> Vec<String> {
    let (referenced, _) = identifiers(lang, extra_code);
    if referenced.is_empty() || file_imports.is_empty() {
        return Vec::new();
    }
    let parsed: Vec<Analyzed> = file_imports.iter().filter_map(|s| analyze(lang, s)).collect();
    let have: Vec<Analyzed> = existing.iter().filter_map(|s| analyze(lang, s)).collect();
    let have_text: FxHashSet<String> = existing.iter().map(|s| collapse(s)).collect();
    let have_bound: FxHashSet<&str> =
        have.iter().flat_map(|a| a.items.iter()).flat_map(|i| i.bindings.iter().map(String::as_str)).collect();
    let have_wild: FxHashSet<&str> = have.iter().filter(|a| a.items.iter().any(|i| i.wildcard)).map(|a| a.stmt.as_str()).collect();

    let bound: FxHashSet<&str> =
        parsed.iter().flat_map(|a| a.items.iter()).flat_map(|i| i.bindings.iter().map(String::as_str)).collect();
    let unresolved_caps = referenced.iter().any(|r| {
        r.chars().next().is_some_and(char::is_uppercase)
            && !bound.contains(r.as_str())
            && !have_bound.contains(r.as_str())
            && !is_builtin_type(lang, r)
    });

    let mut out: Vec<String> = Vec::new();
    for a in &parsed {
        let keep: Vec<&Item> = a
            .items
            .iter()
            .filter(|i| {
                if i.wildcard {
                    return unresolved_caps && !have_wild.contains(a.stmt.as_str());
                }
                let used: Vec<&String> = i.bindings.iter().filter(|b| referenced.contains(b.as_str())).collect();
                !used.is_empty() && !used.iter().all(|b| have_bound.contains(b.as_str()))
            })
            .collect();
        if keep.is_empty() {
            continue;
        }
        let s = a.rebuild(&keep);
        if !have_text.contains(&s) && !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// Identifiers referenced by `code` (parsed with tree-sitter, error tolerant):
/// variables, types, packages, callees and macro names, but not member / field
/// names after `.` or `::`, string contents or comments.
pub fn referenced_identifiers(lang: Lang, code: &str) -> FxHashSet<String> {
    identifiers(lang, code).0
}

/// `(referenced, declared)`: [`referenced_identifiers`], and the names `code`
/// declares itself (generic parameters, type / function / class names), which
/// never need an import.
fn identifiers(lang: Lang, code: &str) -> (FxHashSet<String>, FxHashSet<String>) {
    let (prefix, suffix) = match lang {
        Lang::Java => ("class __W { void __m() {\n", "\n}}\n"),
        Lang::Go => ("package __p\nfunc __m() {\n", "\n}\n"),
        Lang::Rust => ("fn __m() {\n", "\n}\n"),
        _ => ("", ""),
    };
    let wrapped = format!("{prefix}{code}{suffix}");
    let (mut refs, mut decls) = (FxHashSet::default(), FxHashSet::default());
    let Ok(tree) = parse_ts(lang, &wrapped) else {
        return (refs, decls);
    };
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.child_count() == 0 {
            let t = text(node, &wrapped);
            if t.is_empty() || matches!(t, "__W" | "__m" | "__p") {
                continue;
            }
            if is_decl_ident(node) {
                decls.insert(t.to_string());
            } else if is_ref_ident(lang, node) {
                refs.insert(t.to_string());
            }
            continue;
        }
        if matches!(node.kind(), "comment" | "line_comment" | "block_comment") {
            continue;
        }
        let mut cur = node.walk();
        stack.extend(node.children(&mut cur));
    }
    for d in &decls {
        refs.remove(d);
    }
    (refs, decls)
}

/// A generic parameter or the name of a type / function / class declaration.
fn is_decl_ident(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else { return false };
    let field = field_in_parent(node, parent);
    match parent.kind() {
        "type_parameters" | "type_parameter_list" => true,
        "constrained_type_parameter" => field == Some("left"),
        "type_parameter" | "type_parameter_declaration" => field == Some("name") || field.is_none(),
        k => {
            field == Some("name")
                && matches!(
                    k,
                    "struct_item" | "enum_item" | "trait_item" | "type_item" | "union_item" | "class_declaration"
                        | "interface_declaration" | "enum_declaration" | "record_declaration" | "class_definition"
                        | "type_alias_declaration" | "type_spec" | "function_item" | "function_definition"
                )
        }
    }
}

/// Capitalised names every file sees without an import (`java.lang`, the Rust
/// prelude, Python builtins), so they never justify a wildcard import.
fn is_builtin_type(lang: Lang, name: &str) -> bool {
    const JAVA: &[&str] = &[
        "String", "Object", "Integer", "Long", "Double", "Float", "Short", "Byte", "Character", "Boolean", "Number",
        "Math", "System", "Thread", "Runnable", "Iterable", "Comparable", "CharSequence", "StringBuilder", "Class",
        "Void", "Override", "Deprecated", "SuppressWarnings", "FunctionalInterface", "Enum", "Record", "Throwable",
        "Error", "Exception", "RuntimeException", "AutoCloseable", "Cloneable", "Process", "Runtime", "StrictMath",
    ];
    const RUST: &[&str] = &[
        "Option", "Some", "None", "Result", "Ok", "Err", "Vec", "String", "Box", "Self", "Default", "Clone", "Copy",
        "Send", "Sync", "Sized", "Drop", "Fn", "FnMut", "FnOnce", "Iterator", "IntoIterator", "Extend", "ToString",
        "ToOwned", "From", "Into", "TryFrom", "TryInto", "AsRef", "AsMut", "PartialEq", "Eq", "PartialOrd", "Ord",
        "Debug", "Hash", "DoubleEndedIterator", "ExactSizeIterator", "Unpin",
    ];
    const JS: &[&str] = &[
        "Array", "Object", "String", "Number", "Boolean", "Promise", "Map", "Set", "WeakMap", "WeakSet", "Date",
        "JSON", "Math", "Error", "TypeError", "RangeError", "RegExp", "Symbol", "BigInt", "Infinity", "NaN", "Reflect",
        "Proxy", "Intl", "URL", "URLSearchParams", "Buffer", "Record", "Partial", "Readonly", "Pick", "Omit",
        "ReturnType", "Exclude", "Extract", "Required", "NonNullable", "Awaited", "Parameters",
    ];
    match lang {
        Lang::Java => JAVA.contains(&name),
        Lang::Rust => RUST.contains(&name),
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => JS.contains(&name),
        Lang::Python => {
            matches!(name, "True" | "False" | "None" | "Ellipsis" | "NotImplemented")
                || name.ends_with("Error")
                || name.ends_with("Exception")
                || name.ends_with("Warning")
                || matches!(name, "BaseException" | "KeyboardInterrupt" | "StopIteration" | "SystemExit" | "GeneratorExit")
        }
        Lang::Go => false,
    }
}

fn is_ref_ident(lang: Lang, node: Node<'_>) -> bool {
    let kind = node.kind();
    if !node.is_named() {
        return false;
    }
    let ok_kind = matches!(
        kind,
        "identifier" | "type_identifier" | "package_identifier" | "shorthand_property_identifier" | "namespace_identifier"
    );
    if !ok_kind {
        return false;
    }
    let Some(parent) = node.parent() else { return true };
    let field = field_in_parent(node, parent);
    match (lang, parent.kind(), field) {
        (Lang::Python, "attribute", Some("attribute")) => false,
        (Lang::Python, "keyword_argument", Some("name")) => false,
        (Lang::Java, "field_access", Some("field")) => false,
        (Lang::Java, "method_invocation", Some("name")) => parent.child_by_field_name("object").is_none(),
        (Lang::Java, "scoped_identifier" | "scoped_type_identifier", Some("name")) => false,
        (Lang::Rust, "scoped_identifier" | "scoped_type_identifier", Some("name")) => {
            parent.child_by_field_name("path").is_none()
        }
        (Lang::Rust, "field_expression", Some("field")) => false,
        (Lang::Rust, "field_initializer", Some("field")) => false,
        (Lang::Go, "qualified_type", Some("name")) => false,
        (Lang::Go, "selector_expression", Some("field")) => false,
        (_, "nested_type_identifier", Some("name")) => false,
        _ => true,
    }
}

fn field_in_parent<'t>(node: Node<'t>, parent: Node<'t>) -> Option<&'static str> {
    let mut cur = parent.walk();
    if !cur.goto_first_child() {
        return None;
    }
    loop {
        if cur.node().id() == node.id() {
            return cur.field_name();
        }
        if !cur.goto_next_sibling() {
            return None;
        }
    }
}

/// One importable unit of a statement: its text, the names it binds, and
/// whether it is a wildcard (`*`, Go `.`).
#[derive(Debug, Clone)]
struct Item {
    text: String,
    bindings: Vec<String>,
    wildcard: bool,
}

#[derive(Debug)]
enum Shape {
    /// The statement is one unit.
    Whole,
    /// `from {module} import a, b as c`.
    PyFrom { module: String },
    /// `import a, b as c`.
    PyImport,
    /// `import {type }{default}, {* as ns | { named }} from {source};`.
    Js { type_prefix: bool, source: String, kinds: Vec<JsKind> },
    /// `use {path}::{items};`.
    RustList { path: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsKind {
    Default,
    Namespace,
    Named,
}

#[derive(Debug)]
struct Analyzed {
    stmt: String,
    shape: Shape,
    items: Vec<Item>,
}

impl Analyzed {
    fn rebuild(&self, keep: &[&Item]) -> String {
        if keep.len() == self.items.len() && matches!(self.shape, Shape::Whole | Shape::PyImport | Shape::RustList { .. }) {
            return self.stmt.clone();
        }
        let join = |v: &[&Item]| v.iter().map(|i| i.text.as_str()).collect::<Vec<_>>().join(", ");
        match &self.shape {
            Shape::Whole => self.stmt.clone(),
            Shape::PyFrom { module } => format!("from {module} import {}", join(keep)),
            Shape::PyImport => format!("import {}", join(keep)),
            Shape::RustList { path } => match keep {
                [only] if only.text == "self" => format!("use {path};"),
                [only] => format!("use {path}::{};", only.text),
                _ => format!("use {path}::{{{}}};", join(keep)),
            },
            Shape::Js { type_prefix, source, kinds } => {
                let kind_of = |it: &Item| {
                    self.items.iter().position(|x| std::ptr::eq(x, it)).map(|p| kinds[p]).unwrap_or(JsKind::Named)
                };
                let mut parts: Vec<String> = Vec::new();
                for &it in keep {
                    if kind_of(it) != JsKind::Named {
                        parts.push(it.text.clone());
                    }
                }
                let named: Vec<&Item> = keep.iter().copied().filter(|it| kind_of(it) == JsKind::Named).collect();
                if !named.is_empty() {
                    parts.push(format!("{{ {} }}", join(&named)));
                }
                let t = if *type_prefix { "type " } else { "" };
                format!("import {t}{} from {source};", parts.join(", "))
            }
        }
    }
}

fn analyze(lang: Lang, stmt: &str) -> Option<Analyzed> {
    let stmt = collapse(stmt);
    let src = match lang {
        Lang::Go => format!("package __p\n{stmt}\n"),
        _ => stmt.clone(),
    };
    let tree = parse_ts(lang, &src).ok()?;
    let root = tree.root_node();
    let whole = |bindings: Vec<String>, wildcard: bool| Analyzed {
        stmt: stmt.clone(),
        shape: Shape::Whole,
        items: vec![Item { text: stmt.clone(), bindings, wildcard }],
    };
    let kinds: &[&str] = match lang {
        Lang::Java => &["import_declaration"],
        Lang::Python => &["import_statement", "import_from_statement"],
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => &["import_statement", "lexical_declaration", "variable_declaration"],
        Lang::Go => &["import_spec"],
        Lang::Rust => &["use_declaration", "extern_crate_declaration"],
    };
    let node = first_of_kinds(root, kinds)?;
    let t = |n: Node<'_>| text(n, &src).to_string();
    match (lang, node.kind()) {
        (Lang::Java, _) => {
            let wildcard = child_of_kind(node, "asterisk").is_some();
            let path = (0..node.named_child_count())
                .filter_map(|i| node.named_child(i))
                .find(|c| matches!(c.kind(), "scoped_identifier" | "identifier"))?;
            let last = last_segment(&t(path), '.');
            Some(whole(if wildcard { vec![] } else { vec![last] }, wildcard))
        }
        (Lang::Python, "import_statement") => {
            let items: Vec<Item> = named_children_by_field(node, "name")
                .into_iter()
                .map(|n| {
                    let b = if n.kind() == "aliased_import" {
                        n.child_by_field_name("alias").map(t).unwrap_or_default()
                    } else {
                        t(n).split('.').next().unwrap_or_default().to_string()
                    };
                    Item { text: t(n), bindings: vec![b], wildcard: false }
                })
                .collect();
            Some(Analyzed { stmt: stmt.clone(), shape: Shape::PyImport, items })
        }
        (Lang::Python, "import_from_statement") => {
            let module = node.child_by_field_name("module_name").map(t)?;
            if child_of_kind(node, "wildcard_import").is_some() {
                return Some(whole(vec![], true));
            }
            let items: Vec<Item> = named_children_by_field(node, "name")
                .into_iter()
                .map(|n| {
                    let b = if n.kind() == "aliased_import" {
                        n.child_by_field_name("alias").map(t).unwrap_or_default()
                    } else {
                        t(n)
                    };
                    Item { text: t(n), bindings: vec![b], wildcard: false }
                })
                .collect();
            Some(Analyzed { stmt: stmt.clone(), shape: Shape::PyFrom { module }, items })
        }
        (Lang::JavaScript | Lang::TypeScript | Lang::Tsx, "import_statement") => {
            if let Some(req) = child_of_kind(node, "import_require_clause") {
                let name = child_of_kind(req, "identifier").map(t).unwrap_or_default();
                return Some(whole(vec![name], false));
            }
            let source = node.child_by_field_name("source").map(t)?;
            let Some(clause) = child_of_kind(node, "import_clause") else {
                return Some(whole(vec![], false)); // side-effect import
            };
            let type_prefix = stmt.starts_with("import type ");
            let (mut items, mut kinds) = (Vec::new(), Vec::new());
            for i in 0..clause.named_child_count() {
                let Some(c) = clause.named_child(i) else { continue };
                match c.kind() {
                    "identifier" => {
                        items.push(Item { text: t(c), bindings: vec![t(c)], wildcard: false });
                        kinds.push(JsKind::Default);
                    }
                    "namespace_import" => {
                        let b = child_of_kind(c, "identifier").map(t).unwrap_or_default();
                        items.push(Item { text: t(c), bindings: vec![b], wildcard: false });
                        kinds.push(JsKind::Namespace);
                    }
                    "named_imports" => {
                        for j in 0..c.named_child_count() {
                            let Some(s) = c.named_child(j) else { continue };
                            if s.kind() != "import_specifier" {
                                continue;
                            }
                            let b = s.child_by_field_name("alias").or_else(|| s.child_by_field_name("name")).map(t);
                            items.push(Item { text: t(s), bindings: b.into_iter().collect(), wildcard: false });
                            kinds.push(JsKind::Named);
                        }
                    }
                    _ => {}
                }
            }
            Some(Analyzed { stmt: stmt.clone(), shape: Shape::Js { type_prefix, source, kinds }, items })
        }
        (Lang::JavaScript | Lang::TypeScript | Lang::Tsx, _) => {
            let mut names = Vec::new();
            let mut stack = vec![node];
            while let Some(n) = stack.pop() {
                if n.kind() == "variable_declarator" {
                    if let Some(name) = n.child_by_field_name("name") {
                        collect_kinds(
                            name,
                            &src,
                            &["identifier", "shorthand_property_identifier_pattern"],
                            &mut names,
                        );
                    }
                    continue;
                }
                let mut cur = n.walk();
                stack.extend(n.named_children(&mut cur));
            }
            Some(whole(names, false))
        }
        (Lang::Go, _) => {
            let path = node.child_by_field_name("path").map(t)?;
            let path = path.trim_matches(|c| c == '"' || c == '`');
            match node.child_by_field_name("name") {
                Some(n) if n.kind() == "blank_identifier" => Some(whole(vec![], false)),
                Some(n) if n.kind() == "dot" => Some(whole(vec![], true)),
                Some(n) => Some(whole(vec![t(n)], false)),
                None => Some(whole(go_package_names(path), false)),
            }
        }
        (Lang::Rust, "extern_crate_declaration") => {
            let b = node.child_by_field_name("alias").or_else(|| node.child_by_field_name("name")).map(t);
            Some(whole(b.into_iter().collect(), false))
        }
        (Lang::Rust, _) => {
            let arg = node.child_by_field_name("argument")?;
            if arg.kind() == "scoped_use_list"
                && let (Some(path), Some(list)) = (arg.child_by_field_name("path"), arg.child_by_field_name("list"))
            {
                let p = t(path);
                let items = (0..list.named_child_count())
                    .filter_map(|i| list.named_child(i))
                    .map(|c| {
                        let mut b = Vec::new();
                        let wildcard = rust_bindings(c, &src, &p, &mut b);
                        Item { text: t(c), bindings: b, wildcard }
                    })
                    .collect();
                return Some(Analyzed { stmt: stmt.clone(), shape: Shape::RustList { path: p }, items });
            }
            let mut b = Vec::new();
            let wildcard = rust_bindings(arg, &src, "", &mut b);
            Some(whole(b, wildcard))
        }
        _ => None,
    }
}

/// Names bound by a Rust use tree; returns `true` when it contains a wildcard.
fn rust_bindings(n: Node<'_>, src: &str, parent_path: &str, out: &mut Vec<String>) -> bool {
    let t = |n: Node<'_>| text(n, src).to_string();
    match n.kind() {
        "identifier" | "type_identifier" => {
            out.push(t(n));
            false
        }
        "self" => {
            let last = last_segment(parent_path, ':');
            if !last.is_empty() {
                out.push(last);
            }
            false
        }
        "scoped_identifier" => {
            if let Some(name) = n.child_by_field_name("name") {
                if name.kind() == "self" {
                    let p = n.child_by_field_name("path").map(t).unwrap_or_default();
                    out.push(last_segment(&p, ':'));
                } else {
                    out.push(t(name));
                }
            }
            false
        }
        "use_as_clause" => {
            if let Some(a) = n.child_by_field_name("alias") {
                out.push(t(a));
            }
            false
        }
        "use_wildcard" => true,
        "scoped_use_list" => {
            let p = n.child_by_field_name("path").map(t).unwrap_or_default();
            let full = if parent_path.is_empty() { p } else { format!("{parent_path}::{p}") };
            n.child_by_field_name("list").is_some_and(|l| rust_bindings(l, src, &full, out))
        }
        "use_list" => {
            let mut wild = false;
            for i in 0..n.named_child_count() {
                if let Some(c) = n.named_child(i) {
                    wild |= rust_bindings(c, src, parent_path, out);
                }
            }
            wild
        }
        _ => false,
    }
}

/// Candidate package names of a Go import path: the last segment, without a
/// `.vN` suffix or `go-` prefix, or the one before a `/vN` major-version suffix.
fn go_package_names(path: &str) -> Vec<String> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let Some(&last) = segs.last() else { return Vec::new() };
    let is_major = |s: &str| s.len() > 1 && s.starts_with('v') && s[1..].bytes().all(|b| b.is_ascii_digit());
    let base = if is_major(last) && segs.len() > 1 { segs[segs.len() - 2] } else { last };
    let mut names = vec![base.to_string()];
    if let Some((head, tail)) = base.rsplit_once('.')
        && is_major(tail)
    {
        names.push(head.to_string());
    }
    let extra: Vec<String> = names
        .iter()
        .flat_map(|n| {
            let mut v = Vec::new();
            if let Some(s) = n.strip_prefix("go-") {
                v.push(s.to_string());
            }
            if let Some(s) = n.strip_suffix("-go").or_else(|| n.strip_suffix(".go")) {
                v.push(s.to_string());
            }
            if n.contains('-') {
                v.push(n.replace('-', ""));
                v.push(n.replace('-', "_"));
            }
            v
        })
        .collect();
    names.extend(extra);
    names
}

fn is_require_decl(node: Node<'_>, src: &str) -> bool {
    let mut cur = node.walk();
    node.named_children(&mut cur).any(|d| {
        d.kind() == "variable_declarator"
            && d.child_by_field_name("value").is_some_and(|v| {
                v.kind() == "call_expression"
                    && v.child_by_field_name("function").is_some_and(|f| text(f, src) == "require")
            })
    })
}

fn collect_kinds(n: Node<'_>, src: &str, kinds: &[&str], out: &mut Vec<String>) {
    if kinds.contains(&n.kind()) {
        out.push(text(n, src).to_string());
        return;
    }
    if n.kind() == "pair_pattern" {
        if let Some(v) = n.child_by_field_name("value") {
            collect_kinds(v, src, kinds, out);
        }
        return;
    }
    let mut cur = n.walk();
    for c in n.named_children(&mut cur) {
        collect_kinds(c, src, kinds, out);
    }
}

fn named_children_by_field<'t>(node: Node<'t>, field: &str) -> Vec<Node<'t>> {
    let mut cur = node.walk();
    node.children_by_field_name(field, &mut cur).collect()
}

fn first_of_kinds<'t>(root: Node<'t>, kinds: &[&str]) -> Option<Node<'t>> {
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if kinds.contains(&n.kind()) {
            return Some(n);
        }
        let mut cur = n.walk();
        let children: Vec<Node<'t>> = n.named_children(&mut cur).collect();
        stack.extend(children.into_iter().rev());
    }
    None
}

fn child_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut cur = node.walk();
    node.children(&mut cur).find(|c| c.kind() == kind)
}

fn last_segment(path: &str, sep: char) -> String {
    path.rsplit(sep).find(|s| !s.is_empty()).unwrap_or_default().trim().to_string()
}

fn text<'s>(n: Node<'_>, src: &'s str) -> &'s str {
    src.get(n.start_byte()..n.end_byte()).unwrap_or("")
}

/// Collapses whitespace runs to one space (none after `(` / `{` or before `)` / `}` / `,`).
fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending = false;
    for c in s.trim().chars() {
        if c.is_whitespace() {
            pending = true;
            continue;
        }
        if pending && !out.is_empty() && !out.ends_with(['(', '[']) && !matches!(c, ')' | ']' | ',' | ';') {
            out.push(' ');
        }
        pending = false;
        out.push(c);
    }
    out
}

fn with_semicolon(mut s: String) -> String {
    if !s.ends_with(';') {
        s.push(';');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imp(lang: Lang, src: &str) -> Vec<String> {
        extract_imports(lang, src).into_iter().map(String::from).collect()
    }

    #[test]
    fn collapse_normalizes_whitespace() {
        assert_eq!(collapse("from x import (\n  a,\n  b\n)"), "from x import (a, b)");
        assert_eq!(collapse("use a::{\n  b,\n  c,\n};"), "use a::{ b, c, };");
        assert_eq!(collapse("import { a ,b } from 'x'"), "import { a,b } from 'x'");
    }

    #[test]
    fn go_package_name_heuristics() {
        assert_eq!(go_package_names("fmt"), ["fmt"]);
        assert_eq!(go_package_names("net/http"), ["http"]);
        assert!(go_package_names("gopkg.in/yaml.v2").contains(&"yaml".to_string()));
        assert!(go_package_names("github.com/x/foo/v5").contains(&"foo".to_string()));
        assert!(go_package_names("github.com/google/go-cmp").contains(&"cmp".to_string()));
    }

    #[test]
    fn rust_use_lists_extract_whole_statements() {
        let v = imp(Lang::Rust, "pub use a::b;\nuse a::b;\nfn f() { use std::fmt::Write; }");
        assert_eq!(v, ["use a::b;", "use std::fmt::Write;"]);
    }
}
