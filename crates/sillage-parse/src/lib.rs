use std::path::Path;

use sillage_core::{Lang, SimplifiedTree, Span};

#[derive(Debug, Clone)]
pub struct ExtractedMethod {
    pub name: Box<str>,
    pub line_start: u32,
    pub text_span: Span,
    pub body_span: Span,
    pub tree: SimplifiedTree,
}

pub fn lang_from_path(_p: &Path) -> Option<Lang> {
    todo!("Slice A")
}

pub fn extract_methods(_lang: Lang, _src: &str) -> Vec<ExtractedMethod> {
    todo!("Slice A")
}

pub fn parse_method(_lang: Lang, _text: &str, _body: Span) -> anyhow::Result<SimplifiedTree> {
    todo!("Slice A")
}

pub fn parse_snippet(_lang: Lang, _snippet: &str) -> anyhow::Result<SimplifiedTree> {
    todo!("Slice A")
}
