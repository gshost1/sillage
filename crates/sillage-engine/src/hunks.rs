//! Unified-diff (`git diff`) parsing into per-file hunks, for reviewing added
//! code (missing guards) and sweeping removed code (bug-fix clones).

use serde::{Deserialize, Serialize};
use sillage_core::Lang;

/// One line of a hunk with its 1-based line number on its side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub line: u32,
    pub text: String,
}

/// One `@@` hunk of a unified diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    /// Old path without the `a/` prefix (`None` for a new file).
    pub old_path: Option<String>,
    /// New path without the `b/` prefix (`None` for a deleted file).
    pub new_path: Option<String>,
    /// `new_path`, else `old_path`.
    pub path: String,
    /// Language guessed from the path's extension.
    pub lang: Option<Lang>,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// Text after the closing `@@` (often the enclosing function's signature).
    pub section: String,
    /// The old side: context and removed lines, in order.
    pub before_code: String,
    /// The new side: context and added lines, in order.
    pub after_code: String,
    pub added: Vec<DiffLine>,
    pub removed: Vec<DiffLine>,
    pub is_new_file: bool,
    pub is_deleted_file: bool,
}

impl DiffHunk {
    /// Only the added lines, joined with `\n`.
    pub fn added_code(&self) -> String {
        self.added.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    }

    /// Only the removed lines, joined with `\n`.
    pub fn removed_code(&self) -> String {
        self.removed.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    }
}

#[derive(Default)]
struct FileState {
    old_path: Option<String>,
    new_path: Option<String>,
    is_new: bool,
    is_deleted: bool,
}

struct Open {
    hunk: DiffHunk,
    old_left: u32,
    new_left: u32,
    old_no: u32,
    new_no: u32,
    before: Vec<String>,
    after: Vec<String>,
}

impl Open {
    fn finish(mut self) -> DiffHunk {
        self.hunk.before_code = self.before.join("\n");
        self.hunk.after_code = self.after.join("\n");
        self.hunk
    }
}

/// Parses a unified diff (`git diff`, `diff -u`) into hunks. Unparseable lines
/// are ignored; a truncated last hunk keeps the lines seen.
pub fn extract_diff_hunks(unified_diff: &str) -> Vec<DiffHunk> {
    let mut out = Vec::new();
    let mut file = FileState::default();
    let mut open: Option<Open> = None;
    for line in unified_diff.lines() {
        if let Some(h) = open.as_mut() {
            if h.old_left > 0 || h.new_left > 0 {
                // Dispatch on the ASCII tag byte: slicing at byte 1 is then
                // always a char boundary, whatever the line's content.
                let text = line.get(1..).unwrap_or("");
                match line.as_bytes().first() {
                    Some(b' ') | None => {
                        h.before.push(text.to_string());
                        h.after.push(text.to_string());
                        h.old_no = h.old_no.saturating_add(1);
                        h.new_no = h.new_no.saturating_add(1);
                        h.old_left = h.old_left.saturating_sub(1);
                        h.new_left = h.new_left.saturating_sub(1);
                        continue;
                    }
                    Some(b'-') => {
                        h.hunk.removed.push(DiffLine { line: h.old_no, text: text.to_string() });
                        h.before.push(text.to_string());
                        h.old_no = h.old_no.saturating_add(1);
                        h.old_left = h.old_left.saturating_sub(1);
                        continue;
                    }
                    Some(b'+') => {
                        h.hunk.added.push(DiffLine { line: h.new_no, text: text.to_string() });
                        h.after.push(text.to_string());
                        h.new_no = h.new_no.saturating_add(1);
                        h.new_left = h.new_left.saturating_sub(1);
                        continue;
                    }
                    Some(b'\\') => continue, // "\ No newline at end of file"
                    _ => {}
                }
            } else if line.starts_with('\\') {
                continue;
            }
            out.push(open.take().expect("open hunk").finish());
        }

        if let Some(rest) = line.strip_prefix("diff --git ") {
            file = FileState::default();
            if let Some((a, b)) = split_git_paths(rest) {
                file.old_path = Some(a);
                file.new_path = Some(b);
            }
        } else if line.starts_with("new file mode") {
            file.is_new = true;
        } else if line.starts_with("deleted file mode") {
            file.is_deleted = true;
        } else if let Some(p) = line.strip_prefix("rename from ") {
            file.old_path = Some(p.to_string());
        } else if let Some(p) = line.strip_prefix("rename to ") {
            file.new_path = Some(p.to_string());
        } else if let Some(p) = line.strip_prefix("--- ") {
            let p = header_path(p, "a/");
            file.is_new |= p.is_none();
            file.old_path = p;
        } else if let Some(p) = line.strip_prefix("+++ ") {
            let p = header_path(p, "b/");
            file.is_deleted |= p.is_none();
            file.new_path = p;
        } else if line.starts_with("@@")
            && let Some((os, ol, ns, nl, section)) = parse_range(line)
        {
            let path = file.new_path.clone().or_else(|| file.old_path.clone()).unwrap_or_default();
            let lang = sillage_parse::lang_from_path(std::path::Path::new(&path));
            open = Some(Open {
                hunk: DiffHunk {
                    old_path: file.old_path.clone(),
                    new_path: file.new_path.clone(),
                    path,
                    lang,
                    old_start: os,
                    old_lines: ol,
                    new_start: ns,
                    new_lines: nl,
                    section,
                    before_code: String::new(),
                    after_code: String::new(),
                    added: Vec::new(),
                    removed: Vec::new(),
                    is_new_file: file.is_new,
                    is_deleted_file: file.is_deleted,
                },
                old_left: ol,
                new_left: nl,
                old_no: os.max(1),
                new_no: ns.max(1),
                before: Vec::new(),
                after: Vec::new(),
            });
        }
    }
    if let Some(h) = open {
        out.push(h.finish());
    }
    out
}

/// `a/x b/y` from a `diff --git` header (paths without spaces, or quoted).
fn split_git_paths(rest: &str) -> Option<(String, String)> {
    let rest = rest.trim();
    if rest.starts_with('"') {
        let mut parts = rest.split('"').filter(|s| !s.trim().is_empty());
        let a = parts.next()?;
        let b = parts.next()?;
        return Some((strip(a, "a/"), strip(b, "b/")));
    }
    // `a/<p> b/<p>`: split at the " b/" that halves the string when possible.
    if let Some(i) = rest.find(" b/") {
        return Some((strip(&rest[..i], "a/"), strip(&rest[i + 1..], "b/")));
    }
    let (a, b) = rest.split_once(' ')?;
    Some((strip(a, "a/"), strip(b, "b/")))
}

fn strip(p: &str, prefix: &str) -> String {
    p.strip_prefix(prefix).unwrap_or(p).to_string()
}

/// A `---` / `+++` header path: `None` for `/dev/null`; drops a timestamp
/// suffix (`\t2024-…`) and the `a/` / `b/` prefix.
fn header_path(p: &str, prefix: &str) -> Option<String> {
    let p = p.split('\t').next().unwrap_or(p).trim();
    let p = p.trim_matches('"');
    if p == "/dev/null" {
        return None;
    }
    Some(strip(p, prefix))
}

/// `@@ -a[,b] +c[,d] @@ section`.
fn parse_range(line: &str) -> Option<(u32, u32, u32, u32, String)> {
    let rest = line.strip_prefix("@@")?.trim_start();
    let end = rest.find("@@")?;
    let (ranges, section) = (&rest[..end], rest[end + 2..].trim());
    let mut it = ranges.split_whitespace();
    let old = it.next()?.strip_prefix('-')?;
    let new = it.next()?.strip_prefix('+')?;
    let pair = |s: &str| -> Option<(u32, u32)> {
        match s.split_once(',') {
            Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    };
    let (os, ol) = pair(old)?;
    let (ns, nl) = pair(new)?;
    Some((os, ol, ns, nl, section.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "diff --git a/pkg/server.go b/pkg/server.go
index 83db48f..bf269f4 100644
--- a/pkg/server.go
+++ b/pkg/server.go
@@ -10,6 +10,9 @@ func (s *Server) Load(path string) error {
 \tf, err := os.Open(path)
-\tdata, _ := io.ReadAll(f)
+\tif err != nil {
+\t\treturn err
+\t}
+\tdata, err := io.ReadAll(f)
 \ts.data = data
 \treturn nil
 }
@@ -40,2 +43,2 @@
-\told()
+\tnew()
 }
diff --git a/README.md b/README.md
new file mode 100644
--- /dev/null
+++ b/README.md
@@ -0,0 +1,2 @@
+# Title
+text
\\ No newline at end of file
diff --git a/src/gone.rs b/src/gone.rs
deleted file mode 100644
--- a/src/gone.rs
+++ /dev/null
@@ -1 +0,0 @@
-fn gone() {}
";

    #[test]
    fn parses_git_diff_hunks() {
        let h = extract_diff_hunks(DIFF);
        assert_eq!(h.len(), 4);

        let a = &h[0];
        assert_eq!((a.path.as_str(), a.lang), ("pkg/server.go", Some(Lang::Go)));
        assert_eq!((a.old_start, a.old_lines, a.new_start, a.new_lines), (10, 6, 10, 9));
        assert_eq!(a.section, "func (s *Server) Load(path string) error {");
        assert_eq!(a.added.iter().map(|l| l.line).collect::<Vec<_>>(), [11, 12, 13, 14]);
        assert_eq!(a.removed, [DiffLine { line: 11, text: "\tdata, _ := io.ReadAll(f)".into() }]);
        assert_eq!(
            a.after_code,
            "\tf, err := os.Open(path)\n\tif err != nil {\n\t\treturn err\n\t}\n\tdata, err := io.ReadAll(f)\n\ts.data = data\n\treturn nil\n}"
        );
        assert_eq!(a.before_code, "\tf, err := os.Open(path)\n\tdata, _ := io.ReadAll(f)\n\ts.data = data\n\treturn nil\n}");
        assert_eq!(a.added_code().lines().count(), 4);
        assert!(!a.is_new_file && !a.is_deleted_file);

        let b = &h[1];
        assert_eq!((b.old_start, b.new_start, b.section.as_str()), (40, 43, ""));
        assert_eq!(b.removed_code(), "\told()");
        assert_eq!(b.added[0], DiffLine { line: 43, text: "\tnew()".into() });

        let c = &h[2];
        assert!(c.is_new_file && c.old_path.is_none());
        assert_eq!((c.path.as_str(), c.lang), ("README.md", None));
        assert_eq!(c.after_code, "# Title\ntext");

        let d = &h[3];
        assert!(d.is_deleted_file && d.new_path.is_none());
        assert_eq!((d.path.as_str(), d.lang), ("src/gone.rs", Some(Lang::Rust)));
        assert_eq!(d.before_code, "fn gone() {}");
        assert!(d.after_code.is_empty());
    }

    #[test]
    fn plain_unified_diff_and_edge_cases() {
        let plain = "--- a.py\t2024-01-01 00:00:00\n+++ a.py\t2024-01-02 00:00:00\n@@ -1,2 +1,3 @@\n def f(x):\n+    if x is None:\n+        return\n     return x.y\n";
        let h = extract_diff_hunks(plain);
        assert_eq!(h.len(), 1);
        assert_eq!((h[0].path.as_str(), h[0].lang), ("a.py", Some(Lang::Python)));
        assert_eq!(h[0].after_code, "def f(x):\n    if x is None:\n        return\n    return x.y");
        // CRLF, a context line stripped to empty, and a truncated hunk.
        let crlf = "--- a/x.rs\r\n+++ b/x.rs\r\n@@ -1,3 +1,4 @@\r\n fn f() {\r\n\r\n+    g();\r\n";
        let h = extract_diff_hunks(crlf);
        assert_eq!(h[0].after_code, "fn f() {\n\n    g();");
        assert!(extract_diff_hunks("").is_empty());
        assert!(extract_diff_hunks("not a diff\n@@ nonsense @@\n").is_empty());
        assert_eq!(split_git_paths("\"a/with space.rs\" \"b/with space.rs\""), Some(("with space.rs".into(), "with space.rs".into())));
    }

    #[test]
    fn non_ascii_lines_inside_a_hunk_never_panic() {
        // A line starting with a multi-byte char (a mangled diff) ends the hunk.
        let d = "--- a/x.py\n+++ b/x.py\n@@ -1,3 +1,3 @@\n+é = 1\nélan = 2\n-ü\n 日本\n";
        let h = extract_diff_hunks(d);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].added, [DiffLine { line: 1, text: "é = 1".into() }]);
        let d = "--- a/x.py\n+++ b/x.py\n@@ -1,2 +1,2 @@\n-ü = 1\n+日本 = 2\n 😀\n";
        let h = extract_diff_hunks(d);
        assert_eq!((h[0].removed[0].text.as_str(), h[0].added[0].text.as_str()), ("ü = 1", "日本 = 2"));
        assert_eq!(h[0].after_code, "日本 = 2\n😀");
    }

    #[test]
    fn line_numbers_saturate_at_u32_max() {
        let d = "--- a/x.rs\n+++ b/x.rs\n@@ -4294967295,3 +4294967295,3 @@\n a\n-b\n+c\n d\n";
        let h = extract_diff_hunks(d);
        assert_eq!(h[0].removed[0].line, u32::MAX);
        assert_eq!(h[0].added[0].line, u32::MAX);
        let d = "--- a/x.rs\n+++ b/x.rs\n@@ -4294967295,1 +4294967295,1 @@\n-a\n+b\n";
        let h = extract_diff_hunks(d);
        assert_eq!((h[0].old_start, h[0].new_start), (u32::MAX, u32::MAX));
    }
}
