//! Sillage MCP server: the agent tools of `sillage-engine` over the Model
//! Context Protocol, on two transports sharing one dispatcher and one
//! in-memory [`CorpusRegistry`]:
//!
//! - [`http`]: Streamable HTTP (`POST`/`GET`/`DELETE /mcp`, `GET /healthz`)
//!   for a web-hosted server (`sillage serve`);
//! - [`stdio`]: newline-delimited JSON-RPC on stdin/stdout for a local
//!   server launched by the agent (`sillage mcp`).

pub mod http;
pub mod protocol;
pub mod registry;
pub mod stdio;
pub mod tools;

use std::sync::Arc;
use std::time::Duration;

pub use http::HttpConfig;
pub use protocol::Dispatcher;
pub use registry::{CorpusHandle, CorpusMeta, CorpusRegistry, CorpusSpec, LoadOptions, SourceKind};

/// Environment variable holding the bearer token (`--token` overrides it).
pub const TOKEN_ENV: &str = "SILLAGE_MCP_TOKEN";

/// A cloneable one-shot shutdown signal.
#[derive(Clone)]
pub struct Shutdown {
    tx: Arc<tokio::sync::watch::Sender<bool>>,
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

impl Shutdown {
    pub fn new() -> Self {
        Self { tx: Arc::new(tokio::sync::watch::channel(false).0) }
    }

    pub fn trigger(&self) {
        self.tx.send_replace(true);
    }

    pub fn is_triggered(&self) -> bool {
        *self.tx.borrow()
    }

    /// Resolves once [`trigger`](Self::trigger) has been called.
    pub async fn wait(&self) {
        let mut rx = self.tx.subscribe();
        let _ = rx.wait_for(|v| *v).await;
    }

    /// Triggers on SIGINT (Ctrl-C) or SIGTERM.
    pub fn on_signals(&self) -> tokio::task::JoinHandle<()> {
        let sd = self.clone();
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
                let sigterm = async {
                    match term.as_mut() {
                        Some(t) => {
                            t.recv().await;
                        }
                        None => std::future::pending::<()>().await,
                    }
                };
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = sigterm => {}
                    _ = sd.wait() => return,
                }
            }
            #[cfg(not(unix))]
            {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = sd.wait() => return,
                }
            }
            eprintln!("sillage: shutting down");
            sd.trigger();
        })
    }
}

/// `sillage serve` options.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    pub host: String,
    pub port: u16,
    pub http: HttpConfig,
    /// Background refresh period (`None`: manual `refresh_corpus` only).
    pub refresh_interval: Option<Duration>,
}

fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("sillage-mcp").build()?)
}

fn load_registry(load: &LoadOptions) -> anyhow::Result<Arc<CorpusRegistry>> {
    let reg = CorpusRegistry::load(load)?;
    for h in reg.iter() {
        let m = h.meta();
        eprintln!(
            "sillage: corpus {}{} ({}): {} methods ({} prod / {} test) in {} files",
            h.name(),
            if h.name() == reg.default_name() { " [default]" } else { "" },
            h.kind().as_str(),
            m.methods,
            m.prod_methods,
            m.test_methods,
            m.files
        );
    }
    Ok(Arc::new(reg))
}

/// Runs the Streamable HTTP server until SIGINT / SIGTERM.
pub fn run_http(load: LoadOptions, opts: ServeOptions) -> anyhow::Result<()> {
    let registry = load_registry(&load)?;
    let rt = runtime()?;
    rt.block_on(async move {
        let shutdown = Shutdown::new();
        shutdown.on_signals();
        let refresher = opts.refresh_interval.map(|every| registry.spawn_refresh_task(every, shutdown.clone()));
        let listener = tokio::net::TcpListener::bind((opts.host.as_str(), opts.port))
            .await
            .map_err(|e| anyhow::anyhow!("cannot bind {}:{}: {e}", opts.host, opts.port))?;
        let addr = listener.local_addr()?;
        let auth = if opts.http.token.is_some() { "bearer token" } else { "none" };
        eprintln!("sillage: MCP Streamable HTTP on http://{addr}/mcp (auth: {auth}; health: /healthz)");
        if opts.http.token.is_none() && !addr.ip().is_loopback() {
            eprintln!(
                "sillage: warning: listening on a non-loopback address without a token; set --token or {TOKEN_ENV}"
            );
        }
        let dispatcher = Arc::new(Dispatcher::new(registry));
        let served = http::serve(listener, dispatcher, opts.http, shutdown.clone()).await;
        shutdown.trigger();
        if let Some(r) = refresher {
            let _ = r.await;
        }
        served.map_err(anyhow::Error::from)
    })
}

/// Runs the stdio server until stdin closes or SIGINT / SIGTERM.
pub fn run_stdio(load: LoadOptions, refresh_interval: Option<Duration>) -> anyhow::Result<()> {
    let registry = load_registry(&load)?;
    let rt = runtime()?;
    let result = rt.block_on(async move {
        let shutdown = Shutdown::new();
        shutdown.on_signals();
        let refresher = refresh_interval.map(|every| registry.spawn_refresh_task(every, shutdown.clone()));
        let dispatcher = Arc::new(Dispatcher::new(registry));
        let res = stdio::serve_stdio(dispatcher, shutdown.clone()).await;
        shutdown.trigger();
        if let Some(r) = refresher {
            let _ = r.await;
        }
        res
    });
    // The stdin reader thread may still be blocked in read(); don't wait for it.
    rt.shutdown_timeout(Duration::from_millis(200));
    result
}
