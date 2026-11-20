use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::tree::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Java,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
    Rust,
}

impl Lang {
    pub const ALL: &'static [Lang] = &[
        Lang::Java,
        Lang::Python,
        Lang::JavaScript,
        Lang::TypeScript,
        Lang::Tsx,
        Lang::Go,
        Lang::Rust,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Lang::Java => "java",
            Lang::Python => "python",
            Lang::JavaScript => "javascript",
            Lang::TypeScript => "typescript",
            Lang::Tsx => "tsx",
            Lang::Go => "go",
            Lang::Rust => "rust",
        }
    }

    pub fn placeholder(self) -> &'static str {
        match self {
            Lang::Python => "...",
            _ => "// ...",
        }
    }
}

/// Whether a query in `query` may retrieve a method in `method`: the same
/// language, except that TypeScript and TSX share one family (a `.tsx` file is
/// TypeScript with JSX, and both use the same grammar for method bodies).
pub fn lang_compatible(query: Lang, method: Lang) -> bool {
    let family = |l: Lang| match l {
        Lang::Tsx => Lang::TypeScript,
        l => l,
    };
    family(query) == family(method)
}

impl fmt::Display for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Lang {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "java" => Ok(Lang::Java),
            "py" | "python" => Ok(Lang::Python),
            "js" | "jsx" | "mjs" | "cjs" | "javascript" => Ok(Lang::JavaScript),
            "ts" | "mts" | "cts" | "typescript" => Ok(Lang::TypeScript),
            "tsx" => Ok(Lang::Tsx),
            "go" | "golang" => Ok(Lang::Go),
            "rs" | "rust" => Ok(Lang::Rust),
            other => anyhow::bail!("unsupported language: {other}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MethodId(pub u32);

impl fmt::Display for MethodId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "M{}", self.0)
    }
}

/// Which methods a query may retrieve: production code, test code, or both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MethodScope {
    #[default]
    All,
    Prod,
    Test,
}

impl MethodScope {
    pub fn as_str(self) -> &'static str {
        match self {
            MethodScope::All => "all",
            MethodScope::Prod => "prod",
            MethodScope::Test => "test",
        }
    }

    /// Whether a method with the given test flag is in this scope.
    pub fn admits(self, is_test: bool) -> bool {
        match self {
            MethodScope::All => true,
            MethodScope::Prod => !is_test,
            MethodScope::Test => is_test,
        }
    }
}

impl fmt::Display for MethodScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MethodScope {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "all" | "any" | "both" => Ok(MethodScope::All),
            "prod" | "production" | "src" | "main" => Ok(MethodScope::Prod),
            "test" | "tests" => Ok(MethodScope::Test),
            other => anyhow::bail!("unknown method scope: {other} (expected all, prod or test)"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodRecord {
    pub id: MethodId,
    pub lang: Lang,
    pub repo: Box<str>,
    pub path: Box<str>,
    pub name: Box<str>,
    pub line_start: u32,
    pub text: Arc<str>,
    pub body: Span,
    /// Test code (test file, test function or test module), see
    /// `sillage_parse::is_test_method`.
    pub is_test: bool,
    /// Index of the method's file in `Corpus::file_imports`.
    pub file_id: u32,
}

impl MethodRecord {
    /// Path relative to the corpus root: `repo/path`, or `path` for repo `"."`.
    pub fn rel_path(&self) -> String {
        if &*self.repo == "." { self.path.to_string() } else { format!("{}/{}", self.repo, self.path) }
    }

    /// 1-based last line of the method in its file.
    pub fn line_end(&self) -> u32 {
        self.line_start + self.text.matches('\n').count() as u32
    }

    /// `rel_path:line_start` citation.
    pub fn provenance(&self) -> String {
        format!("{}:{}", self.rel_path(), self.line_start)
    }

    /// The method body text (`text[body]`), or the whole text for an invalid span.
    pub fn body_text(&self) -> &str {
        let text: &str = &self.text;
        text.get(self.body.start as usize..self.body.end as usize).unwrap_or(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_scope_parse_display_admit() {
        for s in [MethodScope::All, MethodScope::Prod, MethodScope::Test] {
            assert_eq!(s.to_string().parse::<MethodScope>().unwrap(), s);
        }
        assert_eq!("Tests".parse::<MethodScope>().unwrap(), MethodScope::Test);
        assert!("nope".parse::<MethodScope>().is_err());
        assert_eq!(MethodScope::default(), MethodScope::All);
        assert!(MethodScope::All.admits(true) && MethodScope::All.admits(false));
        assert!(MethodScope::Prod.admits(false) && !MethodScope::Prod.admits(true));
        assert!(MethodScope::Test.admits(true) && !MethodScope::Test.admits(false));
    }

    #[test]
    fn record_paths() {
        let mut m = MethodRecord {
            id: MethodId(0),
            lang: Lang::Rust,
            repo: ".".into(),
            path: "src/a.rs".into(),
            name: "f".into(),
            line_start: 3,
            text: Arc::from("fn f() {\n  1\n}"),
            body: Span::new(7, 14),
            is_test: false,
            file_id: 0,
        };
        assert_eq!(m.rel_path(), "src/a.rs");
        assert_eq!(m.provenance(), "src/a.rs:3");
        assert_eq!(m.line_end(), 5);
        assert_eq!(m.body_text(), "{\n  1\n}");
        m.repo = "proj".into();
        assert_eq!(m.rel_path(), "proj/src/a.rs");
    }

    #[test]
    fn lang_families() {
        assert!(lang_compatible(Lang::Java, Lang::Java));
        assert!(lang_compatible(Lang::TypeScript, Lang::Tsx));
        assert!(lang_compatible(Lang::Tsx, Lang::TypeScript));
        assert!(!lang_compatible(Lang::JavaScript, Lang::TypeScript));
        assert!(!lang_compatible(Lang::Python, Lang::Java));
        for &a in Lang::ALL {
            for &b in Lang::ALL {
                assert_eq!(lang_compatible(a, b), lang_compatible(b, a));
            }
        }
    }
}
