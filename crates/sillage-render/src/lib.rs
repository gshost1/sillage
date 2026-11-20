//! Source-preserving rendering of Sillage recommendations.

use sillage_core::SimplifiedTree;

pub mod ansi;
pub mod html;
pub mod json;
pub mod layout;

pub use layout::{DiffSummary, Layout, Line, diff_summary, layout};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Plain,
    Ansi,
    Html,
    Json,
}

impl std::str::FromStr for Style {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "plain" | "text" => Ok(Style::Plain),
            "ansi" | "color" | "colour" => Ok(Style::Ansi),
            "html" => Ok(Style::Html),
            "json" => Ok(Style::Json),
            other => Err(format!("unknown render style: {other}")),
        }
    }
}

/// Renders `tree` with lines whose tokens are not selected by `query_match`
/// (aligned with `tree.leaves`) marked as recommended additions.
pub fn render(
    tree: &SimplifiedTree,
    query_match: &[bool],
    query: Option<&str>,
    style: Style,
) -> String {
    render_layout(&layout(tree, query_match, query), style)
}

/// Renders an already computed (e.g. [`Layout::locate`]d or
/// [`Layout::truncate`]d) layout.
pub fn render_layout(lay: &Layout, style: Style) -> String {
    match style {
        Style::Plain => {
            let mut code = lay.code();
            if !code.is_empty() {
                code.push('\n');
            }
            code
        }
        Style::Ansi => ansi::render(lay),
        Style::Html => html::render(lay),
        Style::Json => json::render(lay),
    }
}

#[cfg(test)]
mod tests;
