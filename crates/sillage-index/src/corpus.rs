//! Corpus construction (paper §4.1): walk a directory of projects, extract
//! methods, and remove duplicates at project, file and method level by hash.
//!
//! The pipeline is staged so [`IncrementalIndex`](crate::IncrementalIndex) can
//! reuse it: walk → read + hash → project / file dedup → per-file extraction
//! ([`FileUnit`]: methods, imports, test flags) → assembly (method sort, method
//! dedup, `MethodId` and `file_id` assignment). A full build and an incremental
//! refresh over the same files therefore produce identical corpora.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use sillage_core::{Featurizer, Lang, MethodId, MethodRecord};

use crate::bm25::Bm25;
use crate::hash::content_hash;
use crate::index::{Bag, InvertedIndex, feature_bag};

/// Files larger than this are skipped (generated / minified sources).
pub(crate) const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Corpus {
    pub methods: Vec<MethodRecord>,
    /// Import statements of each source file that contributed a method,
    /// deduplicated within the file, indexed by `MethodRecord::file_id`.
    pub file_imports: Vec<Vec<Box<str>>>,
}

impl Corpus {
    /// Checks the invariants the engine relies on for a corpus loaded from disk:
    /// `methods[i].id == MethodId(i)`, every body span lies inside its text on
    /// UTF-8 character boundaries, and every `file_id` indexes `file_imports`.
    pub fn validate(&self) -> anyhow::Result<()> {
        let files = self.file_imports.len();
        for (i, m) in self.methods.iter().enumerate() {
            let expected = u32::try_from(i).map_err(|_| anyhow::anyhow!("more than u32::MAX methods"))?;
            anyhow::ensure!(m.id == MethodId(expected), "method #{i} has id {} (expected M{i})", m.id);
            let (s, e) = (m.body.start as usize, m.body.end as usize);
            anyhow::ensure!(
                s <= e && e <= m.text.len(),
                "{}: body span {s}..{e} is outside its {}-byte text",
                m.id,
                m.text.len()
            );
            anyhow::ensure!(
                m.text.is_char_boundary(s) && m.text.is_char_boundary(e),
                "{}: body span {s}..{e} is not on UTF-8 character boundaries",
                m.id
            );
            anyhow::ensure!(
                (m.file_id as usize) < files,
                "{}: file_id {} is out of range ({files} files in the import table)",
                m.id,
                m.file_id
            );
        }
        Ok(())
    }

    /// The deduplicated imports of `m`'s file (empty for an unknown `file_id`).
    pub fn imports_of(&self, m: &MethodRecord) -> &[Box<str>] {
        self.file_imports.get(m.file_id as usize).map_or(&[], Vec::as_slice)
    }
}

/// What a corpus walk saw besides the extracted methods.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CorpusStats {
    /// Source files (of the requested languages) read successfully.
    pub files: usize,
    /// Directory entries the walker could not read (permissions, broken links, …).
    pub walk_errors: usize,
    /// Source files that could not be read or are not valid UTF-8.
    pub read_errors: usize,
    /// Source files skipped for exceeding the size limit (generated / minified code).
    pub skipped_large: usize,
}

/// A corpus with its Phase I index and BM25 baseline, ready for an `Engine`.
#[derive(Debug, Clone, Default)]
pub struct IndexParts {
    pub corpus: Corpus,
    pub index: InvertedIndex,
    pub bm25: Bm25,
}

/// Builds a corpus from `root`. Each top-level subdirectory of `root` is a
/// project (`repo`); files directly under `root` belong to repo `"."`.
/// Ordering is deterministic: methods are sorted by `(repo, path, line_start, name)`
/// and assigned `MethodId(0..)`; the first occurrence of each duplicate is kept.
///
/// Fails when `root` does not exist or is not a readable directory; unreadable
/// entries below it are counted in [`CorpusStats`].
pub fn build_corpus(root: &Path, langs: &[Lang]) -> anyhow::Result<(Corpus, CorpusStats)> {
    let (candidates, stats) = walk(root, langs)?;
    let (units, stats) = read_and_extract(root, candidates, stats, |_| ());
    let refs: Vec<&FileUnit<()>> = units.iter().collect();
    let (corpus, _) = assemble(&refs, &dedup(&refs));
    Ok((corpus, stats))
}

/// [`build_corpus`] and [`InvertedIndex::build`] in one pass: each method's tree
/// from extraction is featurized directly instead of being re-parsed, giving
/// exactly the same corpus and index.
pub fn build_corpus_and_index(
    root: &Path,
    langs: &[Lang],
    featurizer: &dyn Featurizer,
) -> anyhow::Result<(Corpus, InvertedIndex, CorpusStats)> {
    let (candidates, stats) = walk(root, langs)?;
    let (units, stats) = read_and_extract(root, candidates, stats, |m| feature_bag(&m.tree, featurizer));
    let refs: Vec<&FileUnit<Bag>> = units.iter().collect();
    let (corpus, pos) = assemble(&refs, &dedup(&refs));
    let bags = take_payloads(units, &pos);
    let n = corpus.methods.len();
    Ok((corpus, InvertedIndex::from_bags(bags, n), stats))
}

/// Indexes a whole directory: corpus, Phase I index and BM25 in one call.
pub fn index_dir(root: &Path, langs: &[Lang], featurizer: &dyn Featurizer) -> anyhow::Result<(IndexParts, CorpusStats)> {
    let (corpus, index, stats) = build_corpus_and_index(root, langs, featurizer)?;
    let bm25 = Bm25::build(&corpus);
    Ok((IndexParts { corpus, index, bm25 }, stats))
}

/// Indexes an explicit list of files under `root` (absolute, or relative to
/// `root`), with the same project / file / method deduplication and ordering
/// as [`index_dir`]. Files outside `root` (or with a `..` component), in other
/// languages, missing or oversized are skipped (missing / oversized ones are
/// counted in [`CorpusStats`]).
pub fn index_files(
    root: &Path,
    files: &[PathBuf],
    langs: &[Lang],
    featurizer: &dyn Featurizer,
) -> anyhow::Result<(IndexParts, CorpusStats)> {
    check_root(root)?;
    let mut stats = CorpusStats::default();
    let mut candidates = Vec::new();
    for f in files {
        let abs = if f.is_absolute() { f.clone() } else { root.join(f) };
        let Some(lang) = sillage_parse::lang_from_path(&abs).filter(|l| langs.contains(l)) else { continue };
        // Lexically under `root`, with no `..` escaping it.
        if abs.strip_prefix(root).is_err() || abs.components().any(|c| c == Component::ParentDir) {
            continue;
        }
        match std::fs::metadata(&abs) {
            Ok(m) if m.is_file() && m.len() <= MAX_FILE_BYTES => candidates.push((abs, lang)),
            Ok(m) if m.is_file() => stats.skipped_large += 1,
            _ => stats.read_errors += 1,
        }
    }
    candidates.sort();
    candidates.dedup();
    let (units, stats) = read_and_extract(root, candidates, stats, |m| feature_bag(&m.tree, featurizer));
    let refs: Vec<&FileUnit<Bag>> = units.iter().collect();
    let (corpus, pos) = assemble(&refs, &dedup(&refs));
    let bags = take_payloads(units, &pos);
    let n = corpus.methods.len();
    let index = InvertedIndex::from_bags(bags, n);
    let bm25 = Bm25::build(&corpus);
    Ok((IndexParts { corpus, index, bm25 }, stats))
}

pub(crate) fn check_root(root: &Path) -> anyhow::Result<()> {
    let meta = std::fs::metadata(root).with_context(|| format!("corpus root {} is not accessible", root.display()))?;
    anyhow::ensure!(meta.is_dir(), "corpus root {} is not a directory", root.display());
    std::fs::read_dir(root).with_context(|| format!("corpus root {} is not readable", root.display()))?;
    Ok(())
}

/// A walked source file: `(absolute path, language)`, sorted.
pub(crate) type Candidate = (PathBuf, Lang);

pub(crate) fn walk(root: &Path, langs: &[Lang]) -> anyhow::Result<(Vec<Candidate>, CorpusStats)> {
    walk_with_meta(root, langs).map(|(c, s)| (c.into_iter().map(|(p, l, _)| (p, l)).collect(), s))
}

/// [`walk`] keeping each file's metadata (for incremental change detection).
pub(crate) fn walk_with_meta(
    root: &Path,
    langs: &[Lang],
) -> anyhow::Result<(Vec<(PathBuf, Lang, std::fs::Metadata)>, CorpusStats)> {
    check_root(root)?;
    let mut stats = CorpusStats::default();
    let mut candidates = Vec::new();
    for entry in ignore::WalkBuilder::new(root).build() {
        let Ok(e) = entry else {
            stats.walk_errors += 1;
            continue;
        };
        if !e.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(lang) = sillage_parse::lang_from_path(e.path()).filter(|l| langs.contains(l)) else {
            continue;
        };
        match e.metadata() {
            Ok(m) if m.len() <= MAX_FILE_BYTES => candidates.push((e.into_path(), lang, m)),
            Ok(_) => stats.skipped_large += 1,
            Err(_) => stats.walk_errors += 1,
        }
    }
    candidates.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
    Ok((candidates, stats))
}

/// One source file after extraction. `methods` carry `id = 0` and `file_id = 0`
/// until [`assemble`] numbers them.
#[derive(Debug, Clone)]
pub(crate) struct FileUnit<T> {
    pub repo: Box<str>,
    /// Path relative to the repo directory, `/`-separated.
    pub path: Box<str>,
    pub hash: u64,
    pub imports: Vec<Box<str>>,
    pub methods: Vec<(MethodRecord, T)>,
}

/// Reads, hashes and extracts every candidate (in parallel); unreadable or
/// non-UTF-8 files are counted in `stats.read_errors`. Units are sorted by
/// `(repo, path)`.
pub(crate) fn read_and_extract<T: Send>(
    root: &Path,
    candidates: Vec<Candidate>,
    mut stats: CorpusStats,
    payload: impl Fn(&sillage_parse::ExtractedMethod) -> T + Sync,
) -> (Vec<FileUnit<T>>, CorpusStats) {
    let read: Vec<Option<FileUnit<T>>> = candidates
        .into_par_iter()
        .filter_map(|(abs, lang)| {
            let rel = abs.strip_prefix(root).ok()?;
            let (repo, path) = split_repo(rel)?;
            let Ok(src) = std::fs::read_to_string(&abs) else {
                return Some(None);
            };
            Some(Some(extract_unit(repo, path, lang, &src, &payload)))
        })
        .collect();
    stats.read_errors += read.iter().filter(|f| f.is_none()).count();
    let mut units: Vec<FileUnit<T>> = read.into_iter().flatten().collect();
    stats.files += units.len();
    units.sort_by(|a, b| (&a.repo, &a.path).cmp(&(&b.repo, &b.path)));
    (units, stats)
}

/// Extracts one file's methods (with test flags) and imports.
pub(crate) fn extract_unit<T>(
    repo: Box<str>,
    path: Box<str>,
    lang: Lang,
    src: &str,
    payload: &impl Fn(&sillage_parse::ExtractedMethod) -> T,
) -> FileUnit<T> {
    let hash = content_hash(src.as_bytes());
    let full_path = if &*repo == "." { path.to_string() } else { format!("{repo}/{path}") };
    let methods = sillage_parse::extract_methods(lang, src)
        .into_iter()
        .filter_map(|m| {
            let text = src.get(m.text_span.start as usize..m.text_span.end as usize)?;
            let is_test = m.test_context || sillage_parse::is_test_method(lang, &full_path, &m.name, text);
            let rec = MethodRecord {
                id: MethodId(0),
                lang,
                repo: repo.clone(),
                path: path.clone(),
                name: m.name.clone(),
                line_start: m.line_start,
                text: Arc::from(text),
                body: m.body_span,
                is_test,
                file_id: 0,
            };
            Some((rec, payload(&m)))
        })
        .collect();
    let imports = sillage_parse::extract_imports(lang, src);
    FileUnit { repo, path, hash, imports, methods }
}

/// Project- then file-level deduplication of `(repo, path)`-sorted units:
/// the indices of the units kept, in order.
pub(crate) fn dedup<T>(units: &[&FileUnit<T>]) -> Vec<usize> {
    dedup_files(units, dedup_projects(units))
}

/// Sorts, deduplicates and numbers the methods of `units[kept]` (`kept` from
/// [`dedup`], `units` sorted by `(repo, path)`). Each file that keeps at least
/// one method gets a `file_id` in order of first appearance. Returns the
/// corpus and, per `MethodId`, the `(unit, method)` position it came from.
pub(crate) fn assemble<T>(units: &[&FileUnit<T>], kept: &[usize]) -> (Corpus, Vec<(usize, usize)>) {
    // (unit index, method index) in file order, then a stable method sort.
    let mut order: Vec<(usize, usize)> =
        kept.iter().flat_map(|&u| (0..units[u].methods.len()).map(move |m| (u, m))).collect();
    let rec = |&(u, m): &(usize, usize)| &units[u].methods[m].0;
    order.sort_by(|a, b| {
        let (a, b) = (rec(a), rec(b));
        (&a.repo, &a.path, a.line_start, &a.name).cmp(&(&b.repo, &b.path, b.line_start, &b.name))
    });
    let mut seen = FxHashSet::default();
    order.retain(|o| seen.insert(method_hash(rec(o))));

    let mut file_ids: FxHashMap<usize, u32> = FxHashMap::default();
    let mut file_imports: Vec<Vec<Box<str>>> = Vec::new();
    let mut methods = Vec::with_capacity(order.len());
    for (i, &(u, m)) in order.iter().enumerate() {
        let fid = *file_ids.entry(u).or_insert_with(|| {
            file_imports.push(units[u].imports.clone());
            u32::try_from(file_imports.len() - 1).expect("more than u32::MAX files")
        });
        let mut r = units[u].methods[m].0.clone();
        r.id = MethodId(u32::try_from(i).expect("corpus has more than u32::MAX methods"));
        r.file_id = fid;
        methods.push(r);
    }
    (Corpus { methods, file_imports }, order)
}

/// Moves the payloads at `pos` (from [`assemble`]) out of `units`, as `(MethodId, payload)`.
pub(crate) fn take_payloads<T>(units: Vec<FileUnit<T>>, pos: &[(usize, usize)]) -> Vec<(MethodId, T)> {
    let mut slots: Vec<Vec<Option<T>>> =
        units.into_iter().map(|u| u.methods.into_iter().map(|(_, p)| Some(p)).collect()).collect();
    pos.iter()
        .enumerate()
        .map(|(i, &(u, m))| (MethodId(i as u32), slots[u][m].take().expect("each position is taken once")))
        .collect()
}

pub(crate) fn split_repo(rel: &Path) -> Option<(Box<str>, Box<str>)> {
    let parts: Vec<&str> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    match parts.as_slice() {
        [] => None,
        [file] => Some((".".into(), (*file).into())),
        [repo, rest @ ..] => Some(((*repo).into(), rest.join("/").into())),
    }
}

/// Drops whole projects whose (path, content) fingerprint equals an earlier
/// project's; returns the indices of the files kept.
fn dedup_projects<T>(files: &[&FileUnit<T>]) -> Vec<usize> {
    let mut keep_repos = FxHashSet::default();
    let mut seen = FxHashSet::default();
    let mut i = 0;
    while i < files.len() {
        let repo = &files[i].repo;
        let mut buf = Vec::new();
        let mut j = i;
        while j < files.len() && &files[j].repo == repo {
            buf.extend_from_slice(&(files[j].path.len() as u32).to_le_bytes());
            buf.extend_from_slice(files[j].path.as_bytes());
            buf.extend_from_slice(&files[j].hash.to_le_bytes());
            j += 1;
        }
        if seen.insert(content_hash(&buf)) {
            keep_repos.insert(repo.clone());
        }
        i = j;
    }
    (0..files.len()).filter(|&i| keep_repos.contains(&files[i].repo)).collect()
}

/// Drops files whose content hash equals an earlier kept file's.
fn dedup_files<T>(files: &[&FileUnit<T>], kept: Vec<usize>) -> Vec<usize> {
    let mut seen = FxHashSet::default();
    kept.into_iter().filter(|&i| seen.insert(files[i].hash)).collect()
}

/// Hash of the method body with all whitespace removed, keyed by language.
pub(crate) fn method_hash(m: &MethodRecord) -> u64 {
    let body = m.body_text();
    let mut buf = Vec::with_capacity(body.len() + 1);
    buf.push(m.lang as u8);
    for c in body.chars().filter(|c| !c.is_whitespace()) {
        let mut tmp = [0u8; 4];
        buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
    }
    content_hash(&buf)
}

/// Corpus, index and BM25 from cached units (the incremental builder), cloning
/// the cached feature bags.
pub(crate) fn build_parts(units: &[&FileUnit<Bag>]) -> IndexParts {
    let kept = dedup(units);
    let (corpus, pos) = assemble(units, &kept);
    let bags: Vec<(MethodId, Bag)> =
        pos.iter().enumerate().map(|(i, &(u, m))| (MethodId(i as u32), units[u].methods[m].1.clone())).collect();
    let n = corpus.methods.len();
    let index = InvertedIndex::from_bags(bags, n);
    let bm25 = Bm25::build(&corpus);
    IndexParts { corpus, index, bm25 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_repo_paths() {
        assert_eq!(split_repo(Path::new("a.java")), Some((".".into(), "a.java".into())));
        assert_eq!(
            split_repo(Path::new("proj/src/x/A.java")),
            Some(("proj".into(), "src/x/A.java".into()))
        );
    }

    #[test]
    fn method_hash_ignores_whitespace_and_name() {
        let mk = |name: &str, text: &str, body: (u32, u32)| MethodRecord {
            id: MethodId(0),
            lang: Lang::Java,
            repo: "r".into(),
            path: "p".into(),
            name: name.into(),
            line_start: 1,
            text: Arc::from(text),
            body: sillage_core::Span::new(body.0, body.1),
            is_test: false,
            file_id: 0,
        };
        let a = mk("f", "void f() { return 1; }", (10, 21));
        let b = mk("g", "void g() {\n  return 1;\n}", (10, 23));
        let c = mk("h", "void h() { return 2; }", (10, 21));
        assert_eq!(method_hash(&a), method_hash(&b));
        assert_ne!(method_hash(&a), method_hash(&c));
    }

    #[test]
    fn validate_rejects_out_of_range_file_ids() {
        let m = MethodRecord {
            id: MethodId(0),
            lang: Lang::Java,
            repo: "r".into(),
            path: "p".into(),
            name: "f".into(),
            line_start: 1,
            text: Arc::from("void f() {}"),
            body: sillage_core::Span::new(9, 11),
            is_test: false,
            file_id: 0,
        };
        let mut c = Corpus { methods: vec![m], file_imports: vec![vec![]] };
        c.validate().unwrap();
        c.methods[0].file_id = 1;
        assert!(c.validate().unwrap_err().to_string().contains("file_id"));
        c.file_imports.clear();
        c.methods[0].file_id = 0;
        assert!(c.validate().is_err());
        assert!(c.imports_of(&c.methods[0]).is_empty());
    }
}
