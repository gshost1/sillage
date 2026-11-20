//! High-level analyses for coding agents, built on the Aroma pipeline.
//!
//! - [`Engine::find_idioms`]: Aroma recommendations for a draft, aligned to the
//!   draft's variable names, with a unified diff, the imports the added lines
//!   need, and a [`Confidence`] that favours multi-method consensus clusters.
//! - [`Engine::check_missing_guards`]: Aroma "upside down" for finished code:
//!   finds the draft's production peers and reports only guard lines (error,
//!   null and bounds checks, cleanup, logging, validation) that a consensus of
//!   peers across several files has and the draft lacks. Silent otherwise.
//! - [`Engine::find_test_idioms`]: repo test skeletons for a draft, from tests
//!   structurally similar to it and tests of its production peers.
//!
//! Every analysis prefers returning nothing over returning weak matches.

use std::collections::BTreeMap;

use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use sillage_core::{Lang, MethodId, MethodRecord, MethodScope, Params, lang_compatible};
use sillage_render::Layout;

use crate::align::{Rename, render_aligned};
use crate::hunks::{DiffHunk, extract_diff_hunks};
use crate::{Engine, QueryFilter, Ranked};

/// How much a result is backed by the repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// A single method.
    Low,
    /// Two or more methods agree (possibly in one file).
    Medium,
    /// Three or more methods in at least two files agree.
    High,
}

impl Confidence {
    /// Confidence of a consensus of `methods` distinct methods in `files` distinct files.
    pub fn of(methods: usize, files: usize) -> Self {
        if methods >= 3 && files >= 2 {
            Confidence::High
        } else if methods >= 2 {
            Confidence::Medium
        } else {
            Confidence::Low
        }
    }
}

impl std::fmt::Display for Confidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Confidence::Low => "low",
            Confidence::Medium => "medium",
            Confidence::High => "high",
        })
    }
}

impl std::str::FromStr for Confidence {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Ok(Confidence::Low),
            "medium" | "med" => Ok(Confidence::Medium),
            "high" => Ok(Confidence::High),
            other => anyhow::bail!("unknown confidence: {other} (expected low, medium or high)"),
        }
    }
}

/// A `path:line` citation of a corpus method (or of one line in it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Citation {
    pub id: MethodId,
    pub name: String,
    pub path: String,
    /// The cited line (the method's first line unless a specific line is cited).
    pub line: u32,
    pub line_start: u32,
    pub line_end: u32,
    pub is_test: bool,
    /// `path:line`.
    pub provenance: String,
}

impl Citation {
    pub fn of(m: &MethodRecord) -> Self {
        Self::at(m, m.line_start)
    }

    /// Cites line `line` (1-based, in the file) of `m`.
    pub fn at(m: &MethodRecord, line: u32) -> Self {
        let path = m.rel_path();
        Self {
            id: m.id,
            name: m.name.to_string(),
            provenance: format!("{path}:{line}"),
            path,
            line,
            line_start: m.line_start,
            line_end: m.line_end(),
            is_test: m.is_test,
        }
    }
}

/// Options for [`Engine::find_idioms`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdiomOptions {
    #[serde(default)]
    pub filter: QueryFilter,
    /// Results below this confidence are dropped (default: medium, i.e. no singletons).
    pub min_confidence: Confidence,
    pub max_results: usize,
    /// Imports the draft's file already has; they are not suggested again.
    #[serde(default)]
    pub existing_imports: Vec<String>,
}

impl Default for IdiomOptions {
    fn default() -> Self {
        Self { filter: QueryFilter::default(), min_confidence: Confidence::Medium, max_results: 3, existing_imports: Vec::new() }
    }
}

/// One recommended idiom for a draft.
#[derive(Debug, Clone, Serialize)]
pub struct Idiom {
    /// The recommendation with the draft's variable names.
    pub code: String,
    /// Only the recommended (extra) lines.
    pub added_code: String,
    /// Compact rendering: added lines with one line of context.
    pub compact: String,
    /// Unified diff of the queried draft against `code`.
    pub diff: String,
    /// Imports of the cluster members' files that `added_code` references.
    pub relevant_imports: Vec<String>,
    pub confidence: Confidence,
    /// Methods in the consensus cluster.
    pub cluster_size: usize,
    /// Distinct files among them.
    pub distinct_files: usize,
    pub provenance: Vec<Citation>,
    pub renames: Vec<Rename>,
    /// Cluster scores `l(τ)` and `s(τ)`.
    pub l: f64,
    pub s: f64,
    #[serde(skip)]
    pub layout: Layout,
}

/// Category of a missing guard line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardKind {
    ErrorCheck,
    NullCheck,
    BoundsCheck,
    Cleanup,
    Logging,
    Validation,
}

/// Options for [`Engine::check_missing_guards`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuardOptions {
    /// Default: production code only ([`QueryFilter::prod`]).
    pub filter: QueryFilter,
    /// Structural peers considered.
    pub peers: usize,
    /// Minimum peer score (`SimScore / |F(q)|`).
    pub min_score: f64,
    /// Minimum peers sharing a guard.
    pub min_support: usize,
    /// Minimum distinct files among them.
    pub min_files: usize,
    /// Minimum share of the considered peers sharing a guard.
    pub min_ratio: f64,
    #[serde(default)]
    pub existing_imports: Vec<String>,
}

impl Default for GuardOptions {
    fn default() -> Self {
        Self {
            filter: QueryFilter::prod(),
            peers: 10,
            min_score: 0.5,
            min_support: 3,
            min_files: 2,
            min_ratio: 0.5,
            existing_imports: Vec::new(),
        }
    }
}

/// A guard that a consensus of peers has and the draft lacks.
#[derive(Debug, Clone, Serialize)]
pub struct MissingGuard {
    pub kind: GuardKind,
    /// The guard's first line, aligned to the draft's names and normalized.
    pub head: String,
    /// The guard block from the first supporting peer (aligned).
    pub code: String,
    pub confidence: Confidence,
    /// Peers having the guard.
    pub cluster_size: usize,
    pub distinct_files: usize,
    /// Peers considered.
    pub peers_considered: usize,
    /// Each supporting peer, citing the guard's line.
    pub provenance: Vec<Citation>,
    pub relevant_imports: Vec<String>,
}

/// Missing guards for one diff hunk.
#[derive(Debug, Clone, Serialize)]
pub struct HunkGuards {
    pub hunk: DiffHunk,
    pub guards: Vec<MissingGuard>,
}

/// Options for [`Engine::find_test_idioms`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestIdiomOptions {
    /// Excluded ids / paths apply to tests and peers; the scope is set per search.
    #[serde(default)]
    pub filter: QueryFilter,
    /// Production peers followed to their tests.
    pub peers: usize,
    pub min_score: f64,
    pub max_results: usize,
    #[serde(default)]
    pub existing_imports: Vec<String>,
}

impl Default for TestIdiomOptions {
    fn default() -> Self {
        Self { filter: QueryFilter::default(), peers: 5, min_score: 0.4, max_results: 5, existing_imports: Vec::new() }
    }
}

/// How a test was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestLink {
    /// The test calls the draft's own function name.
    ReferencesDraft,
    /// The test references a production peer of the draft.
    ReferencesPeer,
    /// The test lives in the test file corresponding to a peer's file.
    CorrespondingFile,
    /// The test is structurally similar to the draft.
    Similar,
}

/// One repository test to model a new test on.
#[derive(Debug, Clone, Serialize)]
pub struct TestIdiom {
    pub test: Citation,
    pub via: TestLink,
    /// The production peer that led to the test.
    pub peer: Option<Citation>,
    pub score: f64,
    /// The test's source.
    pub code: String,
    /// `code` with the peer's name replaced by the draft's (when both are known).
    pub skeleton: Option<String>,
    /// Assertion lines of the test.
    pub assertions: Vec<String>,
    /// Setup / fixture lines of the test.
    pub fixtures: Vec<String>,
    /// Imports of the test's file that the test references.
    pub relevant_imports: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestIdioms {
    /// The draft's function name, when the draft is a complete function.
    pub draft_name: Option<String>,
    /// The production peers followed.
    pub peers: Vec<Citation>,
    pub tests: Vec<TestIdiom>,
}

/// A draft split into the snippet the engine queries and its function name.
struct Draft {
    query: String,
    name: Option<String>,
}

/// When `code` is (mostly) one complete function, the query is its body
/// (corpus trees are bodies, Decision 10) without the outer braces;
/// otherwise the whole snippet.
fn split_draft(lang: Lang, code: &str) -> Draft {
    let methods = sillage_parse::extract_methods(lang, code);
    let Some(m) = methods.iter().max_by_key(|m| m.text_span.len()) else {
        return Draft { query: code.to_string(), name: None };
    };
    let covered = m.text_span.len() as usize * 10 >= code.trim().len() * 6;
    if !covered {
        return Draft { query: code.to_string(), name: None };
    }
    let base = m.text_span.start as usize;
    let (mut s, e) = (base + m.body_span.start as usize, base + m.body_span.end as usize);
    // Start the body at its line start, keeping the first line's indentation.
    let line_start = code[..s].rfind('\n').map_or(0, |i| i + 1);
    if code[line_start..s].trim().is_empty() {
        s = line_start;
    }
    let body = code.get(s..e).unwrap_or(code);
    let t = body.trim();
    let inner = t.strip_prefix('{').and_then(|x| x.strip_suffix('}')).map(|x| x.trim_matches('\n')).unwrap_or(body);
    let name = (&*m.name != "<anonymous>").then(|| m.name.to_string());
    Draft { query: if inner.trim().is_empty() { body.to_string() } else { inner.to_string() }, name }
}

/// Whitespace-free text, for "is this the same code" checks.
fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Body text without whitespace or the outer braces.
fn squash_body(s: &str) -> String {
    let q = squash(s);
    q.strip_prefix('{').and_then(|x| x.strip_suffix('}')).map(str::to_string).unwrap_or(q)
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Distinct files among methods.
fn file_count<'a>(ms: impl IntoIterator<Item = &'a MethodRecord>) -> usize {
    ms.into_iter().map(|m| (&m.repo, &m.path)).collect::<FxHashSet<_>>().len()
}

/// `w` occurs in `hay` as a whole word: not preceded or followed by an
/// identifier character (Unicode alphanumerics, `_`, `$`). UTF-8 safe.
fn has_word(hay: &str, w: &str) -> bool {
    if w.is_empty() {
        return false;
    }
    let id = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    hay.match_indices(w).any(|(s, _)| {
        let e = s + w.len();
        hay[..s].chars().next_back().is_none_or(|c| !id(c)) && hay[e..].chars().next().is_none_or(|c| !id(c))
    })
}

/// Classifies one line of code as a guard, by language-agnostic idioms.
pub fn classify_guard(lang: Lang, line: &str) -> Option<GuardKind> {
    let t = line.trim();
    if t.is_empty() || matches!(t, "}" | "{" | ")" | "};" | "});" | "end") || t.starts_with("//") || t.starts_with('#') && lang == Lang::Python {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    let any = |pats: &[&str]| pats.iter().any(|p| t.contains(p));
    let is_cond = ["if ", "if(", "elif ", "unless ", "guard ", "while ", "} else if"].iter().any(|p| t.starts_with(p));

    // Cleanup: deferred / finally / resource release.
    if t.starts_with("defer ")
        || t.starts_with("finally")
        || t.starts_with("} finally")
        || (lang == Lang::Python && t.starts_with("with "))
        || (lang == Lang::Java && t.starts_with("try ("))
        || any(&[".close()", ".Close()", ".unlock()", ".Unlock()", ".release()", ".dispose()", ".Release()", ".shutdown()", ".Shutdown()", ".RUnlock()", "drop("])
    {
        return Some(GuardKind::Cleanup);
    }
    // Error checks.
    if any(&["err != nil", "err == nil", "error != nil", ".is_err()", "if let Err", "Err(e) =>", "Err(err) =>", "Err(_) =>"])
        || t.starts_with("catch")
        || t.starts_with("} catch")
        || t.starts_with("except")
        || t.starts_with("try:")
        || t.starts_with("try {")
        || t == "try"
        || any(&[".catch(", "if (err)", "if (error)", "if err:", "if error:"])
        || (is_cond && (has_word(t, "err") || has_word(t, "error")))
    {
        return Some(GuardKind::ErrorCheck);
    }
    // Null / nil / None / undefined checks.
    if any(&["== nil", "!= nil", "== null", "!= null", "=== null", "!== null", "=== undefined", "!== undefined", "is None", "is not None", "requireNonNull", ".is_none()", ".is_some()", "if let Some", "let Some(", "Optional.ofNullable", "isPresent()", "?? "])
        || (lang == Lang::Python && t.starts_with("if not "))
        || (matches!(lang, Lang::JavaScript | Lang::TypeScript | Lang::Tsx) && (t.starts_with("if (!") || t.starts_with("if(!")))
    {
        return Some(GuardKind::NullCheck);
    }
    // Bounds / emptiness checks in a condition.
    let sized = any(&["len(", ".len()", ".length", ".size()", ".count", "is_empty()", "isEmpty()", ".empty()", "Length", "cap(", ".Len()"]);
    let compares = any(&["<", ">", "==", "!=", "is_empty", "isEmpty", "empty()"]);
    if (is_cond || t.starts_with("assert") || t.starts_with("require")) && sized && compares {
        return Some(GuardKind::BoundsCheck);
    }
    // Logging.
    if any(&["log.", "Log.", "LOG.", "logger.", "Logger.", "logging.", "console.error", "console.warn", "console.log", "console.info", "tracing::", "log::", "warn!(", "error!(", "info!(", "debug!(", "trace!(", "eprintln!(", "slog.", "zap.", "klog.", "logrus."])
    {
        return Some(GuardKind::Logging);
    }
    // Validation: assertions, early failures.
    if t.starts_with("assert")
        || any(&["ensure!(", "bail!(", "debug_assert", "panic!(", "unreachable!(", "Preconditions.check", "IllegalArgumentException", "IllegalStateException", "raise ValueError", "raise TypeError", "throw new Error", "throw new TypeError", "throw new RangeError", "errors.New(", "return Err("])
        || (t.starts_with("throw ") || t.starts_with("raise "))
        || (is_cond && lower.contains("valid"))
    {
        return Some(GuardKind::Validation);
    }
    None
}

/// Guard blocks of a peer layout: `(head line index, block code)` for every
/// extra line classified as a guard, with the lines it opens.
fn guard_blocks(lang: Lang, lay: &Layout) -> Vec<(usize, GuardKind, String)> {
    let indent = |s: &str| s.len() - s.trim_start().len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lay.lines.len() {
        let l = &lay.lines[i];
        let Some(kind) = (l.extra && !l.placeholder).then(|| classify_guard(lang, &l.text)).flatten() else {
            i += 1;
            continue;
        };
        let head_indent = indent(&l.text);
        let opens = l.text.trim_end().ends_with('{') || l.text.trim_end().ends_with(':') || l.text.trim_end().ends_with("=> {");
        let mut block = vec![l.text.clone()];
        let mut j = i + 1;
        if opens {
            while j < lay.lines.len() {
                let t = &lay.lines[j].text;
                if !t.trim().is_empty() && indent(t) <= head_indent {
                    let closes = t.trim_start().starts_with('}') || t.trim_start().starts_with(')');
                    if closes && indent(t) == head_indent {
                        block.push(t.clone());
                        j += 1;
                    }
                    break;
                }
                block.push(t.clone());
                j += 1;
            }
        }
        out.push((i, kind, block.join("\n")));
        i = if opens { j } else { i + 1 };
    }
    out
}

impl Engine {
    /// The records of `ids` (unknown ids skipped).
    fn records(&self, ids: &[MethodId]) -> Vec<&MethodRecord> {
        ids.iter().filter_map(|&id| self.method(id)).collect()
    }

    /// Imports of `members`' files (first member first, deduplicated) that
    /// `code` references, minus `existing`.
    fn imports_for(&self, lang: Lang, members: &[&MethodRecord], code: &str, existing: &[String]) -> Vec<String> {
        let mut seen: FxHashSet<&str> = FxHashSet::default();
        let mut all: Vec<Box<str>> = Vec::new();
        for m in members {
            for imp in self.corpus.imports_of(m) {
                if seen.insert(imp) {
                    all.push(imp.clone());
                }
            }
        }
        sillage_parse::relevant_imports_excluding(lang, &all, code, existing)
    }

    /// Aroma recommendations for `draft`, aligned to its variable names (see
    /// the [module docs](self)). Recommendations without added lines, or below
    /// `opts.min_confidence`, are dropped; the rest are ordered by confidence,
    /// then cluster size.
    pub fn find_idioms(&self, lang: Lang, draft: &str, opts: &IdiomOptions) -> anyhow::Result<Vec<Idiom>> {
        self.find_idioms_with(lang, draft, opts, &self.params)
    }

    /// [`find_idioms`](Self::find_idioms) with per-call parameters instead of
    /// [`Engine::params`].
    pub fn find_idioms_with(&self, lang: Lang, draft: &str, opts: &IdiomOptions, params: &Params) -> anyhow::Result<Vec<Idiom>> {
        let d = split_draft(lang, draft);
        let out = self.recommend_filtered_with(lang, &d.query, &opts.filter, params)?;
        if out.recommendations.is_empty() {
            return Ok(Vec::new());
        }
        let qtree = sillage_parse::parse_snippet(lang, &d.query)?;
        let featurizer = self.featurizer.as_ref();
        let mut idioms: Vec<Idiom> = out
            .recommendations
            .iter()
            .filter_map(|rec| {
                let members = self.records(&rec.cluster);
                let files = file_count(members.iter().copied());
                let confidence = Confidence::of(members.len(), files);
                if confidence < opts.min_confidence {
                    return None;
                }
                let al = render_aligned(&d.query, &qtree, &rec.tree, &rec.query_match, featurizer);
                if al.added_code.trim().is_empty() {
                    return None;
                }
                let relevant_imports = self.imports_for(lang, &members, &al.added_code, &opts.existing_imports);
                Some(Idiom {
                    compact: sillage_render::render_mode(&al.layout, sillage_render::RenderMode::Compact, None),
                    code: al.code,
                    added_code: al.added_code,
                    diff: al.diff,
                    relevant_imports,
                    confidence,
                    cluster_size: members.len(),
                    distinct_files: files,
                    provenance: members.iter().map(|m| Citation::of(m)).collect(),
                    renames: al.renames,
                    l: rec.l,
                    s: rec.s,
                    layout: al.layout,
                })
            })
            .collect();
        idioms.sort_by(|a, b| b.confidence.cmp(&a.confidence).then(b.cluster_size.cmp(&a.cluster_size)));
        idioms.truncate(opts.max_results);
        Ok(idioms)
    }

    /// Structural peers of a draft query: the top `n` admitted by `filter`
    /// with score ≥ `min_score`, excluding methods whose body is the draft.
    fn peers(&self, lang: Lang, query: &str, n: usize, min_score: f64, filter: &QueryFilter) -> anyhow::Result<Vec<Ranked>> {
        let me = squash_body(query);
        let mut ranked = self.search_filtered(lang, query, n + 2, filter)?;
        ranked.retain(|r| r.score >= min_score && self.method(r.id).is_some_and(|m| squash_body(m.body_text()) != me));
        ranked.truncate(n);
        Ok(ranked)
    }

    /// Guards (error / null / bounds checks, cleanup, logging, validation)
    /// that at least `opts.min_support` structural peers in `opts.min_files`
    /// files (and `opts.min_ratio` of the peers considered) have and `code`
    /// lacks. `code` is a finished function or any snippet (e.g. a diff hunk's
    /// new side). Returns nothing unless the consensus is high-confidence.
    pub fn check_missing_guards(&self, lang: Lang, code: &str, opts: &GuardOptions) -> anyhow::Result<Vec<MissingGuard>> {
        let d = split_draft(lang, code);
        let peers = self.peers(lang, &d.query, opts.peers, opts.min_score, &opts.filter)?;
        if peers.len() < opts.min_support.max(1) {
            return Ok(Vec::new());
        }
        let qtree = sillage_parse::parse_snippet(lang, &d.query)?;
        let featurizer = self.featurizer.as_ref();
        let draft_lines: FxHashSet<String> = code.lines().map(squash).filter(|l| !l.is_empty()).collect();

        // head key -> (kind, first block, supporters (peer index, file line))
        let mut votes: BTreeMap<String, (GuardKind, String, Vec<(usize, u32)>)> = BTreeMap::new();
        for (pi, peer) in peers.iter().enumerate() {
            let Some(m) = self.method(peer.id) else { continue };
            let al = render_aligned(&d.query, &qtree, &peer.tree, &peer.pruned.keep, featurizer);
            let mut seen_here: FxHashSet<String> = FxHashSet::default();
            for (li, kind, block) in guard_blocks(lang, &al.layout) {
                let line = &al.layout.lines[li];
                let head = collapse_ws(&line.text);
                if draft_lines.contains(&squash(&head)) || !seen_here.insert(head.clone()) {
                    continue;
                }
                let file_line = line.src_line.map_or(m.line_start, |s| m.line_start + s as u32 - 1);
                let e = votes.entry(head).or_insert_with(|| (kind, block, Vec::new()));
                e.2.push((pi, file_line));
            }
        }

        let considered = peers.len();
        let mut guards: Vec<MissingGuard> = votes
            .into_iter()
            .filter_map(|(head, (kind, code, support))| {
                let members: Vec<&MethodRecord> = support.iter().filter_map(|&(pi, _)| self.method(peers[pi].id)).collect();
                let files = file_count(members.iter().copied());
                let ratio = support.len() as f64 / considered as f64;
                let ok = support.len() >= opts.min_support && files >= opts.min_files && ratio >= opts.min_ratio;
                if !ok || Confidence::of(support.len(), files) != Confidence::High {
                    return None;
                }
                let provenance =
                    support.iter().filter_map(|&(pi, line)| Some(Citation::at(self.method(peers[pi].id)?, line))).collect();
                let relevant_imports = self.imports_for(lang, &members, &code, &opts.existing_imports);
                Some(MissingGuard {
                    kind,
                    head,
                    code,
                    confidence: Confidence::High,
                    cluster_size: support.len(),
                    distinct_files: files,
                    peers_considered: considered,
                    provenance,
                    relevant_imports,
                })
            })
            .collect();
        guards.sort_by(|a, b| b.cluster_size.cmp(&a.cluster_size).then(a.kind.cmp(&b.kind)).then(a.head.cmp(&b.head)));
        Ok(guards)
    }

    /// [`check_missing_guards`](Self::check_missing_guards) over the new side
    /// of every hunk of a unified diff whose language is indexed (see
    /// [`check_missing_guards_in_hunk`](Self::check_missing_guards_in_hunk)).
    /// Only hunks with findings are returned.
    pub fn check_missing_guards_in_diff(&self, unified_diff: &str, opts: &GuardOptions) -> anyhow::Result<Vec<HunkGuards>> {
        let mut out = Vec::new();
        for hunk in extract_diff_hunks(unified_diff) {
            let Some(lang) = hunk.lang else { continue };
            let guards = self.check_missing_guards_in_hunk(lang, &hunk, opts)?;
            if !guards.is_empty() {
                out.push(HunkGuards { hunk, guards });
            }
        }
        Ok(out)
    }

    /// Missing guards for one diff hunk. The hunk's own file is excluded from
    /// its peers. When the engine has a root and the file on disk holds the
    /// hunk's new side, the hunk is widened to its enclosing method, so guards
    /// the method already has outside the hunk are not reported. Only guards
    /// sharing an identifier with the hunk's added lines are kept (guards
    /// without identifiers, like `try {`, always are).
    pub fn check_missing_guards_in_hunk(&self, lang: Lang, hunk: &DiffHunk, opts: &GuardOptions) -> anyhow::Result<Vec<MissingGuard>> {
        if hunk.added.is_empty() || hunk.after_code.trim().is_empty() {
            return Ok(Vec::new());
        }
        let code = self.enclosing_method_on_disk(lang, hunk).unwrap_or_else(|| hunk.after_code.clone());
        let mut o = opts.clone();
        o.filter = o.filter.exclude_path(hunk.path.clone());
        let guards = self.check_missing_guards(lang, &code, &o)?;
        let added: FxHashSet<String> = hunk.added.iter().flat_map(|l| sillage_index::tokenize(&l.text)).collect();
        Ok(guards
            .into_iter()
            .filter(|g| {
                let ids: Vec<String> = guard_identifiers(&g.head).collect();
                ids.is_empty() || ids.iter().any(|t| added.contains(t))
            })
            .collect())
    }

    /// The text of the one method of `hunk`'s file (read under the engine's
    /// root) that contains the hunk's added lines, when the file's content
    /// matches them. `None` without a root, for paths escaping it, oversized
    /// files or methods, and when the added lines span several methods.
    fn enclosing_method_on_disk(&self, lang: Lang, hunk: &DiffHunk) -> Option<String> {
        const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
        let root = self.root.as_deref()?;
        let rel = std::path::Path::new(hunk.new_path.as_deref()?);
        if hunk.is_deleted_file || !rel.components().all(|c| matches!(c, std::path::Component::Normal(_))) {
            return None;
        }
        let path = std::fs::canonicalize(root.join(rel)).ok()?;
        if !path.starts_with(root) || std::fs::metadata(&path).ok()?.len() > MAX_FILE_BYTES {
            return None;
        }
        let src = std::fs::read_to_string(&path).ok()?;
        let file_lines: Vec<&str> = src.lines().collect();
        let mut owner: Option<usize> = None;
        let methods = sillage_parse::extract_methods(lang, &src);
        let ranges: Vec<(u32, u32)> = methods
            .iter()
            .map(|m| {
                let text = &src[m.text_span.start as usize..m.text_span.end as usize];
                (m.line_start, m.line_start + text.lines().count().max(1) as u32 - 1)
            })
            .collect();
        for l in hunk.added.iter().filter(|l| !l.text.trim().is_empty()) {
            let disk = file_lines.get((l.line as usize).checked_sub(1)?)?;
            if disk.trim_end() != l.text.trim_end() {
                return None; // the file is not the diff's new side
            }
            // Innermost method containing the line.
            let inner = ranges
                .iter()
                .enumerate()
                .filter(|(_, (a, b))| (*a..=*b).contains(&l.line))
                .min_by_key(|(_, (a, b))| b - a)
                .map(|(i, _)| i);
            match (owner, inner) {
                (_, None) => {}
                (None, Some(i)) => owner = Some(i),
                (Some(o), Some(i)) if o == i => {}
                _ => return None,
            }
        }
        let m = &methods[owner?];
        let text = &src[m.text_span.start as usize..m.text_span.end as usize];
        (text.len() <= MAX_EXPANDED_METHOD_BYTES).then(|| text.to_string())
    }

    /// The `tests` whose text mentions `name` as a whole word. When the BM25
    /// index covers the corpus, candidates come from its posting list for
    /// `name` (bodies only) instead of a full-text scan of every test.
    fn tests_mentioning<'a>(&self, tests: &[&'a MethodRecord], name: &str) -> Vec<&'a MethodRecord> {
        let single_token = {
            let mut it = sillage_index::tokenize(name);
            it.next().as_deref() == Some(name) && it.next().is_none()
        };
        if single_token && self.bm25.num_docs() == self.corpus.methods.len() {
            let cand: FxHashSet<MethodId> = self.bm25.docs_with_term(name).collect();
            if cand.is_empty() {
                return Vec::new();
            }
            return tests.iter().copied().filter(|t| cand.contains(&t.id) && has_word(&t.text, name)).collect();
        }
        tests.iter().copied().filter(|t| has_word(&t.text, name)).collect()
    }

    /// Tests to model a test of `draft` on: tests that call the draft's own
    /// function, tests structurally similar to the draft, and tests of its
    /// production peers (tests that reference a peer by name, or live in the
    /// peer's corresponding test file).
    pub fn find_test_idioms(&self, lang: Lang, draft: &str, opts: &TestIdiomOptions) -> anyhow::Result<TestIdioms> {
        let d = split_draft(lang, draft);
        let base = QueryFilter { scope: MethodScope::All, ..opts.filter.clone() };
        let peers = self.peers(lang, &d.query, opts.peers, opts.min_score, &base.clone().with_scope(MethodScope::Prod))?;
        let similar = self.peers(lang, &d.query, opts.max_results, opts.min_score, &base.clone().with_scope(MethodScope::Test))?;
        let tests_filter = base.with_scope(MethodScope::Test).compile(&self.corpus, self.root.as_deref());
        let tests: Vec<&MethodRecord> =
            self.corpus.methods.iter().filter(|m| lang_compatible(lang, m.lang) && tests_filter.admits(m)).collect();

        // test id -> (score, link, peer)
        let mut found: FxHashMap<MethodId, (f64, TestLink, Option<MethodId>)> = FxHashMap::default();
        let mut offer = |id: MethodId, score: f64, link: TestLink, peer: Option<MethodId>| {
            let e = found.entry(id).or_insert((f64::MIN, link, peer));
            if score > e.0 {
                *e = (score, link, peer);
            }
        };
        if let Some(name) = d.name.as_deref().filter(|n| distinctive(n)) {
            for t in self.tests_mentioning(&tests, name) {
                if &*t.name != name {
                    offer(t.id, 1.0 + name_bonus(&t.name, name), TestLink::ReferencesDraft, None);
                }
            }
        }
        for r in &similar {
            offer(r.id, 0.8 * r.score, TestLink::Similar, None);
        }
        for p in &peers {
            let Some(pm) = self.method(p.id) else { continue };
            let peer_path = pm.rel_path();
            let mentions: FxHashSet<MethodId> = if distinctive(&pm.name) {
                self.tests_mentioning(&tests, &pm.name).into_iter().filter(|t| t.name != pm.name).map(|t| t.id).collect()
            } else {
                FxHashSet::default()
            };
            for t in &tests {
                if mentions.contains(&t.id) {
                    offer(t.id, p.score * (0.9 + name_bonus(&t.name, &pm.name)), TestLink::ReferencesPeer, Some(p.id));
                } else if corresponding_test_file(lang, &peer_path, &t.rel_path()) {
                    offer(t.id, 0.6 * p.score, TestLink::CorrespondingFile, Some(p.id));
                }
            }
        }

        let mut ranked: Vec<(MethodId, (f64, TestLink, Option<MethodId>))> = found.into_iter().collect();
        ranked.sort_by(|a, b| b.1.0.total_cmp(&a.1.0).then(a.1.1.cmp(&b.1.1)).then(a.0.cmp(&b.0)));
        ranked.truncate(opts.max_results);
        let tests = ranked
            .into_iter()
            .filter_map(|(id, (score, via, peer))| {
                let t = self.method(id)?;
                let peer = peer.and_then(|p| self.method(p));
                let code = t.text.to_string();
                let skeleton = match (peer, d.name.as_deref()) {
                    (Some(p), Some(name)) if via != TestLink::Similar => {
                        let renamed = sillage_parse::rename_identifiers(t.lang, &code, &p.name, name);
                        // `TestLoadUser` → `TestLoadInvoice` for peer `LoadUser`.
                        let new_test_name = t.name.replacen(&*p.name, name, 1);
                        Some(sillage_parse::rename_identifiers(t.lang, &renamed, &t.name, &new_test_name))
                    }
                    _ => None,
                };
                Some(TestIdiom {
                    test: Citation::of(t),
                    via,
                    peer: peer.map(Citation::of),
                    score,
                    assertions: marked_lines(&code, ASSERT_MARKS),
                    fixtures: marked_lines(&code, FIXTURE_MARKS),
                    relevant_imports: self.imports_for(t.lang, &[t], &code, &opts.existing_imports),
                    code,
                    skeleton,
                })
            })
            .collect();
        Ok(TestIdioms {
            draft_name: d.name,
            peers: peers.iter().filter_map(|p| self.method(p.id)).map(Citation::of).collect(),
            tests,
        })
    }
}

/// Largest enclosing method a diff hunk is widened to (bytes).
const MAX_EXPANDED_METHOD_BYTES: usize = 64 * 1024;

/// Identifiers of a guard line, without keywords, literals of common
/// sentinels and numbers: what ties a guard to the code it protects.
fn guard_identifiers(head: &str) -> impl Iterator<Item = String> + '_ {
    const KEYWORDS: &[&str] = &[
        "if", "else", "elif", "for", "while", "return", "nil", "null", "None", "undefined", "true", "false", "True",
        "False", "not", "is", "and", "or", "let", "var", "const", "defer", "try", "catch", "except", "finally", "raise",
        "throw", "throws", "new", "self", "this", "fn", "func", "def", "in", "of", "mut", "Some", "Ok", "Err", "unless",
        "guard", "with", "as", "await", "async", "go", "match", "case",
    ];
    // String literals are not identifiers of the guarded code.
    let mut code = String::with_capacity(head.len());
    let mut quote: Option<char> = None;
    for c in head.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if matches!(c, '"' | '\'' | '`') => quote = Some(c),
            None => code.push(c),
        }
    }
    sillage_index::tokenize_owned(code)
        .filter(|t| !KEYWORDS.contains(&t.as_str()) && !t.bytes().all(|b| b.is_ascii_digit()))
}

const ASSERT_MARKS: &[&str] = &[
    "assert", "expect(", "expect!", "require.", "t.Error", "t.Fatal", ".should", "verify(", "pytest.raises", "assertThat",
    "toBe(", "toEqual(", "check(",
];
const FIXTURE_MARKS: &[&str] = &[
    "setUp", "beforeEach", "beforeAll", "fixture", "t.Run(", "t.Helper()", "tempfile", "tempdir", "TempDir", "t.TempDir",
    "mock", "Mock", "httptest.", "monkeypatch", "tmp_path", "@Before", "spyOn", "jest.fn",
];

fn marked_lines(code: &str, marks: &[&str]) -> Vec<String> {
    code.lines().map(str::trim).filter(|l| !l.is_empty() && marks.iter().any(|m| l.contains(m))).map(str::to_string).collect()
}

/// A function name specific enough to search tests for.
fn distinctive(name: &str) -> bool {
    const COMMON: &[&str] = &[
        "new", "get", "set", "run", "main", "init", "from", "into", "call", "test", "len", "default", "clone", "drop",
        "next", "read", "write", "open", "close", "load", "save", "start", "stop", "parse", "build", "apply", "handle",
        "update", "create", "delete", "render", "<anonymous>", "__init__", "constructor", "setUp", "tearDown",
    ];
    name.len() >= 4 && !COMMON.contains(&name)
}

/// Bonus when a test's name contains the function's name (`TestParseConfig`, `test_parse_config`).
fn name_bonus(test_name: &str, fname: &str) -> f64 {
    let norm = |s: &str| s.chars().filter(|c| *c != '_').collect::<String>().to_ascii_lowercase();
    if norm(test_name).contains(&norm(fname)) { 0.1 } else { 0.0 }
}

/// File stems too generic to pair a test with across unrelated directories.
const GENERIC_STEMS: &[&str] = &["mod", "lib", "main", "index", "__init__"];

/// Whether `test_path` is the conventional test file for `prod_path`.
fn corresponding_test_file(lang: Lang, prod_path: &str, test_path: &str) -> bool {
    if prod_path == test_path {
        return lang == Lang::Rust; // `#[cfg(test)] mod tests` in the same file
    }
    let split = |p: &str| -> (String, String, String) {
        let (dir, file) = p.rsplit_once('/').unwrap_or(("", p));
        let (stem, ext) = file.split_once('.').unwrap_or((file, ""));
        (dir.to_string(), stem.to_string(), ext.to_string())
    };
    let (pdir, mut pstem, _) = split(prod_path);
    let (tdir, tstem, text) = split(test_path);
    if GENERIC_STEMS.contains(&pstem.as_str()) {
        let parent = |d: &str| d.rsplit_once('/').map_or("", |(p, _)| p).to_string();
        let near = tdir == pdir || (tdir.ends_with("__tests__") && parent(&tdir) == pdir);
        if !near {
            // `store/mod.rs`, `store/__init__.py`, `store/index.ts` stand for `store`.
            let dname = pdir.rsplit('/').next().unwrap_or("");
            let named_by_dir = matches!(pstem.as_str(), "mod" | "__init__" | "index");
            if !named_by_dir || dname.is_empty() || dname == "src" || GENERIC_STEMS.contains(&dname) {
                return false;
            }
            pstem = dname.to_string();
        }
    }
    match lang {
        Lang::Go => tdir == pdir && tstem == format!("{pstem}_test"),
        Lang::Rust => {
            [pstem.clone(), format!("{pstem}_test"), format!("{pstem}_tests"), format!("test_{pstem}")].contains(&tstem)
                || (tstem == "tests" && tdir.ends_with(&pstem))
        }
        Lang::Python => [format!("test_{pstem}"), format!("{pstem}_test")].contains(&tstem),
        Lang::Java => [format!("{pstem}Test"), format!("{pstem}Tests"), format!("{pstem}IT"), format!("Test{pstem}")].contains(&tstem),
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => {
            tstem == pstem && (text.starts_with("test.") || text.starts_with("spec.") || tdir.ends_with("__tests__"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_rules() {
        assert_eq!(Confidence::of(1, 1), Confidence::Low);
        assert_eq!(Confidence::of(2, 1), Confidence::Medium);
        assert_eq!(Confidence::of(5, 1), Confidence::Medium);
        assert_eq!(Confidence::of(3, 2), Confidence::High);
        assert!(Confidence::High > Confidence::Medium && Confidence::Medium > Confidence::Low);
        assert_eq!("High".parse::<Confidence>().unwrap(), Confidence::High);
        assert_eq!(Confidence::Medium.to_string(), "medium");
    }

    #[test]
    fn guard_classification() {
        use GuardKind::*;
        let cases: &[(Lang, &str, Option<GuardKind>)] = &[
            (Lang::Go, "if err != nil {", Some(ErrorCheck)),
            (Lang::Go, "defer f.Close()", Some(Cleanup)),
            (Lang::Go, "defer mu.Unlock()", Some(Cleanup)),
            (Lang::Go, "if user == nil {", Some(NullCheck)),
            (Lang::Go, "if len(items) == 0 {", Some(BoundsCheck)),
            (Lang::Go, "log.Printf(\"loaded %d\", n)", Some(Logging)),
            (Lang::Go, "return errors.New(\"empty\")", Some(Validation)),
            (Lang::Rust, "if let Err(e) = r {", Some(ErrorCheck)),
            (Lang::Rust, "if idx >= self.items.len() {", Some(BoundsCheck)),
            (Lang::Rust, "ensure!(n > 0, \"n must be positive\");", Some(Validation)),
            (Lang::Rust, "tracing::warn!(\"slow\");", Some(Logging)),
            (Lang::Python, "if x is None:", Some(NullCheck)),
            (Lang::Python, "with open(p) as f:", Some(Cleanup)),
            (Lang::Python, "except ValueError:", Some(ErrorCheck)),
            (Lang::Python, "raise ValueError(\"bad\")", Some(Validation)),
            (Lang::Java, "if (conn == null) {", Some(NullCheck)),
            (Lang::Java, "} catch (IOException e) {", Some(ErrorCheck)),
            (Lang::JavaScript, "if (!user) {", Some(NullCheck)),
            (Lang::JavaScript, "console.error(e);", Some(Logging)),
            (Lang::Go, "x := compute(y)", None),
            (Lang::Go, "}", None),
            (Lang::Java, "return total;", None),
        ];
        for (lang, line, want) in cases {
            assert_eq!(classify_guard(*lang, line), *want, "{lang}: {line}");
        }
    }

    #[test]
    fn draft_splitting() {
        let d = split_draft(Lang::Go, "func Load(p string) error {\n\tf, err := os.Open(p)\n\treturn f.Close()\n}");
        assert_eq!(d.name.as_deref(), Some("Load"));
        assert_eq!(d.query, "\tf, err := os.Open(p)\n\treturn f.Close()");
        let d = split_draft(Lang::Python, "def load(p):\n    f = open(p)\n    return f.read()\n");
        assert_eq!(d.name.as_deref(), Some("load"));
        assert_eq!(d.query, "    f = open(p)\n    return f.read()");
        let d = split_draft(Lang::Go, "f, err := os.Open(p)\nx := 1");
        assert_eq!((d.name, d.query.as_str()), (None, "f, err := os.Open(p)\nx := 1"));
    }

    #[test]
    fn test_file_correspondence() {
        assert!(corresponding_test_file(Lang::Go, "pkg/store.go", "pkg/store_test.go"));
        assert!(!corresponding_test_file(Lang::Go, "pkg/store.go", "other/store_test.go"));
        assert!(corresponding_test_file(Lang::Python, "app/store.py", "tests/test_store.py"));
        assert!(corresponding_test_file(Lang::Java, "src/main/java/a/Store.java", "src/test/java/a/StoreTest.java"));
        assert!(corresponding_test_file(Lang::TypeScript, "src/store.ts", "src/store.test.ts"));
        assert!(corresponding_test_file(Lang::Rust, "src/store.rs", "src/store.rs"));
        assert!(corresponding_test_file(Lang::Rust, "src/store.rs", "tests/store.rs"));
        assert!(!corresponding_test_file(Lang::Python, "app/store.py", "tests/test_other.py"));
        assert!(distinctive("parse_config") && !distinctive("new") && !distinctive("get"));
        // M6: generic stems never pair across unrelated directories.
        assert!(!corresponding_test_file(Lang::Rust, "a/mod.rs", "b/mod.rs"));
        assert!(!corresponding_test_file(Lang::Rust, "x/src/lib.rs", "y/tests/lib.rs"));
        assert!(!corresponding_test_file(Lang::Python, "pkg/a/__init__.py", "tests/test___init__.py"));
        assert!(!corresponding_test_file(Lang::TypeScript, "a/index.ts", "b/index.test.ts"));
        assert!(corresponding_test_file(Lang::TypeScript, "a/index.ts", "a/index.test.ts"));
        assert!(corresponding_test_file(Lang::TypeScript, "a/index.ts", "a/__tests__/index.ts"));
        assert!(corresponding_test_file(Lang::Rust, "src/store/mod.rs", "tests/store.rs"));
        assert!(corresponding_test_file(Lang::Python, "pkg/store/__init__.py", "tests/test_store.py"));
    }

    #[test]
    fn guard_identifier_extraction() {
        let ids = |h: &str| guard_identifiers(h).collect::<Vec<_>>();
        assert_eq!(ids("if err != nil {"), ["err"]);
        assert_eq!(ids("defer res.Close()"), ["res", "Close"]);
        assert_eq!(ids("log.Printf(\"loaded %d\", n)"), ["log", "Printf", "n"]);
        assert!(ids("try {").is_empty() && ids("if x == 0 {") == ["x"]);
    }

    #[test]
    fn has_word_is_utf8_safe() {
        assert!(!has_word("xéval_config(1)", "éval_config"));
        assert!(has_word("let a = éval_config(1);", "éval_config"));
        assert!(!has_word("ééval_config()", "éval_config"));
        assert!(has_word("ü + éval_config", "éval_config"));
        assert!(!has_word("aé", "é"));
        assert!(!has_word("anything", ""));
        assert!(has_word("load(x)", "load") && !has_word("reload(x)", "load") && !has_word("load_all()", "load"));
    }
}
