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
}

#[cfg(test)]
mod tests {
    use super::*;

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
