//! ANSI terminal rendering: recommended lines in bold green (placeholders in
//! cyan) with a `+` gutter, query-matched lines unstyled. Once
//! [`Layout::locate`] has set file line numbers, each line is prefixed with
//! its file line (blank for placeholders).
//!
//! Corpus text is untrusted: control characters (ESC, CR, BEL, C1 controls, …)
//! are printed as visible `\x..` escapes so a method body cannot inject
//! terminal escape sequences.

use std::fmt::Write;

use crate::layout::Layout;

pub const EXTRA: &str = "\x1b[1;32m";
pub const PLACEHOLDER: &str = "\x1b[36m";
pub const RESET: &str = "\x1b[0m";

/// `text` with every control character except tab replaced by a visible escape
/// (`\x1b`, `\x7f`, `\u{9b}`).
pub fn sanitize(text: &str) -> std::borrow::Cow<'_, str> {
    let is_bad = |c: char| c.is_control() && c != '\t';
    if !text.chars().any(is_bad) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if !is_bad(c) {
            out.push(c);
        } else if (c as u32) < 0x80 {
            let _ = write!(out, "\\x{:02x}", c as u32);
        } else {
            let _ = write!(out, "\\u{{{:x}}}", c as u32);
        }
    }
    std::borrow::Cow::Owned(out)
}

pub fn render(layout: &Layout) -> String {
    let width = layout.lines.iter().filter_map(|l| l.file_line).max().map(|n| n.to_string().len());
    let mut out = String::new();
    for line in &layout.lines {
        if let Some(w) = width {
            match line.file_line {
                Some(n) => {
                    let _ = write!(out, "{n:>w$} ");
                }
                None => out.push_str(&" ".repeat(w + 1)),
            }
        }
        let text = sanitize(&line.text);
        if line.placeholder {
            out.push_str(PLACEHOLDER);
            out.push_str("+ ");
            out.push_str(&text);
            out.push_str(RESET);
        } else if line.extra {
            out.push_str(EXTRA);
            out.push_str("+ ");
            out.push_str(&text);
            out.push_str(RESET);
        } else {
            out.push_str("  ");
            out.push_str(&text);
        }
        out.push('\n');
    }
    out
}
