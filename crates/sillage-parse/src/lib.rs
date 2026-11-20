//! Tree-sitter front end for sillage: simplified parse trees (Aroma §3.1),
//! lexical scoping of local variables, method extraction and error-tolerant
//! snippet parsing for Java, Python, JavaScript, TypeScript/TSX, Go and Rust.

use std::cell::RefCell;
use std::path::Path;

use anyhow::{Context, Result};
use sillage_core::{Lang, SimplifiedTree, Span};
use tree_sitter::{Parser, Tree};

mod extract;
mod imports;
mod lang;
mod lower;
mod rename;
mod scope;
mod snippet;
mod testdetect;

pub use imports::{extract_imports, referenced_identifiers, relevant_imports, relevant_imports_excluding};
pub use rename::{rename_identifiers, shorthand_spans};
pub use testdetect::{is_test_method, is_test_path};

#[derive(Debug, Clone)]
pub struct ExtractedMethod {
    pub name: Box<str>,
    pub line_start: u32,
    pub text_span: Span,
    pub body_span: Span,
    pub tree: SimplifiedTree,
    /// Test context outside the method's own text: a Rust `#[test]`-style
    /// attribute or `#[cfg(test)]` module, a Python `Test*` / `TestCase` class or
    /// pytest decorator, a JS/TS `describe`/`it`/`test` callback, a Java test class.
    pub test_context: bool,
}

/// The tree-sitter grammar used for `lang`.
pub fn ts_language(lang: Lang) -> tree_sitter::Language {
    lang::ts_language(lang)
}

pub fn lang_from_path(p: &Path) -> Option<Lang> {
    let ext = p.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "java" => Lang::Java,
        "py" | "pyi" => Lang::Python,
        "js" | "jsx" | "mjs" | "cjs" => Lang::JavaScript,
        "ts" | "mts" | "cts" => Lang::TypeScript,
        "tsx" => Lang::Tsx,
        "go" => Lang::Go,
        "rs" => Lang::Rust,
        _ => return None,
    })
}

/// Extracts every method/function with a body from `src`.
///
/// `text_span` indexes `src`; `body_span` is relative to `text_span.start`, and
/// `tree` equals `parse_method(lang, &src[text_span], body_span)`.
pub fn extract_methods(lang: Lang, src: &str) -> Vec<ExtractedMethod> {
    extract::extract(lang, src)
}

/// Parses a full method's text (as extracted) and returns the simplified tree of
/// its body (`body` is relative to `text`); parameters still resolve as locals.
/// `tree.source` is `text`, so spans index the full method text.
pub fn parse_method(lang: Lang, text: &str, body: Span) -> Result<SimplifiedTree> {
    snippet::parse_method(lang, text, body)
}

/// Parses a query snippet; spans index into `snippet`.
pub fn parse_snippet(lang: Lang, snippet: &str) -> Result<SimplifiedTree> {
    snippet::parse_snippet(lang, snippet)
}

thread_local! {
    static PARSERS: RefCell<Vec<Option<Parser>>> =
        RefCell::new((0..Lang::ALL.len()).map(|_| None).collect());
}

pub(crate) fn parse_ts(lang: Lang, text: &str) -> Result<Tree> {
    PARSERS.with(|cell| {
        let mut parsers = cell.borrow_mut();
        let slot = &mut parsers[lang::lang_index(lang)];
        if slot.is_none() {
            let mut parser = Parser::new();
            parser
                .set_language(&lang::ts_language(lang))
                .with_context(|| format!("loading tree-sitter grammar for {lang}"))?;
            *slot = Some(parser);
        }
        let parser = slot.as_mut().expect("parser initialised");
        parser
            .parse(text, None)
            .with_context(|| format!("tree-sitter failed to parse {lang} source"))
    })
}
