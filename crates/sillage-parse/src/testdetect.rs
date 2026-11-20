//! Test-code detection by language conventions.
//!
//! [`is_test_method`] looks at the file path, the method name and the method
//! text (which includes Java annotations, and Rust attributes when a caller
//! passes them). Context that lives *outside* a method's text (a Rust
//! `#[test]` attribute or `#[cfg(test)] mod`, a Python `TestCase` class, a JS
//! `describe(...)` callback, a Java `*Test` class) is detected during extraction
//! ([`ExtractedMethod::test_context`](crate::ExtractedMethod::test_context)).

use sillage_core::Lang;
use tree_sitter::Node;

/// Whether a file path is a test file / lives in a test directory for `lang`.
/// Paths may be relative or absolute, with `/` or `\` separators.
pub fn is_test_path(lang: Lang, path: &str) -> bool {
    let norm = path.replace('\\', "/");
    let segs: Vec<&str> = norm.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    let Some((&file, dirs)) = segs.split_last() else { return false };
    let dir_is = |names: &[&str]| dirs.iter().any(|d| names.contains(d));
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    match lang {
        Lang::Rust => dir_is(&["tests", "benches"]) || stem.ends_with("_test") || stem.ends_with("_tests") || stem == "tests",
        Lang::Go => file.ends_with("_test.go"),
        Lang::Java => {
            dirs.windows(2).any(|w| w[0] == "src" && matches!(w[1], "test" | "androidTest" | "testFixtures" | "integrationTest"))
                || dir_is(&["test", "tests"])
                || stem.ends_with("Test")
                || stem.ends_with("Tests")
                || stem.ends_with("IT")
                || (stem.starts_with("Test") && stem.len() > 4 && stem[4..].starts_with(char::is_uppercase))
        }
        Lang::Python => {
            dir_is(&["tests", "test", "testing"])
                || stem.starts_with("test_")
                || stem.ends_with("_test")
                || stem == "conftest"
                || stem == "tests"
        }
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => {
            dir_is(&["__tests__", "__mocks__", "test", "tests", "spec", "e2e", "cypress"])
                || file.contains(".test.")
                || file.contains(".spec.")
                || file.contains(".e2e.")
                || stem.ends_with("_test")
        }
    }
}

/// Whether a method is test code: a test file ([`is_test_path`]), a test
/// function by name, or test markers in its text (`#[test]`, `@Test`,
/// `describe(` / `it(` / `test(` calls, Go `*testing.T` parameters, …).
pub fn is_test_method(lang: Lang, path: &str, name: &str, text: &str) -> bool {
    if is_test_path(lang, path) {
        return true;
    }
    match lang {
        Lang::Rust => rust_text_has_test_attr(text),
        Lang::Go => {
            let typed = ["*testing.T", "*testing.B", "*testing.F", "*testing.M", "testing.TB"].iter().any(|m| text.contains(m));
            typed && ["Test", "Benchmark", "Fuzz", "Example"].iter().any(|p| name.starts_with(p))
        }
        Lang::Java => {
            const ANN: &[&str] = &[
                "@Test", "@ParameterizedTest", "@RepeatedTest", "@TestFactory", "@TestTemplate", "@BeforeEach",
                "@AfterEach", "@BeforeAll", "@AfterAll", "@Before", "@After", "@BeforeClass", "@AfterClass",
            ];
            let head = text.split('{').next().unwrap_or(text);
            ANN.iter().any(|a| has_word(head, a))
        }
        Lang::Python => {
            name.starts_with("test_") || name == "test" || matches!(name, "setUp" | "tearDown" | "setUpClass" | "tearDownClass")
        }
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => {
            ["describe", "it", "test", "beforeEach", "afterEach", "beforeAll", "afterAll", "expect"]
                .iter()
                .any(|f| has_call(text, f))
        }
    }
}

/// `#[test]`, `#[tokio::test]`, `#[rstest]`, `#[test_case(..)]`, `#[bench]`,
/// `#[cfg(test)]` (not `cfg(not(test))`), … anywhere in `text`.
pub(crate) fn rust_text_has_test_attr(text: &str) -> bool {
    let mut rest = text;
    while let Some(i) = rest.find("#[") {
        let after = &rest[i + 2..];
        let end = after.find(']').unwrap_or(after.len());
        if rust_attr_is_test(&after[..end]) {
            return true;
        }
        rest = &after[end.min(after.len())..];
    }
    false
}

/// Attribute contents (between `#[` and `]`) that mark test code.
pub(crate) fn rust_attr_is_test(attr: &str) -> bool {
    let attr = attr.trim();
    let path = attr.split(['(', ' ', '=']).next().unwrap_or("").trim();
    let last = path.rsplit("::").next().unwrap_or(path);
    if matches!(last, "test" | "bench" | "rstest" | "quickcheck" | "test_case" | "test_log" | "wasm_bindgen_test" | "serial") {
        return true;
    }
    if last.starts_with("test_") {
        return true;
    }
    if path == "cfg" {
        let args = &attr[3..];
        return has_word(args, "test") && !args.replace(' ', "").contains("not(test");
    }
    false
}

/// Test context of an extracted method that its own text does not show.
pub(crate) fn node_test_context(lang: Lang, method: Node<'_>, src: &str) -> bool {
    let text = |n: Node<'_>| src.get(n.start_byte()..n.end_byte()).unwrap_or("");
    let mut cur = Some(method);
    while let Some(n) = cur {
        match lang {
            Lang::Rust => {
                if matches!(n.kind(), "function_item" | "mod_item" | "impl_item") && rust_preceding_attrs_test(n, src) {
                    return true;
                }
            }
            Lang::Python => match n.kind() {
                "class_definition" => {
                    let name = n.child_by_field_name("name").map(text).unwrap_or("");
                    let sup = n.child_by_field_name("superclasses").map(text).unwrap_or("");
                    if name.starts_with("Test") || sup.contains("TestCase") {
                        return true;
                    }
                }
                "decorated_definition" => {
                    let mut c = n.walk();
                    let decorated = n.children(&mut c).filter(|d| d.kind() == "decorator").any(|d| {
                        let t = text(d);
                        t.contains("pytest") || t.contains("fixture")
                    });
                    if decorated {
                        return true;
                    }
                }
                _ => {}
            },
            Lang::JavaScript | Lang::TypeScript | Lang::Tsx => {
                if n.kind() == "call_expression" && n.id() != method.id() {
                    let f = n.child_by_field_name("function").map(text).unwrap_or("");
                    let head = f.split('.').next().unwrap_or(f);
                    if matches!(head, "describe" | "it" | "test" | "suite" | "beforeEach" | "afterEach" | "beforeAll" | "afterAll") {
                        return true;
                    }
                }
            }
            Lang::Java => {
                if n.kind() == "class_declaration" && n.id() != method.id() {
                    let name = n.child_by_field_name("name").map(text).unwrap_or("");
                    if name.ends_with("Test") || name.ends_with("Tests") || name.ends_with("IT") {
                        return true;
                    }
                    let mut c = n.walk();
                    let annotated = n.children(&mut c).filter(|m| m.kind() == "modifiers").any(|m| {
                        let t = text(m);
                        ["@RunWith", "@ExtendWith", "@SpringBootTest", "@DataJpaTest", "@WebMvcTest"].iter().any(|a| t.contains(a))
                    });
                    if annotated {
                        return true;
                    }
                }
            }
            Lang::Go => {}
        }
        cur = n.parent();
    }
    false
}

/// The `attribute_item`s directly before `n` (comments skipped) mark test code.
fn rust_preceding_attrs_test(n: Node<'_>, src: &str) -> bool {
    let mut prev = n.prev_sibling();
    while let Some(p) = prev {
        match p.kind() {
            "attribute_item" => {
                let t = src.get(p.start_byte()..p.end_byte()).unwrap_or("");
                let inner = t.trim_start_matches("#[").trim_end_matches(']');
                if rust_attr_is_test(inner) {
                    return true;
                }
            }
            "line_comment" | "block_comment" => {}
            _ => break,
        }
        prev = p.prev_sibling();
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// `word` occurs in `s` not preceded or followed by an identifier character.
fn has_word(s: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let b = s.as_bytes();
    let mut from = 0;
    while let Some(i) = s[from..].find(word) {
        let start = from + i;
        let end = start + word.len();
        let before_ok = start == 0 || !is_ident_byte(b[start - 1]) || word.starts_with('@');
        let after_ok = end >= b.len() || !is_ident_byte(b[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + first_char_len(word);
    }
    false
}

/// Byte length of `s`'s first char: the step from one match to the next
/// search start that stays on a UTF-8 boundary.
fn first_char_len(s: &str) -> usize {
    s.chars().next().map_or(1, char::len_utf8)
}

/// `name(` (optionally `name.x(`) called as a free function: not `.name(`.
fn has_call(s: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let b = s.as_bytes();
    let mut from = 0;
    while let Some(i) = s[from..].find(name) {
        let start = from + i;
        let end = start + name.len();
        from = start + first_char_len(name);
        if start > 0 && (is_ident_byte(b[start - 1]) || b[start - 1] == b'.') {
            continue;
        }
        let rest = s[end..].trim_start();
        if rest.starts_with('(') {
            return true;
        }
        if let Some(r) = rest.strip_prefix('.') {
            let r = r.trim_start();
            let n = r.bytes().take_while(|&b| is_ident_byte(b)).count();
            if n > 0 && r[n..].trim_start().starts_with('(') {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_and_calls() {
        assert!(has_word("@Test void f()", "@Test"));
        assert!(!has_word("@TestFoo void f()", "@Test"));
        assert!(has_call("it('x', () => {})", "it"));
        assert!(has_call("describe.each([1])('x', f)", "describe"));
        assert!(!has_call("/x/.test(s)", "test"));
        assert!(!has_call("submit(1)", "it"));
    }

    #[test]
    fn utf8_safe_scanning() {
        assert!(!has_word("xéval_config(1)", "éval_config"));
        assert!(has_word("(éval_config)", "éval_config"));
        assert!(!has_call("xéval_config(1)", "éval_config"));
        assert!(has_call("éval_config(1)", "éval_config"));
        assert!(!has_word("abc", "") && !has_call("abc", ""));
    }

    #[test]
    fn rust_attrs() {
        for a in ["test", "tokio::test", "tokio::test(flavor = \"multi_thread\")", "rstest", "test_case(1)", "bench", "cfg(test)", "cfg(all(test, unix))"] {
            assert!(rust_attr_is_test(a), "{a}");
        }
        for a in ["derive(Debug)", "cfg(not(test))", "inline", "cfg(feature = \"testing\")", "allow(dead_code)"] {
            assert!(!rust_attr_is_test(a), "{a}");
        }
    }
}
