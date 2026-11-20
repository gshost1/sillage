//! Structured JSON rendering.

use serde::Serialize;

use crate::layout::{DiffSummary, Layout, Line};

#[derive(Debug, Serialize)]
struct JsonOut<'a> {
    lang: &'a str,
    code: String,
    lines: Vec<JsonLine<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diff: Option<&'a DiffSummary>,
}

#[derive(Debug, Serialize)]
struct JsonLine<'a> {
    line_no: usize,
    text: &'a str,
    extra: bool,
    placeholder: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    src_line: Option<usize>,
}

impl<'a> From<&'a Line> for JsonLine<'a> {
    fn from(l: &'a Line) -> Self {
        Self {
            line_no: l.line_no,
            text: &l.text,
            extra: l.extra,
            placeholder: l.placeholder,
            src_line: l.src_line,
        }
    }
}

/// Serializes the layout as a `serde_json::Value`.
pub fn to_value(layout: &Layout) -> serde_json::Value {
    let out = JsonOut {
        lang: &layout.lang,
        code: layout.code(),
        lines: layout.lines.iter().map(JsonLine::from).collect(),
        diff: layout.diff.as_ref(),
    };
    serde_json::to_value(out).expect("layout serializes")
}

pub fn render(layout: &Layout) -> String {
    serde_json::to_string_pretty(&to_value(layout)).expect("layout serializes")
}
