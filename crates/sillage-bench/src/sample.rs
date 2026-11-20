//! Deterministic query sampling for the §5 recall and §4.2 quality benchmarks.

use rand::Rng;
use rand::seq::index;
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::SeedableRng;
use sillage_core::MethodRecord;

/// Paper §5: sampled method bodies have at least 12 lines of code.
pub const MIN_BODY_LINES: usize = 12;
/// Paper §5: partial queries contain 5 lines.
pub const QUERY_LINES: usize = 5;

/// Body text of a method record (`text[body]`), falling back to the full text
/// when the body span is empty or out of bounds.
pub fn body_text(m: &MethodRecord) -> &str {
    let (s, e) = (m.body.start as usize, m.body.end as usize);
    if s < e && e <= m.text.len() && m.text.is_char_boundary(s) && m.text.is_char_boundary(e) {
        &m.text[s..e]
    } else {
        &m.text
    }
}

/// Non-empty lines of a method body with the enclosing `{` / `}` delimiters
/// (if any) removed.
pub fn body_lines(body: &str) -> Vec<&str> {
    let trimmed = body.trim();
    let inner = if trimmed.starts_with('{') && trimmed.ends_with('}') && trimmed.len() >= 2 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        // Keep the original indentation of brace-less (e.g. Python) bodies.
        body
    };
    inner
        .split('\n')
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .collect()
}

/// Joins `lines`, removing the longest common leading whitespace prefix.
pub fn dedent(lines: &[&str]) -> String {
    let mut common: Option<&str> = None;
    for l in lines.iter().filter(|l| !l.trim().is_empty()) {
        let ws = &l[..l.len() - l.trim_start().len()];
        common = Some(match common {
            None => ws,
            Some(c) => {
                let n = c
                    .char_indices()
                    .zip(ws.chars())
                    .take_while(|((_, a), b)| a == b)
                    .last()
                    .map(|((i, a), _)| i + a.len_utf8())
                    .unwrap_or(0);
                &c[..n]
            }
        });
    }
    let prefix = common.unwrap_or("");
    lines
        .iter()
        .map(|l| l.strip_prefix(prefix).unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Indices into `methods` of up to `n` methods with at least `min_lines`
/// non-empty body lines, sampled without replacement with `ChaCha8Rng(seed)`.
pub fn sample_methods(methods: &[MethodRecord], n: usize, min_lines: usize, seed: u64) -> Vec<usize> {
    let eligible: Vec<usize> = methods
        .iter()
        .enumerate()
        .filter(|(_, m)| body_lines(body_text(m)).len() >= min_lines)
        .map(|(i, _)| i)
        .collect();
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let take = n.min(eligible.len());
    index::sample(&mut rng, eligible.len(), take)
        .into_iter()
        .map(|i| eligible[i])
        .collect()
}

/// First `k` non-empty body lines (paper §5 contiguous queries).
pub fn extract_contiguous_query(body_text: &str, k: usize) -> String {
    let lines = body_lines(body_text);
    dedent(&lines[..k.min(lines.len())])
}

/// `k` randomly sampled non-empty body lines, kept in original order
/// (paper §5 non-contiguous queries).
pub fn extract_noncontiguous_query<R: Rng + ?Sized>(body_text: &str, k: usize, rng: &mut R) -> String {
    let lines = body_lines(body_text);
    let take = k.min(lines.len());
    let mut picks = index::sample(rng, lines.len(), take).into_vec();
    picks.sort_unstable();
    let chosen: Vec<&str> = picks.into_iter().map(|i| lines[i]).collect();
    dedent(&chosen)
}

/// Splits the non-empty body lines into `(first half, second half)`: the
/// first half is the query, the second half the held-out ground truth
/// (paper §4.2). Odd counts put the extra line in the held-out half.
pub fn split_half_query(body_text: &str) -> (String, String) {
    let lines = body_lines(body_text);
    let mid = lines.len() / 2;
    (dedent(&lines[..mid]), dedent(&lines[mid..]))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sillage_core::{Lang, MethodId, Span};

    use super::*;

    fn java_body(n: usize) -> String {
        let mut s = String::from("{\n");
        for i in 0..n {
            s.push_str(&format!("        int v{i} = f({i});\n"));
            if i % 3 == 0 {
                s.push('\n');
            }
        }
        s.push_str("    }");
        s
    }

    fn record(id: u32, body: &str) -> MethodRecord {
        let text = format!("void m{id}() {body}");
        let start = text.find('{').unwrap() as u32;
        MethodRecord {
            id: MethodId(id),
            lang: Lang::Java,
            repo: "r".into(),
            path: "A.java".into(),
            name: format!("m{id}").into(),
            line_start: 1,
            body: Span::new(start, text.len() as u32),
            text: Arc::from(text),
        }
    }

    #[test]
    fn body_lines_strip_braces_and_blanks() {
        let body = java_body(4);
        let lines = body_lines(&body);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], "        int v0 = f(0);");
        let py = "    x = 1\n\n    return x\n";
        assert_eq!(body_lines(py), vec!["    x = 1", "    return x"]);
    }

    #[test]
    fn contiguous_takes_first_five_dedented() {
        let q = extract_contiguous_query(&java_body(14), 5);
        assert_eq!(
            q,
            "int v0 = f(0);\nint v1 = f(1);\nint v2 = f(2);\nint v3 = f(3);\nint v4 = f(4);"
        );
        assert_eq!(extract_contiguous_query(&java_body(2), 5).lines().count(), 2);
    }

    #[test]
    fn contiguous_preserves_relative_indentation() {
        let body = "{\n    if (a) {\n        b();\n    }\n}";
        assert_eq!(extract_contiguous_query(body, 5), "if (a) {\n    b();\n}");
    }

    #[test]
    fn noncontiguous_is_seeded_ordered_and_sized() {
        let body = java_body(20);
        let mut r1 = ChaCha8Rng::seed_from_u64(7);
        let mut r2 = ChaCha8Rng::seed_from_u64(7);
        let a = extract_noncontiguous_query(&body, 5, &mut r1);
        let b = extract_noncontiguous_query(&body, 5, &mut r2);
        assert_eq!(a, b);
        let idx: Vec<usize> = a
            .lines()
            .map(|l| l.trim_start_matches("int v").split(' ').next().unwrap().parse().unwrap())
            .collect();
        assert_eq!(idx.len(), 5);
        assert!(idx.windows(2).all(|w| w[0] < w[1]), "{idx:?}");
        let mut r3 = ChaCha8Rng::seed_from_u64(8);
        let c = extract_noncontiguous_query(&body, 5, &mut r3);
        assert_ne!(a, c);
    }

    #[test]
    fn split_half_partitions_lines() {
        let (q, held) = split_half_query(&java_body(13));
        assert_eq!(q.lines().count(), 6);
        assert_eq!(held.lines().count(), 7);
        assert!(q.starts_with("int v0"));
        assert!(held.starts_with("int v6"));
        let (q1, h1) = split_half_query("{\n  only();\n}");
        assert_eq!(q1, "");
        assert_eq!(h1, "only();");
    }

    #[test]
    fn sample_methods_filters_and_is_deterministic() {
        let methods: Vec<MethodRecord> = (0..50)
            .map(|i| record(i, &java_body(if i % 2 == 0 { 14 } else { 6 })))
            .collect();
        assert_eq!(body_lines(body_text(&methods[0])).len(), 14);
        let a = sample_methods(&methods, 10, MIN_BODY_LINES, 7);
        let b = sample_methods(&methods, 10, MIN_BODY_LINES, 7);
        assert_eq!(a, b);
        assert_eq!(a.len(), 10);
        assert!(a.iter().all(|&i| i % 2 == 0));
        let mut uniq = a.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), 10);
        assert_eq!(sample_methods(&methods, 1000, MIN_BODY_LINES, 7).len(), 25);
        assert_ne!(a, sample_methods(&methods, 10, MIN_BODY_LINES, 8));
    }
}
