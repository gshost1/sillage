//! ANSI terminal rendering: recommended lines in bold green (placeholders in
//! cyan) with a `+` gutter, query-matched lines unstyled.

use crate::layout::Layout;

pub const EXTRA: &str = "\x1b[1;32m";
pub const PLACEHOLDER: &str = "\x1b[36m";
pub const RESET: &str = "\x1b[0m";

pub fn render(layout: &Layout) -> String {
    let mut out = String::new();
    for line in &layout.lines {
        if line.placeholder {
            out.push_str(PLACEHOLDER);
            out.push_str("+ ");
            out.push_str(&line.text);
            out.push_str(RESET);
        } else if line.extra {
            out.push_str(EXTRA);
            out.push_str("+ ");
            out.push_str(&line.text);
            out.push_str(RESET);
        } else {
            out.push_str("  ");
            out.push_str(&line.text);
        }
        out.push('\n');
    }
    out
}
