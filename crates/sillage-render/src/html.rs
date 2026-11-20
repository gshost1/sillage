//! Self-contained HTML snippet rendering.

use crate::layout::Layout;

/// Escapes `&`, `<`, `>`, `"` and `'` for HTML text and attribute contexts.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn render(layout: &Layout) -> String {
    let mut out = format!(
        "<pre class=\"sillage-rec\" data-lang=\"{}\">",
        escape(&layout.lang)
    );
    for (i, line) in layout.lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let class = match (line.extra, line.placeholder) {
            (_, true) => "line extra placeholder",
            (true, false) => "line extra",
            (false, false) => "line matched",
        };
        out.push_str("<span class=\"");
        out.push_str(class);
        out.push_str("\">");
        out.push_str(&escape(&line.text));
        out.push_str("</span>");
    }
    out.push_str("</pre>\n");
    out
}
