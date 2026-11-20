//! Variable-aligned recommendations: renames the local variables of a
//! recommendation to the names the agent used in its query snippet.
//!
//! Local variables carry a [`VarId`] per binding in both trees, and the
//! featurizer's per-leaf features are alpha-renamed (`#VAR`), so two variable
//! occurrences in the same structural context share features regardless of
//! their names. A recommendation variable `a` is paired with the query
//! variable `b` maximising `Σ max(0, |F(leaf_a) ∩ F(leaf_b)| − 1)` over their
//! occurrences (the `− 1` discounts the `#VAR` token feature every variable
//! shares), one-to-one and greedily. Only variables that occur in the
//! query-matching part of the recommendation are renamed (new variables keep
//! their names), and a rename is dropped when it would collide with another
//! variable or a non-member identifier of the recommendation.
//!
//! Renaming is applied per token span ([`sillage_render::layout_with`]), so
//! string literals, comments, keywords and member names such as `.x` are never
//! touched; a shorthand field (`Foo { x }`, JS `{ x }`) is expanded to `x: y`.

use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use sillage_core::{Featurizer, NodeKind, SimplifiedTree, VarId};
use sillage_render::{Layout, layout_with, unified_diff};

/// Minimum pairing score for a rename (shared non-token features).
const MIN_SCORE: u64 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rename {
    /// The recommendation's variable name.
    pub from: String,
    /// The query's variable name.
    pub to: String,
}

/// Renames for one recommendation tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VarAlignment {
    pub renames: Vec<Rename>,
    /// Replacement text per leaf of the recommendation (aligned with
    /// `rec.leaves`), for [`layout_with`].
    pub overrides: Vec<Option<String>>,
}

/// A recommendation rendered with the query's variable names.
#[derive(Debug, Clone, Serialize)]
pub struct AlignedRecommendation {
    pub layout: Layout,
    /// The whole (aligned) recommendation.
    pub code: String,
    /// Only the recommended (extra) lines.
    pub added_code: String,
    /// Unified diff of the query snippet against `code` (placeholders omitted).
    pub diff: String,
    pub renames: Vec<Rename>,
}

struct Vars {
    name: Vec<(VarId, String)>,
    leaves: FxHashMap<VarId, Vec<usize>>,
}

fn vars(tree: &SimplifiedTree) -> Vars {
    let mut name = Vec::new();
    let mut leaves: FxHashMap<VarId, Vec<usize>> = FxHashMap::default();
    for (i, &l) in tree.leaves.iter().enumerate() {
        if let NodeKind::Token { text, var: Some(v) } = &tree.node(l).kind {
            let e = leaves.entry(*v).or_default();
            if e.is_empty() {
                name.push((*v, text.to_string()));
            }
            e.push(i);
        }
    }
    Vars { name, leaves }
}

/// Pairs the variables of `rec` with those of `query` (see the [module docs](self)).
/// `rec_query_match` marks the leaves of `rec` matched to the query.
pub fn align_variables(
    query: &SimplifiedTree,
    rec: &SimplifiedTree,
    rec_query_match: &[bool],
    featurizer: &dyn Featurizer,
) -> VarAlignment {
    let mut out = VarAlignment { renames: Vec::new(), overrides: vec![None; rec.leaves.len()] };
    if query.leaves.is_empty() || rec.leaves.is_empty() {
        return out;
    }
    let (qv, rv) = (vars(query), vars(rec));
    if qv.name.is_empty() || rv.name.is_empty() {
        return out;
    }
    let qf = featurizer.featurize(query).per_leaf;
    let rf = featurizer.featurize(rec).per_leaf;

    // Pair scores, for recommendation variables in the query-matching part.
    let mut pairs: Vec<(u64, usize, usize)> = Vec::new();
    for (ri, (rvar, _)) in rv.name.iter().enumerate() {
        let rleaves = &rv.leaves[rvar];
        if !rleaves.iter().any(|&i| rec_query_match.get(i).copied().unwrap_or(false)) {
            continue;
        }
        for (qi, (qvar, _)) in qv.name.iter().enumerate() {
            let score: u64 = rleaves
                .iter()
                .flat_map(|&i| qv.leaves[qvar].iter().map(move |&j| (i, j)))
                .map(|(i, j)| match (rf.get(i), qf.get(j)) {
                    (Some(a), Some(b)) => a.intersect_count(b).saturating_sub(1),
                    _ => 0,
                })
                .sum();
            if score >= MIN_SCORE {
                pairs.push((score, ri, qi));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let (mut used_r, mut used_q) = (FxHashSet::default(), FxHashSet::default());
    let mut chosen: Vec<(u64, usize, usize)> = Vec::new();
    for p in pairs {
        if used_r.insert(p.1) {
            if used_q.insert(p.2) {
                chosen.push(p);
            } else {
                used_r.remove(&p.1);
            }
        }
    }
    chosen.retain(|&(_, ri, qi)| rv.name[ri].1 != qv.name[qi].1);

    // Non-variable identifiers that a rename must not shadow (members after
    // `.` / `->` / `::` are fine).
    let src: &str = &rec.source;
    let globals: FxHashSet<&str> = rec
        .leaves
        .iter()
        .filter_map(|&l| {
            let n = rec.node(l);
            match &n.kind {
                NodeKind::Token { text, var: None } if !is_member(src, n.span.start as usize) => Some(&**text),
                _ => None,
            }
        })
        .collect();
    // Drop the weakest colliding rename until the final names are injective.
    loop {
        let target: FxHashMap<usize, &str> = chosen.iter().map(|&(_, ri, qi)| (ri, qv.name[qi].1.as_str())).collect();
        let final_name = |ri: usize| target.get(&ri).copied().unwrap_or(rv.name[ri].1.as_str());
        let mut owner: FxHashMap<&str, usize> = FxHashMap::default();
        let mut bad: Option<usize> = None;
        for ri in 0..rv.name.len() {
            let n = final_name(ri);
            if let Some(&other) = owner.get(n) {
                bad = [ri, other].into_iter().filter(|r| target.contains_key(r)).min_by_key(|r| {
                    chosen.iter().find(|c| c.1 == *r).map_or(0, |c| c.0)
                });
                if bad.is_some() {
                    break;
                }
            }
            owner.insert(n, ri);
        }
        if bad.is_none() {
            bad = chosen.iter().filter(|&&(_, _, qi)| globals.contains(qv.name[qi].1.as_str())).map(|c| c.1).next();
        }
        match bad {
            Some(ri) => chosen.retain(|c| c.1 != ri),
            None => break,
        }
    }

    let shorthand: FxHashSet<(u32, u32)> = sillage_parse::shorthand_spans(rec.lang, src).into_iter().collect();
    chosen.sort_by_key(|c| rv.leaves[&rv.name[c.1].0][0]);
    for &(_, ri, qi) in &chosen {
        let (rvar, from) = &rv.name[ri];
        let to = &qv.name[qi].1;
        for &i in &rv.leaves[rvar] {
            let span = rec.node(rec.leaves[i]).span;
            out.overrides[i] = Some(if shorthand.contains(&(span.start, span.end)) { format!("{from}: {to}") } else { to.clone() });
        }
        out.renames.push(Rename { from: from.clone(), to: to.clone() });
    }
    out
}

/// The token at `start` is a member name (after `.`, `?.`, `->` or `::`).
fn is_member(src: &str, start: usize) -> bool {
    let before = src.get(..start).unwrap_or("").trim_end();
    before.ends_with('.') || before.ends_with("->") || before.ends_with("::")
}

/// Aligns `rec` to `query` and renders it: code, added lines and a unified
/// diff against `query_text`.
pub fn render_aligned(
    query_text: &str,
    query: &SimplifiedTree,
    rec: &SimplifiedTree,
    rec_query_match: &[bool],
    featurizer: &dyn Featurizer,
) -> AlignedRecommendation {
    let al = align_variables(query, rec, rec_query_match, featurizer);
    let layout = layout_with(rec, rec_query_match, None, &al.overrides);
    let code = layout.code();
    let added_code = layout.extra_code();
    let diff = unified_diff(query_text, &layout.code_without_placeholders(), "draft", "recommendation");
    AlignedRecommendation { layout, code, added_code, diff, renames: al.renames }
}
