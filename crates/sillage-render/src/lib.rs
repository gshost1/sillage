//! Source-preserving rendering of Sillage recommendations.

use sillage_core::SimplifiedTree;

pub mod ansi;
pub mod html;
pub mod json;
pub mod layout;

pub use layout::{DiffSummary, Layout, Line, diff_summary, layout, layout_with, unified_diff};

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

/// How much of a recommendation an agent-facing renderer shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RenderMode {
    /// Recommended (extra) lines with one line of context around each run;
    /// skipped lines collapse into a placeholder.
    #[default]
    Compact,
    /// Unified diff of the draft against the recommendation.
    Diff,
    /// The whole recommendation.
    Full,
}

impl std::str::FromStr for RenderMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "compact" | "short" => Ok(RenderMode::Compact),
            "diff" | "patch" => Ok(RenderMode::Diff),
            "full" | "all" => Ok(RenderMode::Full),
            other => Err(format!("unknown render mode: {other} (expected compact, diff or full)")),
        }
    }
}

impl std::fmt::Display for RenderMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RenderMode::Compact => "compact",
            RenderMode::Diff => "diff",
            RenderMode::Full => "full",
        })
    }
}

/// Renders a layout as plain text in `mode`; `draft` is the agent's code for
/// [`RenderMode::Diff`] (without it the diff is against an empty draft).
pub fn render_mode(lay: &Layout, mode: RenderMode, draft: Option<&str>) -> String {
    match mode {
        RenderMode::Full => lay.code(),
        RenderMode::Diff => unified_diff(draft.unwrap_or(""), &lay.code_without_placeholders(), "draft", "recommendation"),
        RenderMode::Compact => compact(lay),
    }
}

fn compact(lay: &Layout) -> String {
    let n = lay.lines.len();
    let is_extra = |i: usize| lay.lines[i].extra && !lay.lines[i].placeholder;
    let show: Vec<bool> = (0..n)
        .map(|i| is_extra(i) || (i > 0 && is_extra(i - 1)) || (i + 1 < n && is_extra(i + 1)))
        .collect();
    if !show.iter().any(|&s| s) {
        return String::new();
    }
    let placeholder = lay.lang.parse::<sillage_core::Lang>().map_or("// ...", |l| l.placeholder());
    let mut out: Vec<String> = Vec::new();
    let mut skipped = false;
    for (i, l) in lay.lines.iter().enumerate() {
        if show[i] && !l.placeholder {
            if skipped && !out.is_empty() {
                let indent = &l.text[..l.text.len() - l.text.trim_start().len()];
                out.push(format!("{indent}{placeholder}"));
            }
            skipped = false;
            out.push(l.text.clone());
        } else {
            skipped = true;
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests;
