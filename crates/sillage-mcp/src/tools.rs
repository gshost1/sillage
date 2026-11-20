//! The six MCP tools: definitions (JSON Schemas) and token-disciplined
//! text renderings of the engine's agent analyses.
//!
//! Argument shape errors (missing required fields, wrong types, unknown enum
//! values) are JSON-RPC `-32602` errors; semantic problems the agent can fix
//! (unknown language or corpus, oversized input, neither `code` nor `diff`)
//! are tool results with `isError: true`. Engine work runs on the blocking
//! pool, never on the async runtime's worker threads.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::SystemTime;

use serde::Deserialize;
use serde_json::{Value, json};
use sillage_core::{Lang, MethodScope, Params};
use sillage_engine::{
    Citation, DiffHunk, GuardOptions, IdiomOptions, MissingGuard, QueryFilter, TestIdiomOptions, extract_diff_hunks,
};

use crate::protocol::{INTERNAL_ERROR, RpcError};
use crate::registry::{CorpusHandle, CorpusRegistry, SourceKind};

/// Largest `snippet` / `code` accepted (bytes).
pub const MAX_SNIPPET_BYTES: usize = 64 * 1024;
/// Largest `diff` accepted (bytes).
pub const MAX_DIFF_BYTES: usize = 1024 * 1024;
/// Diff hunks analyzed per `check_missing_guards` call.
pub const MAX_DIFF_HUNKS: usize = 64;
pub const MAX_EXCLUDE_PATHS: usize = 32;
pub const MAX_PATH_BYTES: usize = 1024;
pub const DEFAULT_LIMIT: usize = 5;
pub const MAX_LIMIT: usize = 20;
pub const DEFAULT_MAX_LINES: usize = 60;
pub const MIN_MAX_LINES: usize = 5;
pub const MAX_MAX_LINES: usize = 400;
/// Citations listed per result before `(+N more)`.
const MAX_CITES: usize = 5;
/// Lines shown per `find_similar_code` match by default.
const SIMILAR_LINES: usize = 30;
/// Lines shown per test in `find_test_idioms` by default.
const TEST_LINES: usize = 40;

/// A tool's text result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

impl ToolOutput {
    fn ok(text: String) -> Self {
        Self { text, is_error: false }
    }
    fn err(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: true }
    }
}

type ToolResult = Result<String, String>;

const LANGS: &str = "java, python, javascript, typescript, tsx, go, rust";

/// The `tools/list` definitions.
pub fn tool_definitions() -> Value {
    let corpus = json!({ "type": "string", "description": "Corpus name (default: the server's default corpus; see corpus_status)." });
    let lang = json!({ "type": "string", "enum": ["java", "python", "javascript", "typescript", "tsx", "go", "rust"], "description": "Language of the code." });
    let exclude = json!({ "type": "string", "description": "Repo-relative (or absolute) file to exclude, usually the file you are editing; a trailing `/` excludes a directory." });
    json!([
        {
            "name": "find_idioms",
            "title": "Find repo idioms for a draft",
            "description": "Call right after drafting a function body or snippet. Returns how this repo usually completes code like your draft: a consensus recommendation clustered from several repo methods, aligned to YOUR variable names (unified diff by default), with confidence, cluster size, file:line provenance and the imports the added lines need. Empty result = no consensus; keep your code.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "snippet": { "type": "string", "description": "Your draft: a partial or complete function / method body." },
                    "lang": lang,
                    "corpus": corpus,
                    "exclude_path": exclude,
                    "preset": { "type": "string", "enum": ["repo", "paper"], "default": "repo", "description": "repo: thresholds tuned for one codebase; paper: the Aroma paper's stricter defaults." },
                    "detail": { "type": "string", "enum": ["diff", "compact", "full"], "default": "diff", "description": "diff: unified diff draft→recommendation; compact: added lines with context; full: the whole recommendation." },
                    "max_lines": { "type": "integer", "minimum": MIN_MAX_LINES, "maximum": MAX_MAX_LINES, "default": DEFAULT_MAX_LINES, "description": "Max lines of code shown per idiom." }
                },
                "required": ["snippet", "lang"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "find_similar_code",
            "title": "Find structurally similar code",
            "description": "Structural code search: repo methods shaped like a snippet (not keyword matching), ranked by similarity, each with file:line, method name, prod/test badge and score. Use before writing a helper that may already exist, or to find implementations / call sites / tests of a pattern.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "snippet": { "type": "string", "description": "Code to match: a snippet or a whole function." },
                    "lang": lang,
                    "corpus": corpus,
                    "scope": { "type": "string", "enum": ["all", "prod", "test"], "default": "all", "description": "Search production code, test code, or both." },
                    "exclude_path": exclude,
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "default": DEFAULT_LIMIT, "description": "Matches to return." },
                    "max_lines": { "type": "integer", "minimum": MIN_MAX_LINES, "maximum": MAX_MAX_LINES, "default": SIMILAR_LINES, "description": "Max lines shown per match." }
                },
                "required": ["snippet", "lang"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "check_missing_guards",
            "title": "Check for missing guards",
            "description": "Review finished code or a raw `git diff` against the repo's consensus: reports ONLY high-confidence guards (error / null / bounds checks, cleanup, logging, validation) that most structurally similar production methods (3+ in 2+ files) have and yours lacks, with file:line citations. Silent when clean. Pass `code` + `lang`, or `diff` (language from file extensions; `lang` is the fallback).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code": { "type": "string", "description": "A finished function or snippet (requires lang)." },
                    "diff": { "type": "string", "description": "A unified diff (`git diff` output); added code of each hunk is checked." },
                    "lang": lang,
                    "corpus": corpus,
                    "exclude_path": exclude
                },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "find_test_idioms",
            "title": "Find repo test idioms",
            "description": "When writing tests for new code: returns this repo's tests to model them on (tests that call your function, tests of structurally similar production code, tests in their corresponding test files), as skeletons renamed to your function, with how each was linked, assertions, fixtures and relevant test imports.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code": { "type": "string", "description": "The code under test, ideally the complete function." },
                    "lang": lang,
                    "corpus": corpus,
                    "exclude_path": exclude,
                    "max_lines": { "type": "integer", "minimum": MIN_MAX_LINES, "maximum": MAX_MAX_LINES, "default": TEST_LINES, "description": "Max lines shown per test." }
                },
                "required": ["code", "lang"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "corpus_status",
            "title": "Corpus status",
            "description": "What is indexed: per corpus its kind (live directory or static snapshot), method counts (prod / test), files, languages and last refresh. Omit corpus for all loaded corpora.",
            "inputSchema": {
                "type": "object",
                "properties": { "corpus": corpus },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true, "openWorldHint": false }
        },
        {
            "name": "refresh_corpus",
            "title": "Refresh corpus",
            "description": "Incrementally re-index a directory corpus after files changed (only changed files are re-parsed), so new code becomes searchable. Omit corpus to refresh every directory corpus. Returns files added / modified / removed / unchanged, total methods and elapsed time.",
            "inputSchema": {
                "type": "object",
                "properties": { "corpus": corpus },
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false }
        }
    ])
}

/// One path or several.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Paths {
    One(String),
    Many(Vec<String>),
}

impl Paths {
    fn into_vec(self) -> Vec<String> {
        match self {
            Paths::One(p) => vec![p],
            Paths::Many(v) => v,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Preset {
    #[default]
    Repo,
    Paper,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Detail {
    #[default]
    Diff,
    Compact,
    Full,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Scope {
    #[default]
    All,
    Prod,
    Test,
}

#[derive(Debug, Deserialize)]
struct FindIdiomsArgs {
    snippet: String,
    lang: String,
    corpus: Option<String>,
    exclude_path: Option<Paths>,
    #[serde(default)]
    preset: Preset,
    #[serde(default)]
    detail: Detail,
    max_lines: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct FindSimilarArgs {
    snippet: String,
    lang: String,
    corpus: Option<String>,
    #[serde(default)]
    scope: Scope,
    exclude_path: Option<Paths>,
    limit: Option<u64>,
    max_lines: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GuardArgs {
    code: Option<String>,
    diff: Option<String>,
    lang: Option<String>,
    corpus: Option<String>,
    exclude_path: Option<Paths>,
}

#[derive(Debug, Deserialize)]
struct TestIdiomArgs {
    code: String,
    lang: String,
    corpus: Option<String>,
    exclude_path: Option<Paths>,
    max_lines: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct CorpusArgs {
    corpus: Option<String>,
}

fn parse_args<T: serde::de::DeserializeOwned>(tool: &str, args: Value) -> Result<T, RpcError> {
    serde_json::from_value(args).map_err(|e| RpcError::invalid_params(format!("{tool}: invalid arguments: {e}")))
}

/// Dispatches `tools/call`; unknown tools are `-32602`.
pub async fn call_tool(registry: &Arc<CorpusRegistry>, name: &str, args: Value) -> Result<ToolOutput, RpcError> {
    let reg = Arc::clone(registry);
    let job: Box<dyn FnOnce() -> ToolResult + Send> = match name {
        "find_idioms" => {
            let a: FindIdiomsArgs = parse_args(name, args)?;
            Box::new(move || find_idioms(&reg, a))
        }
        "find_similar_code" => {
            let a: FindSimilarArgs = parse_args(name, args)?;
            Box::new(move || find_similar(&reg, a))
        }
        "check_missing_guards" => {
            let a: GuardArgs = parse_args(name, args)?;
            Box::new(move || check_guards(&reg, a))
        }
        "find_test_idioms" => {
            let a: TestIdiomArgs = parse_args(name, args)?;
            Box::new(move || find_test_idioms(&reg, a))
        }
        "corpus_status" => {
            let a: CorpusArgs = parse_args(name, args)?;
            Box::new(move || corpus_status(&reg, a))
        }
        "refresh_corpus" => {
            let a: CorpusArgs = parse_args(name, args)?;
            Box::new(move || refresh_corpus(&reg, a))
        }
        other => return Err(RpcError::invalid_params(format!("unknown tool: {other}"))),
    };
    match tokio::task::spawn_blocking(job).await {
        Ok(Ok(text)) => Ok(ToolOutput::ok(text)),
        Ok(Err(text)) => Ok(ToolOutput::err(text)),
        Err(e) => Err(RpcError::new(INTERNAL_ERROR, format!("{name} failed: {e}"))),
    }
}

// ---------------------------------------------------------------------------
// Validation helpers

fn parse_lang(s: &str) -> Result<Lang, String> {
    s.parse::<Lang>().map_err(|_| format!("unsupported lang {s:?}; expected one of: {LANGS}"))
}

fn check_size(what: &str, s: &str, max: usize) -> Result<(), String> {
    if s.trim().is_empty() {
        return Err(format!("`{what}` is empty"));
    }
    if s.len() > max {
        return Err(format!("`{what}` is {} bytes; the limit is {max} bytes. Send a smaller excerpt.", s.len()));
    }
    Ok(())
}

fn exclude_paths(p: Option<Paths>) -> Result<Vec<String>, String> {
    let v: Vec<String> =
        p.map(Paths::into_vec).unwrap_or_default().into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if v.len() > MAX_EXCLUDE_PATHS {
        return Err(format!("too many exclude_path entries ({}; max {MAX_EXCLUDE_PATHS})", v.len()));
    }
    if v.iter().any(|s| s.len() > MAX_PATH_BYTES) {
        return Err(format!("exclude_path entries are limited to {MAX_PATH_BYTES} bytes"));
    }
    Ok(v)
}

fn clamp(v: Option<u64>, default: usize, min: usize, max: usize) -> usize {
    v.map_or(default, |v| usize::try_from(v).unwrap_or(max).clamp(min, max))
}

fn corpus(reg: &CorpusRegistry, name: Option<&str>) -> Result<Arc<CorpusHandle>, String> {
    reg.get(name)
}

// ---------------------------------------------------------------------------
// Rendering helpers

/// The first `max` lines of `text`, then `… (K more lines)`.
fn clip(text: &str, max: usize) -> String {
    let text = text.trim_end_matches('\n');
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max {
        return text.to_string();
    }
    let mut out = lines[..max].join("\n");
    let _ = write!(out, "\n… ({} more lines)", lines.len() - max);
    out
}

/// A fenced code block whose fence cannot be closed by the content.
fn fenced(info: &str, body: &str) -> String {
    let mut fence = "```".to_string();
    while body.contains(fence.as_str()) {
        fence.push('`');
    }
    format!("{fence}{info}\n{body}\n{fence}\n")
}

fn badge(is_test: bool) -> &'static str {
    if is_test { "test" } else { "prod" }
}

fn cite(c: &Citation) -> String {
    format!("{} {}", c.provenance, c.name)
}

fn cites(cs: &[Citation]) -> String {
    let mut s = cs.iter().take(MAX_CITES).map(cite).collect::<Vec<_>>().join(", ");
    if cs.len() > MAX_CITES {
        let _ = write!(s, " (+{} more)", cs.len() - MAX_CITES);
    }
    s
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn age(t: Option<SystemTime>) -> String {
    let Some(t) = t else { return "never".into() };
    let secs = SystemTime::now().duration_since(t).map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

// ---------------------------------------------------------------------------
// Tools

fn find_idioms(reg: &CorpusRegistry, a: FindIdiomsArgs) -> ToolResult {
    check_size("snippet", &a.snippet, MAX_SNIPPET_BYTES)?;
    let lang = parse_lang(&a.lang)?;
    let excl = exclude_paths(a.exclude_path)?;
    let max_lines = clamp(a.max_lines, DEFAULT_MAX_LINES, MIN_MAX_LINES, MAX_MAX_LINES);
    let h = corpus(reg, a.corpus.as_deref())?;
    let params = match a.preset {
        Preset::Repo => Params::repo_preset(),
        Preset::Paper => Params::default(),
    };
    let opts = IdiomOptions { filter: QueryFilter::new().exclude_paths(excl), ..IdiomOptions::default() };
    let idioms = h.engine().find_idioms_with(lang, &a.snippet, &opts, &params).map_err(|e| format!("{e:#}"))?;
    if idioms.is_empty() {
        return Ok(format!(
            "No consensus idioms in corpus `{}` for this {lang} draft. Nothing in the repo suggests a change; keep your code.",
            h.name()
        ));
    }
    let preset = match a.preset {
        Preset::Repo => "repo",
        Preset::Paper => "paper",
    };
    let mut out = format!("{} for {lang} draft · corpus `{}` · preset {preset}\n", plural(idioms.len(), "idiom", "idioms"), h.name());
    for (i, idiom) in idioms.iter().enumerate() {
        let _ = write!(
            out,
            "\n## Idiom {} · confidence {} · {} in {}\nprovenance: {}\n",
            i + 1,
            idiom.confidence,
            plural(idiom.cluster_size, "method", "methods"),
            plural(idiom.distinct_files, "file", "files"),
            cites(&idiom.provenance)
        );
        if !idiom.renames.is_empty() {
            let r: Vec<String> = idiom.renames.iter().map(|r| format!("{}→{}", r.from, r.to)).collect();
            let _ = writeln!(out, "aligned to your names: {}", r.join(", "));
        }
        if !idiom.relevant_imports.is_empty() {
            let _ = writeln!(out, "relevant_imports: {}", idiom.relevant_imports.join("; "));
        }
        let (info, body) = match a.detail {
            Detail::Diff => ("diff", idiom.diff.as_str()),
            Detail::Compact => (lang.as_str(), idiom.compact.as_str()),
            Detail::Full => (lang.as_str(), idiom.code.as_str()),
        };
        out.push_str(&fenced(info, &clip(body, max_lines)));
    }
    Ok(out)
}

fn find_similar(reg: &CorpusRegistry, a: FindSimilarArgs) -> ToolResult {
    check_size("snippet", &a.snippet, MAX_SNIPPET_BYTES)?;
    let lang = parse_lang(&a.lang)?;
    let excl = exclude_paths(a.exclude_path)?;
    let limit = clamp(a.limit, DEFAULT_LIMIT, 1, MAX_LIMIT);
    let max_lines = clamp(a.max_lines, SIMILAR_LINES, MIN_MAX_LINES, MAX_MAX_LINES);
    let scope = match a.scope {
        Scope::All => MethodScope::All,
        Scope::Prod => MethodScope::Prod,
        Scope::Test => MethodScope::Test,
    };
    let h = corpus(reg, a.corpus.as_deref())?;
    let filter = QueryFilter::new().with_scope(scope).exclude_paths(excl);
    let matches = h.engine().find_similar(lang, &a.snippet, limit, &filter).map_err(|e| format!("{e:#}"))?;
    let matches: Vec<_> = matches.into_iter().filter(|m| m.score > 0.0).collect();
    if matches.is_empty() {
        return Ok(format!("No structurally similar {lang} code in corpus `{}` (scope {scope}).", h.name()));
    }
    let mut out =
        format!("{} · {lang} · scope {scope} · corpus `{}`\n", plural(matches.len(), "similar method", "similar methods"), h.name());
    for (i, m) in matches.iter().enumerate() {
        let _ = write!(
            out,
            "\n{}. {} {} [{}] score {:.2} (sim {})\n",
            i + 1,
            m.provenance,
            m.name,
            badge(m.is_test),
            m.score,
            m.sim
        );
        out.push_str(&fenced(m.lang.as_str(), &clip(&m.snippet, max_lines)));
    }
    Ok(out)
}

fn render_guards(out: &mut String, guards: &[MissingGuard], lang: Lang) {
    for (i, g) in guards.iter().enumerate() {
        let kind = serde_json::to_value(g.kind).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        let _ = write!(
            out,
            "{}. {kind}: `{}` · {}/{} peers in {}\n   seen at: {}\n",
            i + 1,
            g.head,
            g.cluster_size,
            g.peers_considered,
            plural(g.distinct_files, "file", "files"),
            cites(&g.provenance)
        );
        if !g.relevant_imports.is_empty() {
            let _ = writeln!(out, "   relevant_imports: {}", g.relevant_imports.join("; "));
        }
        if g.code.lines().count() > 1 {
            out.push_str(&fenced(lang.as_str(), &clip(&g.code, 12)));
        }
    }
}

fn check_guards(reg: &CorpusRegistry, a: GuardArgs) -> ToolResult {
    let excl = exclude_paths(a.exclude_path)?;
    let lang = a.lang.as_deref().map(parse_lang).transpose()?;
    let h = corpus(reg, a.corpus.as_deref())?;
    let base = GuardOptions { filter: QueryFilter::prod().exclude_paths(excl), ..GuardOptions::default() };
    const CLEAN: &str = "No high-confidence missing guards detected.";
    match (a.code.filter(|c| !c.trim().is_empty()), a.diff.filter(|d| !d.trim().is_empty())) {
        (Some(_), Some(_)) => Err("pass either `code` or `diff`, not both".into()),
        (None, None) => Err("pass `code` (with `lang`) or `diff` (a unified `git diff`)".into()),
        (Some(code), None) => {
            check_size("code", &code, MAX_SNIPPET_BYTES)?;
            let lang = lang.ok_or("`code` needs `lang` (one of: java, python, javascript, typescript, tsx, go, rust)")?;
            let guards = h.engine().check_missing_guards(lang, &code, &base).map_err(|e| format!("{e:#}"))?;
            if guards.is_empty() {
                return Ok(CLEAN.into());
            }
            let mut out = format!(
                "{} (corpus `{}`):\n",
                plural(guards.len(), "high-confidence missing guard", "high-confidence missing guards"),
                h.name()
            );
            render_guards(&mut out, &guards, lang);
            Ok(out)
        }
        (None, Some(diff)) => {
            check_size("diff", &diff, MAX_DIFF_BYTES)?;
            let hunks: Vec<DiffHunk> = extract_diff_hunks(&diff);
            if hunks.is_empty() {
                return Err("`diff` contains no unified-diff hunks (expected `git diff` output with @@ headers)".into());
            }
            let total = hunks.len();
            let engine = h.engine();
            let mut sections = Vec::new();
            let mut analyzed = 0usize;
            let mut found = 0usize;
            for hunk in hunks.into_iter().filter(|h| !h.added.is_empty() && !h.after_code.trim().is_empty()) {
                let Some(hl) = hunk.lang.or(lang) else { continue };
                if analyzed == MAX_DIFF_HUNKS {
                    break;
                }
                analyzed += 1;
                let mut o = base.clone();
                o.filter = o.filter.exclude_path(hunk.path.clone());
                let guards = engine.check_missing_guards(hl, &hunk.after_code, &o).map_err(|e| format!("{e:#}"))?;
                if guards.is_empty() {
                    continue;
                }
                found += guards.len();
                let mut s = format!(
                    "\n## {}:{} (hunk +{},{}{})\n",
                    hunk.path,
                    hunk.added.first().map_or(hunk.new_start, |l| l.line),
                    hunk.new_start,
                    hunk.new_lines,
                    if hunk.section.trim().is_empty() { String::new() } else { format!(" · {}", hunk.section.trim()) }
                );
                render_guards(&mut s, &guards, hl);
                sections.push(s);
            }
            let capped = if analyzed == MAX_DIFF_HUNKS && total > MAX_DIFF_HUNKS {
                format!(" (analyzed the first {MAX_DIFF_HUNKS} of {total} hunks)")
            } else {
                String::new()
            };
            if sections.is_empty() {
                return Ok(format!("{CLEAN}{capped}"));
            }
            let mut out = format!(
                "{} in {} (corpus `{}`){capped}:\n",
                plural(found, "high-confidence missing guard", "high-confidence missing guards"),
                plural(sections.len(), "hunk", "hunks"),
                h.name()
            );
            for s in sections {
                out.push_str(&s);
            }
            Ok(out)
        }
    }
}

fn link_name(via: sillage_engine::TestLink) -> String {
    serde_json::to_value(via).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn find_test_idioms(reg: &CorpusRegistry, a: TestIdiomArgs) -> ToolResult {
    check_size("code", &a.code, MAX_SNIPPET_BYTES)?;
    let lang = parse_lang(&a.lang)?;
    let excl = exclude_paths(a.exclude_path)?;
    let max_lines = clamp(a.max_lines, TEST_LINES, MIN_MAX_LINES, MAX_MAX_LINES);
    let h = corpus(reg, a.corpus.as_deref())?;
    let opts = TestIdiomOptions { filter: QueryFilter::new().exclude_paths(excl), ..TestIdiomOptions::default() };
    let ti = h.engine().find_test_idioms(lang, &a.code, &opts).map_err(|e| format!("{e:#}"))?;
    if ti.tests.is_empty() {
        return Ok(format!("No repo tests to model on in corpus `{}` for this {lang} code.", h.name()));
    }
    let mut out = format!("{} · corpus `{}`", plural(ti.tests.len(), "repo test to model on", "repo tests to model on"), h.name());
    if let Some(n) = &ti.draft_name {
        let _ = write!(out, " · draft `{n}`");
    }
    out.push('\n');
    if !ti.peers.is_empty() {
        let _ = writeln!(out, "production peers: {}", cites(&ti.peers));
    }
    for (i, t) in ti.tests.iter().enumerate() {
        let _ = write!(out, "\n## {}. {} {} · via {}", i + 1, t.test.provenance, t.test.name, link_name(t.via));
        if let Some(p) = &t.peer {
            let _ = write!(out, " (peer {})", cite(p));
        }
        let _ = writeln!(out, " · score {:.2}", t.score);
        if !t.relevant_imports.is_empty() {
            let _ = writeln!(out, "test imports: {}", t.relevant_imports.join("; "));
        }
        if !t.fixtures.is_empty() {
            let _ = writeln!(out, "fixtures: {}", t.fixtures.iter().take(4).map(|s| format!("`{s}`")).collect::<Vec<_>>().join(", "));
        }
        if !t.assertions.is_empty() {
            let _ =
                writeln!(out, "assertions: {}", t.assertions.iter().take(4).map(|s| format!("`{s}`")).collect::<Vec<_>>().join(", "));
        }
        let (label, body) = match &t.skeleton {
            Some(s) => ("skeleton (renamed to your function)", s.as_str()),
            None => ("test", t.code.as_str()),
        };
        let _ = writeln!(out, "{label}:");
        out.push_str(&fenced(lang.as_str(), &clip(body, max_lines)));
    }
    Ok(out)
}

fn status_line(h: &CorpusHandle, default: bool) -> String {
    let m = h.meta();
    let langs: Vec<&str> = m.langs.iter().map(|l| l.as_str()).collect();
    let mut s = format!(
        "- `{}`{} · {} · {} methods ({} prod / {} test) in {} · langs {} · ",
        h.name(),
        if default { " (default)" } else { "" },
        h.kind().as_str(),
        m.methods,
        m.prod_methods,
        m.test_methods,
        plural(m.files, "file", "files"),
        if langs.is_empty() { "-".to_string() } else { langs.join(", ") }
    );
    match (h.kind(), m.last_stats) {
        (SourceKind::Directory, Some(st)) => {
            let _ = write!(
                s,
                "refreshed {} (+{} ~{} -{} files, {:.1} ms)",
                age(m.last_refresh),
                st.files_added,
                st.files_modified,
                st.files_removed,
                st.elapsed_ms
            );
        }
        (SourceKind::Directory, None) => s.push_str("not refreshed yet"),
        (SourceKind::Snapshot, _) => {
            let _ = write!(s, "static snapshot built {}", age(m.last_refresh));
        }
    }
    s
}

fn corpus_status(reg: &CorpusRegistry, a: CorpusArgs) -> ToolResult {
    let default = reg.default_name().to_string();
    match a.corpus.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        Some(name) => {
            let h = corpus(reg, Some(name))?;
            Ok(status_line(&h, h.name() == default))
        }
        None => {
            let mut out = format!("{} loaded:\n", plural(reg.len(), "corpus", "corpora"));
            for h in reg.iter() {
                out.push_str(&status_line(h, h.name() == default));
                out.push('\n');
            }
            Ok(out)
        }
    }
}

fn refresh_line(name: &str, s: &sillage_index::RefreshStats) -> String {
    format!(
        "- `{name}`: files_added {}, files_modified {}, files_removed {}, files_unchanged {} → total_methods {} (elapsed_ms {:.1})",
        s.files_added, s.files_modified, s.files_removed, s.files_unchanged, s.total_methods, s.elapsed_ms
    )
}

fn refresh_corpus(reg: &CorpusRegistry, a: CorpusArgs) -> ToolResult {
    match a.corpus.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        Some(name) => {
            let h = corpus(reg, Some(name))?;
            if h.kind() == SourceKind::Snapshot {
                return Err(format!(
                    "corpus `{}` is a static .sil snapshot and cannot be refreshed; rebuild it with `sillage index` and restart the server",
                    h.name()
                ));
            }
            let s = h.refresh().map_err(|e| format!("{e:#}"))?;
            Ok(format!("Refreshed:\n{}", refresh_line(h.name(), &s)))
        }
        None => {
            let results = reg.refresh_all();
            if results.is_empty() {
                return Ok("No directory corpora to refresh (all loaded corpora are static .sil snapshots).".into());
            }
            let mut out = String::from("Refreshed:\n");
            let mut failed = false;
            for (name, r) in results {
                match r {
                    Ok(s) => out.push_str(&refresh_line(&name, &s)),
                    Err(e) => {
                        failed = true;
                        let _ = write!(out, "- `{name}`: refresh failed: {e:#}");
                    }
                }
                out.push('\n');
            }
            let skipped: Vec<&str> = reg.iter().filter(|h| h.kind() == SourceKind::Snapshot).map(|h| h.name()).collect();
            if !skipped.is_empty() {
                let _ = writeln!(out, "skipped static snapshots: {}", skipped.join(", "));
            }
            if failed { Err(out) } else { Ok(out) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_and_fence() {
        assert_eq!(clip("a\nb\nc\n", 5), "a\nb\nc");
        assert_eq!(clip("a\nb\nc\nd", 2), "a\nb\n… (2 more lines)");
        assert_eq!(fenced("go", "x"), "```go\nx\n```\n");
        assert_eq!(fenced("md", "```\ny\n```"), "````md\n```\ny\n```\n````\n");
    }

    #[test]
    fn clamps() {
        assert_eq!(clamp(None, 5, 1, 20), 5);
        assert_eq!(clamp(Some(0), 5, 1, 20), 1);
        assert_eq!(clamp(Some(1_000_000), 5, 1, 20), 20);
    }

    #[test]
    fn definitions_cover_all_tools() {
        let defs = tool_definitions();
        let names: Vec<&str> = defs.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            ["find_idioms", "find_similar_code", "check_missing_guards", "find_test_idioms", "corpus_status", "refresh_corpus"]
        );
        for t in defs.as_array().unwrap() {
            assert_eq!(t["inputSchema"]["type"], "object");
            assert!(t["description"].as_str().unwrap().len() > 40);
        }
    }

    #[test]
    fn exclude_paths_limits() {
        assert_eq!(exclude_paths(Some(Paths::One(" a.go ".into()))).unwrap(), ["a.go"]);
        assert!(exclude_paths(Some(Paths::Many(vec!["x".into(); 40]))).is_err());
        assert!(exclude_paths(None).unwrap().is_empty());
    }
}
