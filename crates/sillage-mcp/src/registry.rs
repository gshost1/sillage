//! Named corpora kept hot in RAM, shared by every transport.
//!
//! Each [`CorpusHandle`] owns its [`Engine`] behind an [`RwLock`]: queries take
//! a read lock (many in parallel). A directory-backed corpus also owns an
//! [`IncrementalIndex`] behind its own [`Mutex`], the per-corpus refresh lock:
//! [`CorpusHandle::refresh`] holds it while it re-walks and re-parses the
//! directory (queries keep running on the old index meanwhile) and takes the
//! engine's write lock only for the [`Engine::replace_index`] swap. Concurrent
//! refreshes of one corpus coalesce: while one runs, the others return
//! [`RefreshInProgress`] at once instead of queueing redundant directory walks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, TryLockError};
use std::time::{Duration, SystemTime};

use anyhow::{Context, bail};
use sillage_core::{Lang, Params};
use sillage_engine::Engine;
use sillage_index::{AromaFeaturizer, Corpus, IncrementalIndex, IndexParts, RefreshStats};

use crate::Shutdown;

/// Where a corpus comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A live directory, re-indexed incrementally on refresh.
    Directory,
    /// A prebuilt `.sil` index (static).
    Snapshot,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Directory => "directory",
            SourceKind::Snapshot => "snapshot",
        }
    }
}

/// A `name=path` corpus to load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusSpec {
    pub name: String,
    pub path: PathBuf,
}

impl CorpusSpec {
    pub fn new(name: impl Into<String>, path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let name = name.into();
        validate_name(&name)?;
        Ok(Self { name, path: path.into() })
    }

    /// Parses `name=path`.
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let Some((name, path)) = s.split_once('=') else {
            bail!("expected name=path, got {s:?}");
        };
        if path.trim().is_empty() {
            bail!("corpus {name:?} has an empty path");
        }
        Self::new(name.trim(), path.trim())
    }

    /// A spec for a root directory, named after its basename (or `default`).
    pub fn for_root(root: &Path) -> Self {
        let canon = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let name = canon
            .file_name()
            .and_then(|n| n.to_str())
            .map(sanitize_name)
            .filter(|n| validate_name(n).is_ok())
            .unwrap_or_else(|| "default".to_string());
        Self { name, path: root.to_path_buf() }
    }
}

/// Corpus names are what agents type: `[A-Za-z0-9_.-]{1,64}`.
pub fn validate_name(name: &str) -> anyhow::Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
    if !ok {
        bail!("invalid corpus name {name:?}: use 1-64 characters from A-Z, a-z, 0-9, '_', '-', '.'");
    }
    Ok(())
}

fn sanitize_name(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') { c } else { '-' })
        .take(64)
        .collect();
    out.trim_matches('-').to_string()
}

/// What is loaded and how fresh it is.
#[derive(Debug, Clone, PartialEq)]
pub struct CorpusMeta {
    pub methods: usize,
    pub prod_methods: usize,
    pub test_methods: usize,
    /// Indexed source files.
    pub files: usize,
    pub langs: Vec<Lang>,
    /// When the index was last (re)built or checked.
    pub last_refresh: Option<SystemTime>,
    pub last_stats: Option<RefreshStats>,
}

fn counts(corpus: &Corpus) -> (usize, usize) {
    let test = corpus.methods.iter().filter(|m| m.is_test).count();
    (corpus.methods.len() - test, test)
}

/// Returned (as the error) by [`CorpusHandle::refresh`] when another refresh
/// of the same corpus is already running; its result serves both callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshInProgress {
    pub corpus: String,
}

impl std::fmt::Display for RefreshInProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "refresh of corpus {} already in progress", self.corpus)
    }
}

impl std::error::Error for RefreshInProgress {}

/// One loaded corpus.
pub struct CorpusHandle {
    name: String,
    kind: SourceKind,
    engine: RwLock<Engine>,
    refresher: Option<Mutex<IncrementalIndex>>,
    meta: RwLock<CorpusMeta>,
}

/// Engine parameters for served corpora: the repo preset (single repositories
/// rarely have the paper's 5M-method density). The paper defaults stay
/// available per call (`preset: "paper"`).
fn serving_params() -> Params {
    Params::repo_preset()
}

impl CorpusHandle {
    /// Loads `spec`: a directory is indexed now (and refreshable later), a
    /// file is loaded as a `.sil` snapshot.
    pub fn open(spec: &CorpusSpec, langs: &[Lang]) -> anyhow::Result<Self> {
        let meta = std::fs::metadata(&spec.path)
            .with_context(|| format!("corpus {}: {} is not accessible", spec.name, spec.path.display()))?;
        if meta.is_dir() {
            Self::open_dir(&spec.name, &spec.path, langs)
        } else {
            Self::open_snapshot(&spec.name, &spec.path)
        }
    }

    pub fn open_dir(name: &str, root: &Path, langs: &[Lang]) -> anyhow::Result<Self> {
        validate_name(name)?;
        let langs: Vec<Lang> = if langs.is_empty() { Lang::ALL.to_vec() } else { langs.to_vec() };
        let mut inc = IncrementalIndex::new(root, &langs, Box::new(AromaFeaturizer))
            .with_context(|| format!("corpus {name}"))?;
        let refresh = inc.refresh().with_context(|| format!("corpus {name}: indexing failed"))?;
        let parts = match refresh.parts {
            Some(p) => p,
            None => inc.build(),
        };
        let (prod, test) = counts(&parts.corpus);
        let meta = CorpusMeta {
            methods: parts.corpus.methods.len(),
            prod_methods: prod,
            test_methods: test,
            files: refresh.stats.total_files,
            langs,
            last_refresh: Some(SystemTime::now()),
            last_stats: Some(refresh.stats),
        };
        let engine = Engine::from_parts(parts, Box::new(AromaFeaturizer), serving_params()).with_root(root);
        Ok(Self {
            name: name.to_string(),
            kind: SourceKind::Directory,
            engine: RwLock::new(engine),
            refresher: Some(Mutex::new(inc)),
            meta: RwLock::new(meta),
        })
    }

    pub fn open_snapshot(name: &str, path: &Path) -> anyhow::Result<Self> {
        validate_name(name)?;
        let (corpus, index, bm25) =
            sillage_index::load(path).with_context(|| format!("corpus {name}: cannot load snapshot"))?;
        let (prod, test) = counts(&corpus);
        let mut langs: Vec<Lang> = corpus.methods.iter().map(|m| m.lang).collect();
        langs.sort();
        langs.dedup();
        let meta = CorpusMeta {
            methods: corpus.methods.len(),
            prod_methods: prod,
            test_methods: test,
            files: corpus.file_imports.len(),
            langs,
            last_refresh: std::fs::metadata(path).and_then(|m| m.modified()).ok(),
            last_stats: None,
        };
        let engine = Engine::new(corpus, index, bm25, Box::new(AromaFeaturizer), serving_params());
        Ok(Self {
            name: name.to_string(),
            kind: SourceKind::Snapshot,
            engine: RwLock::new(engine),
            refresher: None,
            meta: RwLock::new(meta),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn kind(&self) -> SourceKind {
        self.kind
    }

    pub fn meta(&self) -> CorpusMeta {
        self.meta.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// A read guard on the engine (blocking; call from a blocking context).
    pub fn engine(&self) -> RwLockReadGuard<'_, Engine> {
        self.engine.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Incrementally re-indexes a directory corpus (blocking). The engine's
    /// write lock is held only for the index swap. When another refresh of
    /// this corpus is running, returns a [`RefreshInProgress`] error at once.
    pub fn refresh(&self) -> anyhow::Result<RefreshStats> {
        let Some(refresher) = &self.refresher else {
            bail!("corpus {} is a static .sil snapshot; rebuild it with `sillage index` and restart to update it", self.name);
        };
        let mut inc = match refresher.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => return Err(RefreshInProgress { corpus: self.name.clone() }.into()),
        };
        let refresh = inc.refresh().with_context(|| format!("corpus {}: refresh failed", self.name))?;
        let stats = refresh.stats;
        let counted = refresh.parts.map(|parts: IndexParts| {
            let c = counts(&parts.corpus);
            self.engine.write().unwrap_or_else(|e| e.into_inner()).replace_index(parts);
            c
        });
        // Stats are stored while the refresh lock is still held, so an older
        // refresh can never overwrite a newer one's.
        let mut meta = self.meta.write().unwrap_or_else(|e| e.into_inner());
        if let Some((prod, test)) = counted {
            meta.prod_methods = prod;
            meta.test_methods = test;
        }
        meta.methods = stats.total_methods;
        meta.files = stats.total_files;
        meta.last_refresh = Some(SystemTime::now());
        meta.last_stats = Some(stats);
        drop(meta);
        drop(inc);
        Ok(stats)
    }
}

/// How to assemble a [`CorpusRegistry`].
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// `name=path` corpora (directories or `.sil` files).
    pub corpora: Vec<CorpusSpec>,
    /// A root directory registered under its basename (first, so it is the
    /// default unless `default_corpus` says otherwise).
    pub root: Option<PathBuf>,
    pub default_corpus: Option<String>,
    /// Languages indexed for directory corpora (empty: all).
    pub langs: Vec<Lang>,
}

impl LoadOptions {
    /// The specs to load, in registration order. With neither `root` nor
    /// `corpora`, the current directory is the root.
    pub fn specs(&self) -> anyhow::Result<Vec<CorpusSpec>> {
        let mut specs = Vec::new();
        match &self.root {
            Some(root) => specs.push(CorpusSpec::for_root(root)),
            None if self.corpora.is_empty() => specs.push(CorpusSpec::for_root(&std::env::current_dir()?)),
            None => {}
        }
        specs.extend(self.corpora.iter().cloned());
        let mut seen = std::collections::BTreeSet::new();
        for s in &specs {
            if !seen.insert(s.name.as_str()) {
                bail!("corpus name {:?} is registered twice (rename one with --corpus name=path)", s.name);
            }
        }
        Ok(specs)
    }
}

/// All loaded corpora.
pub struct CorpusRegistry {
    corpora: BTreeMap<String, Arc<CorpusHandle>>,
    order: Vec<String>,
    default: String,
}

impl CorpusRegistry {
    /// Loads every corpus (in parallel) and picks the default one.
    pub fn load(opts: &LoadOptions) -> anyhow::Result<Self> {
        let specs = opts.specs()?;
        let loaded: Vec<anyhow::Result<CorpusHandle>> = std::thread::scope(|s| {
            let jobs: Vec<_> = specs.iter().map(|spec| s.spawn(|| CorpusHandle::open(spec, &opts.langs))).collect();
            jobs.into_iter().map(|j| j.join().unwrap_or_else(|_| Err(anyhow::anyhow!("corpus loader panicked")))).collect()
        });
        let handles = loaded.into_iter().collect::<anyhow::Result<Vec<_>>>()?;
        Self::from_handles(handles, opts.default_corpus.as_deref())
    }

    pub fn from_handles(handles: Vec<CorpusHandle>, default: Option<&str>) -> anyhow::Result<Self> {
        if handles.is_empty() {
            bail!("no corpus loaded: pass --root <dir> or --corpus name=path");
        }
        let order: Vec<String> = handles.iter().map(|h| h.name.clone()).collect();
        let default = match default {
            Some(d) if order.iter().any(|n| n == d) => d.to_string(),
            Some(d) => bail!("default corpus {d:?} is not loaded (loaded: {})", order.join(", ")),
            None => order[0].clone(),
        };
        let mut corpora = BTreeMap::new();
        for h in handles {
            if corpora.insert(h.name.clone(), Arc::new(h)).is_some() {
                bail!("duplicate corpus name");
            }
        }
        Ok(Self { corpora, order, default })
    }

    pub fn default_name(&self) -> &str {
        &self.default
    }

    /// Corpus names in registration order.
    pub fn names(&self) -> &[String] {
        &self.order
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Corpora in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<CorpusHandle>> {
        self.order.iter().filter_map(|n| self.corpora.get(n))
    }

    /// The named corpus, or the default one; the error lists what is loaded.
    pub fn get(&self, name: Option<&str>) -> Result<Arc<CorpusHandle>, String> {
        let name = name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&self.default);
        self.corpora
            .get(name)
            .cloned()
            .ok_or_else(|| format!("unknown corpus {name:?}; loaded corpora: {}", self.order.join(", ")))
    }

    /// Refreshes every directory corpus (blocking); snapshots are skipped.
    /// `stop` is checked before each corpus: once it returns `true` the
    /// remaining corpora are skipped (shutdown stays responsive).
    pub fn refresh_all(&self, stop: &dyn Fn() -> bool) -> Vec<(String, anyhow::Result<RefreshStats>)> {
        let mut out = Vec::new();
        for h in self.iter().filter(|h| h.kind == SourceKind::Directory) {
            if stop() {
                break;
            }
            out.push((h.name.clone(), h.refresh()));
        }
        out
    }

    /// Refreshes all directory corpora every `every` until `shutdown`.
    pub fn spawn_refresh_task(self: &Arc<Self>, every: Duration, shutdown: Shutdown) -> tokio::task::JoinHandle<()> {
        let reg = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await; // the first tick is immediate; corpora were just indexed
            loop {
                tokio::select! {
                    _ = shutdown.wait() => break,
                    _ = tick.tick() => {}
                }
                let r = Arc::clone(&reg);
                let sd = shutdown.clone();
                match tokio::task::spawn_blocking(move || r.refresh_all(&|| sd.is_triggered())).await {
                    Ok(results) => {
                        for (name, res) in results {
                            match res {
                                Ok(s) if s.changed() => eprintln!(
                                    "sillage: refreshed {name}: +{} ~{} -{} files, {} methods ({:.1} ms)",
                                    s.files_added, s.files_modified, s.files_removed, s.total_methods, s.elapsed_ms
                                ),
                                Ok(_) => {}
                                Err(e) if e.is::<RefreshInProgress>() => {}
                                Err(e) => eprintln!("sillage: background refresh of {name} failed: {e:#}"),
                            }
                        }
                    }
                    Err(e) => eprintln!("sillage: background refresh task failed: {e}"),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_corpus(name: &str) -> (tempfile::TempDir, CorpusHandle) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.go"), "package a\n\nfunc F(x int) int {\n\treturn x + 1\n}\n").unwrap();
        let h = CorpusHandle::open_dir(name, dir.path(), &[Lang::Go]).unwrap();
        (dir, h)
    }

    #[test]
    fn concurrent_refreshes_coalesce() {
        let (_dir, h) = dir_corpus("app");
        let guard = h.refresher.as_ref().unwrap().lock().unwrap(); // a refresh "in progress"
        let err = h.refresh().unwrap_err();
        assert_eq!(err.downcast_ref::<RefreshInProgress>(), Some(&RefreshInProgress { corpus: "app".into() }));
        assert!(err.to_string().contains("already in progress"));
        drop(guard);
        assert!(h.refresh().is_ok(), "runs once the other refresh is done");
    }

    #[test]
    fn refresh_all_stops_between_corpora() {
        let (_a, a) = dir_corpus("a");
        let (_b, b) = dir_corpus("b");
        let reg = CorpusRegistry::from_handles(vec![a, b], None).unwrap();
        assert_eq!(reg.refresh_all(&|| false).len(), 2);
        assert!(reg.refresh_all(&|| true).is_empty());
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let once = || calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 1;
        assert_eq!(reg.refresh_all(&once).len(), 1, "stop flag checked before the second corpus");
    }
}
