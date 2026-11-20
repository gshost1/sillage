//! Corpus construction (paper §4.1): walk a directory of projects, extract
//! methods, and remove duplicates at project, file and method level by hash.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};
use sillage_core::{Lang, MethodId, MethodRecord};

use crate::hash::content_hash;

/// Files larger than this are skipped (generated / minified sources).
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Corpus {
    pub methods: Vec<MethodRecord>,
}

struct SourceFile {
    repo: Box<str>,
    /// Path relative to the repo directory, `/`-separated.
    path: Box<str>,
    lang: Lang,
    src: String,
    hash: u64,
}

/// Builds a corpus from `root`. Each top-level subdirectory of `root` is a
/// project (`repo`); files directly under `root` belong to repo `"."`.
/// Ordering is deterministic: methods are sorted by `(repo, path, line_start, name)`
/// and assigned `MethodId(0..)`; the first occurrence of each duplicate is kept.
pub fn build_corpus(root: &Path, langs: &[Lang]) -> Corpus {
    let mut candidates: Vec<(PathBuf, Lang)> = ignore::WalkBuilder::new(root)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter(|e| e.metadata().map(|m| m.len() <= MAX_FILE_BYTES).unwrap_or(false))
        .filter_map(|e| {
            let lang = sillage_parse::lang_from_path(e.path())?;
            langs.contains(&lang).then(|| (e.into_path(), lang))
        })
        .collect();
    candidates.sort();

    let mut files: Vec<SourceFile> = candidates
        .into_par_iter()
        .filter_map(|(abs, lang)| {
            let rel = abs.strip_prefix(root).ok()?;
            let (repo, path) = split_repo(rel)?;
            let src = std::fs::read_to_string(&abs).ok()?;
            let hash = content_hash(src.as_bytes());
            Some(SourceFile { repo, path, lang, src, hash })
        })
        .collect();
    files.sort_by(|a, b| (&a.repo, &a.path).cmp(&(&b.repo, &b.path)));

    let files = dedup_projects(files);
    let files = dedup_files(files);

    let mut methods: Vec<MethodRecord> = files
        .par_iter()
        .flat_map_iter(|f| {
            sillage_parse::extract_methods(f.lang, &f.src)
                .into_iter()
                .filter_map(|m| {
                    let text = f.src.get(m.text_span.start as usize..m.text_span.end as usize)?;
                    Some(MethodRecord {
                        id: MethodId(0),
                        lang: f.lang,
                        repo: f.repo.clone(),
                        path: f.path.clone(),
                        name: m.name,
                        line_start: m.line_start,
                        text: Arc::from(text),
                        body: m.body_span,
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect();
    methods.sort_by(|a, b| {
        (&a.repo, &a.path, a.line_start, &a.name).cmp(&(&b.repo, &b.path, b.line_start, &b.name))
    });

    let mut seen = FxHashSet::default();
    methods.retain(|m| seen.insert(method_hash(m)));
    for (i, m) in methods.iter_mut().enumerate() {
        m.id = MethodId(i as u32);
    }
    Corpus { methods }
}

fn split_repo(rel: &Path) -> Option<(Box<str>, Box<str>)> {
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

/// Drops whole projects whose (path, content) fingerprint equals an earlier project's.
fn dedup_projects(files: Vec<SourceFile>) -> Vec<SourceFile> {
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
    files.into_iter().filter(|f| keep_repos.contains(&f.repo)).collect()
}

/// Drops files whose content hash equals an earlier file's.
fn dedup_files(files: Vec<SourceFile>) -> Vec<SourceFile> {
    let mut seen = FxHashSet::default();
    files.into_iter().filter(|f| seen.insert(f.hash)).collect()
}

/// Hash of the method body with all whitespace removed, keyed by language.
fn method_hash(m: &MethodRecord) -> u64 {
    let text: &str = &m.text;
    let body = text.get(m.body.start as usize..m.body.end as usize).unwrap_or(text);
    let mut buf = Vec::with_capacity(body.len() + 1);
    buf.push(m.lang as u8);
    for c in body.chars().filter(|c| !c.is_whitespace()) {
        let mut tmp = [0u8; 4];
        buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
    }
    content_hash(&buf)
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
        };
        let a = mk("f", "void f() { return 1; }", (10, 21));
        let b = mk("g", "void g() {\n  return 1;\n}", (10, 23));
        let c = mk("h", "void h() { return 2; }", (10, 21));
        assert_eq!(method_hash(&a), method_hash(&b));
        assert_ne!(method_hash(&a), method_hash(&c));
    }
}
