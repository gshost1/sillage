//! Source-preserving layout of a (pruned / intersected) [`SimplifiedTree`].
//!
//! Kept terminals (`Keyword` and `Token` nodes) are copied verbatim from
//! `tree.source`. Whitespace between two kept terminals is preserved when no
//! source text was dropped between them; dropped whole lines collapse into a
//! single indented per-language placeholder line (`// ...` or `...`), and
//! inline drops collapse to at most one space.

use serde::Serialize;
use sillage_core::{NodeKind, SimplifiedTree, Span};

/// One rendered output line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Line {
    /// 1-based output line number.
    pub line_no: usize,
    /// Rendered text (dedented, no trailing whitespace, no newline).
    pub text: String,
    /// `true` when the line carries recommended (non-query) tokens or is a placeholder.
    pub extra: bool,
    /// `true` when the line is a dropped-code placeholder.
    pub placeholder: bool,
    /// 1-based line in `tree.source` the line was copied from (`None` for placeholders).
    pub src_line: Option<usize>,
    /// 1-based line in the method's *file*, once [`Layout::locate`] knows where the
    /// method starts (`None` for placeholders and before `locate`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_line: Option<usize>,
}

/// Line-diff summary of the query against the rendered recommendation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffSummary {
    /// Rendered lines not present in the query (recommended additions).
    pub inserted: usize,
    /// Query lines not present in the rendering.
    pub deleted: usize,
    /// Lines shared by the query and the rendering.
    pub unchanged: usize,
    /// Unified diff (`query` -> `recommendation`), compared on trimmed lines.
    pub unified: String,
}

/// Complete layout of a rendered snippet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Layout {
    pub lang: String,
    pub lines: Vec<Line>,
    pub diff: Option<DiffSummary>,
}

impl Layout {
    /// The rendered code, lines joined with `\n` (no trailing newline).
    pub fn code(&self) -> String {
        let mut out = String::new();
        for (i, l) in self.lines.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&l.text);
        }
        out
    }

    /// The rendered code without dropped-code placeholder lines.
    pub fn code_without_placeholders(&self) -> String {
        self.lines.iter().filter(|l| !l.placeholder).map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    }

    /// The extra (recommended) code lines, excluding placeholders, joined with `\n`.
    pub fn extra_code(&self) -> String {
        self.lines.iter().filter(|l| l.extra && !l.placeholder).map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    }

    /// Number of extra (recommended) code lines, excluding placeholders.
    pub fn extra_code_lines(&self) -> usize {
        self.lines.iter().filter(|l| l.extra && !l.placeholder).count()
    }

    /// Sets each line's [`Line::file_line`], given the 1-based file line on which
    /// `tree.source` (the method text) starts, e.g. `MethodRecord::line_start`.
    pub fn locate(&mut self, source_first_line: usize) {
        for l in &mut self.lines {
            l.file_line = l.src_line.map(|s| s + source_first_line.max(1) - 1);
        }
    }

    /// Keeps the first `max_lines` lines (at least one) and, when lines were cut,
    /// appends a placeholder line `// ... (N more lines)` (`... (N more lines)`
    /// for Python), indented like the last kept line.
    pub fn truncate(&mut self, max_lines: usize) {
        let max_lines = max_lines.max(1);
        if self.lines.len() <= max_lines {
            return;
        }
        let cut = self.lines.len() - max_lines;
        self.lines.truncate(max_lines);
        let indent = self.lines.last().map(|l| leading_ws(&l.text).to_string()).unwrap_or_default();
        let placeholder = self.lang.parse::<sillage_core::Lang>().map_or("// ...", |l| l.placeholder());
        let noun = if cut == 1 { "line" } else { "lines" };
        self.lines.push(Line {
            line_no: max_lines + 1,
            text: format!("{indent}{placeholder} ({cut} more {noun})"),
            extra: true,
            placeholder: true,
            src_line: None,
            file_line: None,
        });
    }
}

#[derive(Debug, Clone, Copy)]
struct Term {
    span: Span,
    /// Index into `tree.leaves` for `Token` terminals.
    leaf: Option<usize>,
    /// `None` for keywords, `Some(matched)` for non-keyword tokens.
    matched: Option<bool>,
    /// Keyword whose parent node holds no query-matched token (e.g. the closing
    /// `}` of a recommended block).
    kw_extra: bool,
}

#[derive(Debug, Default, Clone)]
struct LineBuf {
    text: String,
    matched_tokens: usize,
    extra_tokens: usize,
    extra_kws: usize,
    plain_kws: usize,
    placeholder: bool,
    src_line: Option<usize>,
}

impl LineBuf {
    fn new(src_line: Option<usize>) -> Self {
        Self {
            src_line,
            ..Self::default()
        }
    }
}

/// Collects kept terminals in source order, tagging each `Token` with its
/// `query_match` bit (missing bits count as `false`, i.e. recommended).
fn collect_terms(tree: &SimplifiedTree, query_match: &[bool]) -> Vec<Term> {
    let src_len = tree.source.len() as u32;
    let mut leaf_pos = vec![usize::MAX; tree.nodes.len()];
    for (i, &leaf) in tree.leaves.iter().enumerate() {
        if let Some(slot) = leaf_pos.get_mut(leaf as usize) {
            *slot = i;
        }
    }
    // A recommendation's root is the method body block: render its statements
    // without the enclosing `{` / `}` (paper Listings 5-7 show bodies that way).
    let mut skip = [usize::MAX; 2];
    if let Some(root) = tree.nodes.first()
        && root.kind.is_internal()
        && root.children.len() >= 2
    {
        let (first, last) = (root.children[0] as usize, *root.children.last().expect("children") as usize);
        let is = |id: usize, s: &str| matches!(&tree.nodes[id].kind, NodeKind::Keyword { text } if &**text == s);
        if is(first, "{") && is(last, "}") {
            skip = [first, last];
        }
    }
    // has_match[n]: the subtree of `n` holds a query-matched token. Nodes are in
    // preorder, so a reverse sweep visits children before parents.
    let mut has_match = vec![false; tree.nodes.len()];
    for id in (0..tree.nodes.len()).rev() {
        let node = &tree.nodes[id];
        has_match[id] = match &node.kind {
            NodeKind::Token { .. } => {
                let li = leaf_pos[id];
                li != usize::MAX && query_match.get(li).copied().unwrap_or(false)
            }
            NodeKind::Keyword { .. } => false,
            NodeKind::Internal { .. } => node.children.iter().any(|&c| has_match[c as usize]),
        };
    }
    let mut terms = Vec::new();
    for (id, node) in tree.nodes.iter().enumerate() {
        if skip.contains(&id) {
            continue;
        }
        let kw_extra = node.kind.is_keyword() && node.parent.is_some_and(|p| !has_match[p as usize]);
        let matched = match &node.kind {
            NodeKind::Internal { .. } => continue,
            NodeKind::Keyword { .. } => None,
            NodeKind::Token { .. } => {
                let li = leaf_pos[id];
                Some(li != usize::MAX && query_match.get(li).copied().unwrap_or(false))
            }
        };
        let span = node.span;
        if span.is_empty() || span.end > src_len {
            continue;
        }
        let leaf = (node.kind.is_token() && leaf_pos[id] != usize::MAX).then_some(leaf_pos[id]);
        terms.push(Term { span, leaf, matched, kw_extra });
    }
    terms.sort_by_key(|t| (t.span.start, t.span.end));
    // Drop terminals overlapping an earlier one (defensive; spans should be disjoint).
    let mut out: Vec<Term> = Vec::with_capacity(terms.len());
    for t in terms {
        if let Some(last) = out.last() {
            if t.span.start < last.span.end {
                continue;
            }
        }
        out.push(t);
    }
    out
}

fn line_of(line_starts: &[usize], offset: usize) -> usize {
    match line_starts.binary_search(&offset) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    }
}

fn leading_ws(s: &str) -> &str {
    &s[..s.len() - s.trim_start().len()]
}

fn is_wordish(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '$' | '"' | '\'' | '`')
}

/// Decides whether an inline drop between `prev` and `next` needs a separating space.
fn needs_space(prev: Option<char>, next: Option<char>, gap: &str) -> bool {
    let (Some(p), Some(n)) = (prev, next) else {
        return false;
    };
    if p.is_whitespace() {
        return false;
    }
    if matches!(p, '(' | '[' | '.') || matches!(n, ')' | ']' | ',' | ';' | '.' | ':') {
        return false;
    }
    if is_wordish(p) && is_wordish(n) {
        return true;
    }
    gap.starts_with(char::is_whitespace) || gap.ends_with(char::is_whitespace)
}

/// Whether `gap` holds anything besides whitespace and comments (`//`, `/* */`,
/// or `#` for Python).
fn gap_has_code(gap: &str, lang: sillage_core::Lang) -> bool {
    let b = gap.as_bytes();
    let py = lang == sillage_core::Lang::Python;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if py && c == b'#' || !py && c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if !py && c == b'/' && b.get(i + 1) == Some(&b'*') {
            match gap[i + 2..].find("*/") {
                Some(k) => i += k + 4,
                None => return true,
            }
        } else {
            return true;
        }
    }
    false
}

/// Lays out `tree` against `query_match` (aligned with `tree.leaves`).
pub fn layout(tree: &SimplifiedTree, query_match: &[bool], query: Option<&str>) -> Layout {
    layout_with(tree, query_match, query, &[])
}

/// [`layout`] with per-leaf replacement text: `overrides[i] = Some(text)` renders
/// leaf `tree.leaves[i]` as `text` instead of its source span (variable
/// renaming by token span, so strings, comments, keywords and member names are
/// never touched). Missing entries render the source text.
pub fn layout_with(tree: &SimplifiedTree, query_match: &[bool], query: Option<&str>, overrides: &[Option<String>]) -> Layout {
    let src: &str = &tree.source;
    let terms = collect_terms(tree, query_match);
    let placeholder = tree.lang.placeholder();

    let mut line_starts = vec![0usize];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }

    let mut lines: Vec<LineBuf> = Vec::new();
    let mut prev_end: Option<usize> = None;

    for term in &terms {
        let start = term.span.start as usize;
        let end = term.span.end as usize;
        if !src.is_char_boundary(start) || !src.is_char_boundary(end) {
            continue;
        }
        match prev_end {
            None => {
                // First terminal: keep its line's indentation.
                let ls = line_starts[line_of(&line_starts, start)];
                let prefix = &src[ls..start];
                let mut buf = LineBuf::new(Some(line_of(&line_starts, start) + 1));
                buf.text.push_str(leading_ws(prefix));
                lines.push(buf);
            }
            Some(pe) => {
                let gap = &src[pe..start];
                // Comments are not tree nodes: a gap holding only comments and
                // whitespace dropped no code, so it is copied verbatim.
                let dropped = gap_has_code(gap, tree.lang);
                if !dropped {
                    // Pure whitespace: copy verbatim, splitting on newlines.
                    let mut parts = gap.split('\n');
                    if let Some(first) = parts.next() {
                        lines.last_mut().expect("line").text.push_str(first);
                    }
                    let mut src_line = line_of(&line_starts, pe);
                    for part in parts {
                        src_line += 1;
                        let mut buf = LineBuf::new(Some(src_line + 1));
                        buf.text.push_str(part.trim_end_matches('\r'));
                        lines.push(buf);
                    }
                } else if !gap.contains('\n') {
                    // Inline drop.
                    let cur = lines.last_mut().expect("line");
                    let prev_c = cur.text.chars().last();
                    let next_c = src[start..].chars().next();
                    if needs_space(prev_c, next_c, gap) {
                        cur.text.push(' ');
                    }
                } else {
                    let first_nl = gap.find('\n').expect("newline");
                    let last_nl = gap.rfind('\n').expect("newline");
                    let middle = if last_nl > first_nl {
                        &gap[first_nl + 1..last_nl]
                    } else {
                        ""
                    };
                    let tail = &gap[last_nl + 1..];
                    if let Some(dropped_line) = middle.split('\n').find(|l| !l.trim().is_empty()) {
                        let mut ph = LineBuf::new(None);
                        ph.text.push_str(leading_ws(dropped_line));
                        ph.text.push_str(placeholder);
                        ph.placeholder = true;
                        lines.push(ph);
                    }
                    let mut buf = LineBuf::new(Some(line_of(&line_starts, start) + 1));
                    buf.text.push_str(leading_ws(tail));
                    lines.push(buf);
                }
            }
        }

        // Copy the terminal itself (may span lines, e.g. block strings).
        let text = term.leaf.and_then(|l| overrides.get(l)).and_then(Option::as_deref).unwrap_or(&src[start..end]);
        let mut src_line = line_of(&line_starts, start);
        for (i, part) in text.split('\n').enumerate() {
            if i > 0 {
                src_line += 1;
                lines.push(LineBuf::new(Some(src_line + 1)));
            }
            let cur = lines.last_mut().expect("line");
            cur.text.push_str(part.trim_end_matches('\r'));
            match term.matched {
                Some(true) => cur.matched_tokens += 1,
                Some(false) => cur.extra_tokens += 1,
                None if term.kw_extra => cur.extra_kws += 1,
                None => cur.plain_kws += 1,
            }
        }
        prev_end = Some(end);
    }

    // Trim trailing whitespace and drop empty lines at the edges.
    for l in &mut lines {
        let t = l.text.trim_end().len();
        l.text.truncate(t);
    }
    while lines.last().is_some_and(|l| l.text.is_empty()) {
        lines.pop();
    }
    let first_nonempty = lines.iter().position(|l| !l.text.is_empty()).unwrap_or(lines.len());
    lines.drain(..first_nonempty);

    dedent(&mut lines);

    let lines: Vec<Line> = lines
        .into_iter()
        .enumerate()
        .map(|(i, l)| Line {
            line_no: i + 1,
            extra: l.placeholder
                || l.extra_tokens > 0
                || (l.matched_tokens == 0 && l.extra_kws > l.plain_kws),
            placeholder: l.placeholder,
            src_line: if l.placeholder { None } else { l.src_line },
            file_line: None,
            text: l.text,
        })
        .collect();

    let mut out = Layout {
        lang: tree.lang.as_str().to_string(),
        lines,
        diff: None,
    };
    if let Some(q) = query {
        out.diff = Some(diff_summary(q, &out.code()));
    }
    out
}

/// Removes the longest whitespace prefix shared by every non-empty line.
fn dedent(lines: &mut [LineBuf]) {
    let mut common: Option<String> = None;
    for l in lines.iter().filter(|l| !l.text.is_empty()) {
        let ws = leading_ws(&l.text);
        common = Some(match common {
            None => ws.to_string(),
            Some(c) => c
                .chars()
                .zip(ws.chars())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| a)
                .collect(),
        });
    }
    let Some(prefix) = common.filter(|c| !c.is_empty()) else {
        return;
    };
    for l in lines.iter_mut() {
        if l.text.starts_with(&prefix) {
            l.text.drain(..prefix.len());
        }
    }
}

fn normalize_lines(s: &str) -> String {
    let mut out = String::new();
    for line in s.lines().map(str::trim).filter(|l| !l.is_empty()) {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Line diff of `query` against `code`, ignoring indentation and blank lines.
pub fn diff_summary(query: &str, code: &str) -> DiffSummary {
    use similar::{ChangeTag, TextDiff};
    let old = normalize_lines(query);
    let new = normalize_lines(code);
    let diff = TextDiff::from_lines(&old, &new);
    let (mut inserted, mut deleted, mut unchanged) = (0, 0, 0);
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => inserted += 1,
            ChangeTag::Delete => deleted += 1,
            ChangeTag::Equal => unchanged += 1,
        }
    }
    let unified = diff
        .unified_diff()
        .context_radius(3)
        .header("query", "recommendation")
        .to_string();
    DiffSummary {
        inserted,
        deleted,
        unchanged,
        unified,
    }
}

/// A line's comparison key: trimmed, without a trailing `//` comment or a
/// ` #` comment (outside string / char literals).
fn diff_key(line: &str) -> &str {
    let t = line.trim();
    let b = t.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(q) => {
                if c == b'\\' {
                    i += 1;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                b'"' | b'`' => quote = Some(c),
                // A quote only when it closes later on the line (Rust lifetimes don't).
                b'\'' if b[i + 1..].contains(&b'\'') => quote = Some(c),
                b'/' if b.get(i + 1) == Some(&b'/') && i > 0 => return t[..i].trim_end(),
                b'#' if i > 0 && b[i - 1].is_ascii_whitespace() => return t[..i].trim_end(),
                _ => {}
            },
        }
        i += 1;
    }
    t
}

/// Unified diff of `old` (e.g. the agent's draft) against `new` (e.g. a
/// recommendation), with `---` / `+++` headers naming them. Both sides are
/// dedented and lines are compared ignoring indentation, trailing whitespace
/// and trailing comments, so re-indented or annotated code is not reported as
/// changed; output lines keep their own side's text (equal lines show `old`).
/// Blank lines are ignored. Returns an empty string when the two sides match.
pub fn unified_diff(old: &str, new: &str, old_name: &str, new_name: &str) -> String {
    use similar::{DiffOp, capture_diff_slices, group_diff_ops};
    let prep = |s: &str| -> Vec<String> {
        let lines: Vec<&str> = s.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
        let common = lines.iter().map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
        lines.iter().map(|l| l[common.min(l.len() - l.trim_start().len())..].to_string()).collect()
    };
    let (a, b) = (prep(old), prep(new));
    let ka: Vec<&str> = a.iter().map(|l| diff_key(l)).collect();
    let kb: Vec<&str> = b.iter().map(|l| diff_key(l)).collect();
    let ops = capture_diff_slices(similar::Algorithm::Myers, &ka, &kb);
    if ops.iter().all(|o| matches!(o, DiffOp::Equal { .. })) {
        return String::new();
    }
    let mut out = format!("--- {old_name}\n+++ {new_name}\n");
    for group in group_diff_ops(ops, 3) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else { continue };
        let (o0, n0) = (first.old_range().start, first.new_range().start);
        let (o1, n1) = (last.old_range().end, last.new_range().end);
        let range = |start: usize, len: usize| if len == 0 { format!("{start},0") } else if len == 1 { format!("{}", start + 1) } else { format!("{},{len}", start + 1) };
        out.push_str(&format!("@@ -{} +{} @@\n", range(o0, o1 - o0), range(n0, n1 - n0)));
        for op in &group {
            match *op {
                DiffOp::Equal { old_index, len, .. } => {
                    for l in &a[old_index..old_index + len] {
                        out.push_str(&format!(" {l}\n"));
                    }
                }
                DiffOp::Delete { old_index, old_len, .. } => {
                    for l in &a[old_index..old_index + old_len] {
                        out.push_str(&format!("-{l}\n"));
                    }
                }
                DiffOp::Insert { new_index, new_len, .. } => {
                    for l in &b[new_index..new_index + new_len] {
                        out.push_str(&format!("+{l}\n"));
                    }
                }
                DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                    for l in &a[old_index..old_index + old_len] {
                        out.push_str(&format!("-{l}\n"));
                    }
                    for l in &b[new_index..new_index + new_len] {
                        out.push_str(&format!("+{l}\n"));
                    }
                }
            }
        }
    }
    out
}
