//! `sillage` command-line interface.

mod manifest;

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use sillage_bench::{QualityCfg, RecallCfg, RecallMode};
use sillage_core::{Lang, MethodId, MethodRecord, NodeId, NodeKind, Params, SimplifiedTree};
use sillage_engine::Engine;
use sillage_index::{AromaFeaturizer, Bm25, InvertedIndex};
use sillage_render::Style;

#[derive(Debug, Parser)]
#[command(
    name = "sillage",
    version,
    about = "Structural code search and cluster-and-intersect code recommendation (Aroma remake)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Shallow-clone the repositories listed in a corpus manifest.
    Fetch {
        #[arg(long, default_value = "corpus.toml")]
        manifest: PathBuf,
        #[arg(long, default_value = "corpus")]
        out: PathBuf,
    },
    /// Extract, featurize and index every method under a corpus directory.
    Index {
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Restrict to these languages (default: all).
        #[arg(long = "lang", value_parser = parse_lang, num_args = 1..)]
        langs: Vec<Lang>,
    },
    /// Recommend code for a snippet (file, or stdin when omitted / `-`).
    Query {
        #[arg(long)]
        index: PathBuf,
        #[command(flatten)]
        input: SnippetInput,
        #[arg(long, value_enum, default_value_t = Format::Ansi)]
        format: Format,
        #[command(flatten)]
        params: ParamArgs,
    },
    /// Structural search: Phase I retrieval plus Phase II prune-and-rerank.
    Search {
        #[arg(long)]
        index: PathBuf,
        #[command(flatten)]
        input: SnippetInput,
        /// Number of results to print.
        #[arg(long, default_value_t = 10)]
        n: usize,
        /// Also render each result with the query-matching lines unmarked.
        #[arg(long)]
        show: bool,
        #[arg(long, value_enum, default_value_t = Format::Ansi)]
        format: Format,
        #[command(flatten)]
        params: ParamArgs,
    },
    /// Print a snippet's simplified parse tree and per-leaf features.
    Explain {
        #[command(flatten)]
        input: SnippetInput,
    },
    /// Print corpus and index statistics.
    Stats {
        #[arg(long)]
        index: PathBuf,
    },
    /// Run the paper's benchmarks.
    Bench {
        #[command(subcommand)]
        bench: BenchCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum BenchCommand {
    /// §5 / Table 4: Recall@1 and Recall@100 on partial method bodies.
    Recall {
        #[arg(long)]
        index: PathBuf,
        #[arg(long, default_value_t = 1000)]
        n: usize,
        #[arg(long, value_enum, default_value_t = ModeArg::Contiguous)]
        mode: ModeArg,
        /// Bootstrap resamples for the 95% CI.
        #[arg(long, default_value_t = 1000)]
        bootstrap: usize,
        #[arg(long, default_value_t = 7)]
        seed: u64,
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        params: ParamArgs,
    },
    /// §4.2: recommendation quality on half-snippet queries.
    Quality {
        #[arg(long)]
        index: PathBuf,
        #[arg(long, default_value_t = 200)]
        n: usize,
        #[arg(long, default_value_t = 7)]
        seed: u64,
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        params: ParamArgs,
    },
}

#[derive(Debug, Args)]
pub struct SnippetInput {
    /// Snippet language (inferred from the file extension when omitted).
    #[arg(long, value_parser = parse_lang)]
    pub lang: Option<Lang>,
    /// Snippet file; reads stdin when omitted or `-`.
    pub file: Option<PathBuf>,
}

#[derive(Debug, Args, Default)]
pub struct ParamArgs {
    /// Recommendations to return (paper: 5).
    #[arg(long)]
    pub k: Option<usize>,
    /// Phase I candidates (paper: 1000).
    #[arg(long)]
    pub eta1: Option<usize>,
    /// Phase III candidates (paper: 100).
    #[arg(long)]
    pub eta2: Option<usize>,
    /// Pruned-similarity threshold (paper: 0.65).
    #[arg(long)]
    pub tau1: Option<f64>,
    /// Cluster extension-ratio threshold (paper: 1.5).
    #[arg(long)]
    pub tau2: Option<f64>,
    /// Cluster query-alignment threshold (paper: 0.9).
    #[arg(long)]
    pub tau3: Option<f64>,
}

impl ParamArgs {
    pub fn to_params(&self) -> Params {
        let mut p = Params::default();
        if let Some(v) = self.k {
            p.k = v;
        }
        if let Some(v) = self.eta1 {
            p.eta1 = v;
        }
        if let Some(v) = self.eta2 {
            p.eta2 = v;
        }
        if let Some(v) = self.tau1 {
            p.tau1 = v;
        }
        if let Some(v) = self.tau2 {
            p.tau2 = v;
        }
        if let Some(v) = self.tau3 {
            p.tau3 = v;
        }
        p
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Plain,
    Ansi,
    Html,
    Json,
}

impl From<Format> for Style {
    fn from(f: Format) -> Self {
        match f {
            Format::Plain => Style::Plain,
            Format::Ansi => Style::Ansi,
            Format::Html => Style::Html,
            Format::Json => Style::Json,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ModeArg {
    Contiguous,
    #[value(alias = "non-contiguous")]
    Noncontiguous,
}

impl From<ModeArg> for RecallMode {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Contiguous => RecallMode::Contiguous,
            ModeArg::Noncontiguous => RecallMode::NonContiguous,
        }
    }
}

fn parse_lang(s: &str) -> Result<Lang, String> {
    s.parse::<Lang>().map_err(|e| e.to_string())
}

impl SnippetInput {
    fn is_stdin(&self) -> bool {
        self.file.as_deref().is_none_or(|p| p == Path::new("-"))
    }

    /// Reads the snippet and resolves its language.
    fn read(&self) -> anyhow::Result<(Lang, String)> {
        let text = if self.is_stdin() {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).context("reading stdin")?;
            s
        } else {
            let p = self.file.as_deref().expect("file");
            std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?
        };
        Ok((self.resolve_lang()?, text))
    }

    fn resolve_lang(&self) -> anyhow::Result<Lang> {
        if let Some(l) = self.lang {
            return Ok(l);
        }
        if !self.is_stdin() {
            if let Some(ext) = self.file.as_deref().and_then(Path::extension) {
                if let Ok(l) = ext.to_string_lossy().parse::<Lang>() {
                    return Ok(l);
                }
            }
        }
        bail!("cannot infer the snippet language; pass --lang")
    }
}

fn load_engine(index: &Path, params: Params) -> anyhow::Result<Engine> {
    let (corpus, inv, bm25) =
        sillage_index::load(index).with_context(|| format!("loading index {}", index.display()))?;
    Ok(Engine::new(corpus, inv, bm25, Box::new(AromaFeaturizer), params))
}

fn method_lookup(engine: &Engine) -> HashMap<MethodId, &MethodRecord> {
    engine.corpus.methods.iter().map(|m| (m.id, m)).collect()
}

fn describe(m: Option<&&MethodRecord>, id: MethodId) -> String {
    match m {
        Some(m) => format!("{}:{}:{} {}", m.repo, m.path, m.line_start, m.name),
        None => id.to_string(),
    }
}

fn cmd_fetch(manifest_path: &Path, out: &Path) -> anyhow::Result<()> {
    let m = manifest::Manifest::load(manifest_path)?;
    let (mut ok, mut failed) = (0usize, 0usize);
    for spec in &m.repos {
        match manifest::fetch_repo(spec, out) {
            Ok((dir, sha)) => {
                ok += 1;
                println!(
                    "ok    {:<14} {:<10} {:<24} {} {}",
                    spec.name,
                    spec.lang,
                    spec.tag.as_deref().unwrap_or("-"),
                    &sha[..sha.len().min(12)],
                    dir.display()
                );
            }
            Err(e) => {
                failed += 1;
                eprintln!("fail  {:<14} {e:#}", spec.name);
            }
        }
    }
    println!("fetched {ok}/{} repos into {}", m.repos.len(), out.display());
    if failed > 0 {
        bail!("{failed} repo(s) failed to fetch");
    }
    Ok(())
}

fn cmd_index(corpus_dir: &Path, out: &Path, langs: &[Lang]) -> anyhow::Result<()> {
    let langs: Vec<Lang> = if langs.is_empty() { Lang::ALL.to_vec() } else { langs.to_vec() };
    let t0 = std::time::Instant::now();
    let corpus = sillage_index::build_corpus(corpus_dir, &langs);
    let inv = InvertedIndex::build(&corpus, &AromaFeaturizer);
    let bm25 = Bm25::build(&corpus);
    sillage_index::save(out, &corpus, &inv, &bm25)?;
    println!(
        "indexed {} methods, {} features, {} postings in {:.1}s -> {}",
        inv.num_methods(),
        inv.num_features(),
        inv.num_postings(),
        t0.elapsed().as_secs_f64(),
        out.display()
    );
    Ok(())
}

fn cmd_query(index: &Path, input: &SnippetInput, format: Format, params: &ParamArgs) -> anyhow::Result<()> {
    let (lang, snippet) = input.read()?;
    let engine = load_engine(index, params.to_params())?;
    let out = engine.recommend(lang, &snippet)?;
    let lookup = method_lookup(&engine);
    let style = Style::from(format);
    if format == Format::Json {
        let recs: Vec<serde_json::Value> = out
            .recommendations
            .iter()
            .map(|r| {
                let rendered: serde_json::Value = serde_json::from_str(&sillage_render::render(
                    &r.tree,
                    &r.query_match,
                    Some(&snippet),
                    style,
                ))
                .unwrap_or(serde_json::Value::Null);
                serde_json::json!({
                    "cluster": r.cluster.iter().map(|id| describe(lookup.get(id), *id)).collect::<Vec<_>>(),
                    "l": r.l,
                    "s": r.s,
                    "render": rendered,
                })
            })
            .collect();
        let doc = serde_json::json!({ "recommendations": recs, "timings": out.timings });
        println!("{}", serde_json::to_string_pretty(&doc)?);
        return Ok(());
    }
    if out.recommendations.is_empty() {
        println!("no recommendations ({} reranked candidates)", out.n2.len());
    }
    for (i, r) in out.recommendations.iter().enumerate() {
        let cluster: Vec<String> = r.cluster.iter().map(|id| describe(lookup.get(id), *id)).collect();
        println!(
            "== Recommendation {} (cluster of {}, l = {:.2}, s = {:.2})",
            i + 1,
            r.cluster.len(),
            r.l,
            r.s
        );
        for c in &cluster {
            println!("   from {c}");
        }
        print!("{}", sillage_render::render(&r.tree, &r.query_match, Some(&snippet), style));
        println!();
    }
    let t = &out.timings;
    eprintln!(
        "timings: parse {:.1} ms, phase I {:.1} ms, phase II {:.1} ms, phase III {:.1} ms, total {:.1} ms",
        t.parse_ms, t.phase1_ms, t.phase2_ms, t.phase3_ms, t.total_ms
    );
    Ok(())
}

fn cmd_search(
    index: &Path,
    input: &SnippetInput,
    n: usize,
    show: bool,
    format: Format,
    params: &ParamArgs,
) -> anyhow::Result<()> {
    let (lang, snippet) = input.read()?;
    let engine = load_engine(index, params.to_params())?;
    let results = engine.search(lang, &snippet, n)?;
    let lookup = method_lookup(&engine);
    for (rank, r) in results.iter().take(n).enumerate() {
        println!(
            "{:>3}. score {:.3}  sim {:>4}  overlap {:>4}  {}",
            rank + 1,
            r.score,
            r.sim,
            r.overlap,
            describe(lookup.get(&r.id), r.id)
        );
        if show {
            print!("{}", sillage_render::render(&r.tree, &r.pruned.keep, Some(&snippet), format.into()));
            println!();
        }
    }
    Ok(())
}

fn print_tree(tree: &SimplifiedTree, id: NodeId, depth: usize, out: &mut String) {
    let node = tree.node(id);
    let indent = "  ".repeat(depth);
    let slot = if node.index_in_parent > 0 { format!("[{}] ", node.index_in_parent) } else { String::new() };
    let line = match &node.kind {
        NodeKind::Internal { label } => format!(
            "{indent}{slot}{label:?}{}",
            if node.dropped_children { " (dropped children)" } else { "" }
        ),
        NodeKind::Keyword { text } => format!("{indent}kw {text:?}"),
        NodeKind::Token { text, var } => match var {
            Some(v) => format!("{indent}{slot}tok {text:?} #VAR{v}"),
            None => format!("{indent}{slot}tok {text:?}"),
        },
    };
    out.push_str(&line);
    out.push('\n');
    for &c in &node.children {
        print_tree(tree, c, depth + 1, out);
    }
}

fn cmd_explain(input: &SnippetInput) -> anyhow::Result<()> {
    let (lang, snippet) = input.read()?;
    let tree = sillage_parse::parse_snippet(lang, &snippet)?;
    let mut out = format!("# {lang} simplified parse tree ({} nodes, {} leaves)\n", tree.nodes.len(), tree.leaves.len());
    if !tree.is_empty() {
        print_tree(&tree, 0, 0, &mut out);
    }
    out.push_str("\n# per-leaf features\n");
    for (i, &leaf) in tree.leaves.iter().enumerate() {
        out.push_str(&format!("leaf {i} {:?}\n", tree.label(leaf)));
        for f in sillage_index::feature_debug(&tree, i) {
            out.push_str(&format!("    {f}\n"));
        }
    }
    print!("{out}");
    Ok(())
}

fn cmd_stats(index: &Path) -> anyhow::Result<()> {
    let (corpus, inv, _bm25) = sillage_index::load(index)?;
    let mut per_lang: Vec<(Lang, usize)> = Lang::ALL
        .iter()
        .map(|&l| (l, corpus.methods.iter().filter(|m| m.lang == l).count()))
        .filter(|&(_, c)| c > 0)
        .collect();
    per_lang.sort_by(|a, b| b.1.cmp(&a.1));
    println!("methods: {}", corpus.methods.len());
    for (l, c) in per_lang {
        println!("  {:<11} {c}", l.as_str());
    }
    let repos: std::collections::BTreeSet<&str> = corpus.methods.iter().map(|m| &*m.repo).collect();
    println!("repos: {}", repos.len());
    println!("indexed methods: {}", inv.num_methods());
    println!("features: {}", inv.num_features());
    println!("postings: {}", inv.num_postings());
    Ok(())
}

fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Fetch { manifest, out } => cmd_fetch(&manifest, &out),
        Command::Index { corpus, out, langs } => cmd_index(&corpus, &out, &langs),
        Command::Query { index, input, format, params } => cmd_query(&index, &input, format, &params),
        Command::Search { index, input, n, show, format, params } => {
            cmd_search(&index, &input, n, show, format, &params)
        }
        Command::Explain { input } => cmd_explain(&input),
        Command::Stats { index } => cmd_stats(&index),
        Command::Bench { bench } => match bench {
            BenchCommand::Recall { index, n, mode, bootstrap, seed, json, params } => {
                let engine = load_engine(&index, params.to_params())?;
                let report = sillage_bench::recall(&engine, RecallCfg { n, mode: mode.into(), bootstrap, seed })?;
                if json {
                    println!("{}", sillage_bench::recall_json(&report));
                } else {
                    print!("{}", sillage_bench::recall_markdown(&report));
                }
                Ok(())
            }
            BenchCommand::Quality { index, n, seed, json, params } => {
                let engine = load_engine(&index, params.to_params())?;
                let report = sillage_bench::quality(&engine, QualityCfg { n, seed })?;
                if json {
                    println!("{}", sillage_bench::quality_json(&report));
                } else {
                    print!("{}", sillage_bench::quality_markdown(&report));
                }
                Ok(())
            }
        },
    }
}

fn main() -> anyhow::Result<()> {
    run(Cli::parse())
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    fn parse(args: &[&str]) -> Command {
        let mut v = vec!["sillage"];
        v.extend_from_slice(args);
        Cli::try_parse_from(v).unwrap_or_else(|e| panic!("{e}")).command
    }

    #[test]
    fn clap_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_fetch() {
        match parse(&["fetch", "--manifest", "m.toml", "--out", "repos"]) {
            Command::Fetch { manifest, out } => {
                assert_eq!(manifest, PathBuf::from("m.toml"));
                assert_eq!(out, PathBuf::from("repos"));
            }
            c => panic!("{c:?}"),
        }
        assert!(matches!(parse(&["fetch"]), Command::Fetch { .. }));
    }

    #[test]
    fn parses_index_with_langs() {
        match parse(&["index", "--corpus", "c", "--out", "i.bin", "--lang", "java", "py", "--lang", "rs"]) {
            Command::Index { corpus, out, langs } => {
                assert_eq!(corpus, PathBuf::from("c"));
                assert_eq!(out, PathBuf::from("i.bin"));
                assert_eq!(langs, vec![Lang::Java, Lang::Python, Lang::Rust]);
            }
            c => panic!("{c:?}"),
        }
        match parse(&["index", "--corpus", "c", "--out", "i.bin"]) {
            Command::Index { langs, .. } => assert!(langs.is_empty()),
            c => panic!("{c:?}"),
        }
        assert!(Cli::try_parse_from(["sillage", "index", "--corpus", "c", "--out", "o", "--lang", "cobol"]).is_err());
    }

    #[test]
    fn parses_query_with_params() {
        match parse(&[
            "query", "--index", "i.bin", "--lang", "java", "q.java", "--format", "json", "--k", "3", "--eta1",
            "500", "--eta2", "50", "--tau1", "0.5", "--tau2", "2.0", "--tau3", "0.8",
        ]) {
            Command::Query { index, input, format, params } => {
                assert_eq!(index, PathBuf::from("i.bin"));
                assert_eq!(input.lang, Some(Lang::Java));
                assert_eq!(input.file, Some(PathBuf::from("q.java")));
                assert_eq!(format, Format::Json);
                let p = params.to_params();
                assert_eq!((p.k, p.eta1, p.eta2), (3, 500, 50));
                assert_eq!((p.tau1, p.tau2, p.tau3), (0.5, 2.0, 0.8));
                assert_eq!(p.jaccard, Params::default().jaccard);
            }
            c => panic!("{c:?}"),
        }
        match parse(&["query", "--index", "i.bin", "--lang", "python"]) {
            Command::Query { input, format, params, .. } => {
                assert!(input.file.is_none() && input.is_stdin());
                assert_eq!(format, Format::Ansi);
                assert_eq!(params.to_params(), Params::default());
            }
            c => panic!("{c:?}"),
        }
        for f in ["plain", "ansi", "html", "json"] {
            assert!(matches!(parse(&["query", "--index", "i", "--format", f, "-"]), Command::Query { .. }));
        }
        assert!(Cli::try_parse_from(["sillage", "query", "--index", "i", "--format", "pdf"]).is_err());
    }

    #[test]
    fn parses_search() {
        match parse(&["search", "--index", "i.bin", "--lang", "go", "q.go", "--n", "25", "--show"]) {
            Command::Search { n, show, input, .. } => {
                assert_eq!(n, 25);
                assert!(show);
                assert_eq!(input.lang, Some(Lang::Go));
            }
            c => panic!("{c:?}"),
        }
        match parse(&["search", "--index", "i.bin", "--lang", "ts"]) {
            Command::Search { n, show, .. } => assert_eq!((n, show), (10, false)),
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn parses_explain_and_infers_lang() {
        match parse(&["explain", "--lang", "javascript", "a.js"]) {
            Command::Explain { input } => assert_eq!(input.resolve_lang().unwrap(), Lang::JavaScript),
            c => panic!("{c:?}"),
        }
        match parse(&["explain", "bench/queries/bitmap_decode.java"]) {
            Command::Explain { input } => assert_eq!(input.resolve_lang().unwrap(), Lang::Java),
            c => panic!("{c:?}"),
        }
        match parse(&["explain", "-"]) {
            Command::Explain { input } => assert!(input.resolve_lang().is_err()),
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn parses_stats() {
        match parse(&["stats", "--index", "i.bin"]) {
            Command::Stats { index } => assert_eq!(index, PathBuf::from("i.bin")),
            c => panic!("{c:?}"),
        }
        assert!(Cli::try_parse_from(["sillage", "stats"]).is_err());
    }

    #[test]
    fn parses_bench_recall() {
        match parse(&[
            "bench", "recall", "--index", "i.bin", "--n", "50", "--mode", "noncontiguous", "--bootstrap", "200",
            "--seed", "3", "--json",
        ]) {
            Command::Bench { bench: BenchCommand::Recall { n, mode, bootstrap, seed, json, .. } } => {
                assert_eq!((n, bootstrap, seed, json), (50, 200, 3, true));
                assert_eq!(RecallMode::from(mode), RecallMode::NonContiguous);
            }
            c => panic!("{c:?}"),
        }
        match parse(&["bench", "recall", "--index", "i.bin", "--mode", "non-contiguous"]) {
            Command::Bench { bench: BenchCommand::Recall { mode, .. } } => {
                assert_eq!(mode, ModeArg::Noncontiguous)
            }
            c => panic!("{c:?}"),
        }
        match parse(&["bench", "recall", "--index", "i.bin"]) {
            Command::Bench { bench: BenchCommand::Recall { n, mode, bootstrap, seed, json, .. } } => {
                assert_eq!((n, bootstrap, seed, json), (1000, 1000, 7, false));
                assert_eq!(mode, ModeArg::Contiguous);
            }
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn parses_bench_quality() {
        match parse(&["bench", "quality", "--index", "i.bin", "--n", "20", "--seed", "11", "--json"]) {
            Command::Bench { bench: BenchCommand::Quality { n, seed, json, .. } } => {
                assert_eq!((n, seed, json), (20, 11, true))
            }
            c => panic!("{c:?}"),
        }
        match parse(&["bench", "quality", "--index", "i.bin"]) {
            Command::Bench { bench: BenchCommand::Quality { n, seed, json, .. } } => {
                assert_eq!((n, seed, json), (200, 7, false))
            }
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn query_fixtures_exist_for_every_language() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/queries");
        let mut langs = std::collections::BTreeSet::new();
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            let text = std::fs::read_to_string(&p).unwrap();
            assert!(!text.trim().is_empty(), "{}", p.display());
            if let Some(l) = p.extension().and_then(|x| x.to_str()).and_then(|x| x.parse::<Lang>().ok()) {
                langs.insert(l);
            }
        }
        for l in [Lang::Java, Lang::Python, Lang::JavaScript, Lang::TypeScript, Lang::Go, Lang::Rust] {
            assert!(langs.contains(&l), "missing fixture for {l}");
        }
        for f in ["bitmap_decode.java", "viewgroup_loop.java", "package_version.java"] {
            assert!(dir.join(f).exists(), "{f}");
        }
    }
}
