//! `corpus.toml` manifest parsing and shallow git fetching.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    #[serde(rename = "repo", default)]
    pub repos: Vec<RepoSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepoSpec {
    pub name: String,
    pub url: String,
    /// Pinned commit SHA (or tag) to check out.
    #[serde(default)]
    pub rev: Option<String>,
    /// Branch or tag to clone when no `rev` is pinned.
    #[serde(default)]
    pub branch: Option<String>,
    /// Human-readable release the `rev` corresponds to.
    #[serde(default)]
    pub tag: Option<String>,
    pub lang: String,
}

impl Manifest {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let m: Manifest = toml::from_str(text).context("parsing corpus manifest")?;
        for r in &m.repos {
            r.lang
                .parse::<sillage_core::Lang>()
                .with_context(|| format!("repo {}: bad lang", r.name))?;
            if r.name.is_empty() || r.name.contains(['/', '\\']) || r.name.starts_with('.') {
                bail!("repo name {:?} must be a plain directory name", r.name);
            }
            if r.rev.as_deref().is_some_and(|v| v.starts_with('-'))
                || r.branch.as_deref().is_some_and(|v| v.starts_with('-'))
                || r.url.starts_with('-')
            {
                bail!("repo {}: url/rev/branch must not start with '-'", r.name);
            }
        }
        Ok(m)
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading manifest {}", path.display()))?;
        Self::parse(&text)
    }
}

fn git(dir: Option<&Path>, args: &[&str]) -> anyhow::Result<String> {
    let mut cmd = Command::new("git");
    if let Some(d) = dir {
        cmd.arg("-C").arg(d);
    }
    cmd.args(args);
    let out = cmd.output().context("running git")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Shallow-clones (or updates) `spec` under `out`, returning `(dir, HEAD sha)`.
pub fn fetch_repo(spec: &RepoSpec, out: &Path) -> anyhow::Result<(PathBuf, String)> {
    let dir = out.join(&spec.name);
    if !dir.join(".git").exists() {
        std::fs::create_dir_all(out)?;
        let dir_s = dir.to_string_lossy().to_string();
        let mut args = vec!["clone", "--depth", "1", "--quiet"];
        if spec.rev.is_none() {
            if let Some(b) = spec.branch.as_deref() {
                args.extend(["--branch", b]);
            }
        }
        args.extend(["--", spec.url.as_str(), dir_s.as_str()]);
        git(None, &args)?;
    }
    if let Some(rev) = spec.rev.as_deref() {
        let head = git(Some(&dir), &["rev-parse", "HEAD"]).unwrap_or_default();
        if head != rev {
            git(Some(&dir), &["fetch", "--depth", "1", "--quiet", "origin", rev])?;
            git(Some(&dir), &["checkout", "--quiet", "--detach", "FETCH_HEAD"])?;
        }
    } else if let Some(b) = spec.branch.as_deref() {
        git(Some(&dir), &["fetch", "--depth", "1", "--quiet", "origin", b])?;
        git(Some(&dir), &["checkout", "--quiet", "--detach", "FETCH_HEAD"])?;
    }
    let sha = git(Some(&dir), &["rev-parse", "HEAD"])?;
    Ok((dir, sha))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repo_entries() {
        let m = Manifest::parse(
            r#"
[[repo]]
name = "jsoup"
url = "https://github.com/jhy/jsoup"
rev = "abc"
lang = "java"

[[repo]]
name = "click"
url = "https://github.com/pallets/click"
branch = "main"
lang = "python"
"#,
        )
        .unwrap();
        assert_eq!(m.repos.len(), 2);
        assert_eq!(m.repos[0].rev.as_deref(), Some("abc"));
        assert_eq!(m.repos[1].branch.as_deref(), Some("main"));
    }

    #[test]
    fn rejects_bad_entries() {
        let bad_lang = "[[repo]]\nname = \"x\"\nurl = \"u\"\nlang = \"cobol\"\n";
        assert!(Manifest::parse(bad_lang).is_err());
        let bad_name = "[[repo]]\nname = \"../x\"\nurl = \"u\"\nlang = \"java\"\n";
        assert!(Manifest::parse(bad_name).is_err());
        let bad_rev = "[[repo]]\nname = \"x\"\nurl = \"u\"\nrev = \"--upload-pack=x\"\nlang = \"java\"\n";
        assert!(Manifest::parse(bad_rev).is_err());
    }

    #[test]
    fn repository_corpus_manifest_is_valid() {
        let m = Manifest::parse(include_str!("../../../corpus.toml")).unwrap();
        assert!(m.repos.len() >= 10);
        for name in ["jsoup", "junit4", "commons-io", "commons-lang", "retrofit", "guava"] {
            let r = m.repos.iter().find(|r| r.name == name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(r.lang, "java");
            assert_eq!(r.rev.as_deref().map(str::len), Some(40), "{name} pinned to a full SHA");
        }
        for lang in ["python", "javascript", "typescript", "go", "rust"] {
            assert!(m.repos.iter().any(|r| r.lang == lang), "{lang}");
        }
    }
}
