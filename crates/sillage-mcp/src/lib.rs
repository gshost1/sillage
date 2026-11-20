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
pub use protocol::{Dispatcher, ToolLimits};
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
    /// Allow serving without a token on a non-loopback address, or with
    /// `--cors-origin *` and no token.
    pub allow_unauthenticated: bool,
}

/// The bearer token: `flag` (`--token`), else `env` (`$SILLAGE_MCP_TOKEN`).
/// A token given but blank (empty or whitespace) is an error, never a silent
/// "no auth".
pub fn resolve_token(flag: Option<String>, env: Option<String>) -> anyhow::Result<Option<String>> {
    let (source, value) = match (flag, env) {
        (Some(f), _) => ("--token", f),
        (None, Some(e)) => (TOKEN_ENV, e),
        (None, None) => return Ok(None),
    };
    let t = value.trim();
    if t.is_empty() {
        anyhow::bail!("{source} is set but blank; set a real token (e.g. `openssl rand -hex 32`) or unset it");
    }
    Ok(Some(t.to_string()))
}

/// Refuses unsafe exposure unless `allow_unauthenticated`: no token on a
/// non-loopback address, or `--cors-origin *` without a token.
pub fn check_exposure(loopback: bool, http: &HttpConfig, allow_unauthenticated: bool) -> anyhow::Result<()> {
    if http.token.is_some() || allow_unauthenticated {
        return Ok(());
    }
    if !loopback {
        anyhow::bail!(
            "refusing to serve on a non-loopback address without a token: set {TOKEN_ENV} (or --token), \
             or pass --allow-unauthenticated if the network is trusted"
        );
    }
    if http.cors_origins.iter().any(|o| o.trim() == "*") {
        anyhow::bail!(
            "refusing --cors-origin '*' without a token (any web page could call the tools): set {TOKEN_ENV}, \
             list specific origins, or pass --allow-unauthenticated"
        );
    }
    Ok(())
}

/// Whether every address `host:port` resolves to is loopback.
fn resolves_to_loopback(host: &str, port: u16) -> anyhow::Result<bool> {
    use std::net::ToSocketAddrs;
    let addrs: Vec<_> =
        (host, port).to_socket_addrs().map_err(|e| anyhow::anyhow!("cannot resolve {host}:{port}: {e}"))?.collect();
    Ok(!addrs.is_empty() && addrs.iter().all(|a| a.ip().is_loopback()))
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

/// Runs the Streamable HTTP server until SIGINT / SIGTERM. Shutdown is
/// bounded: open connections get [`HttpConfig::shutdown_timeout`], the
/// background refresher the same, and blocking work still running after
/// that is abandoned.
pub fn run_http(load: LoadOptions, opts: ServeOptions) -> anyhow::Result<()> {
    // Fail fast, before the (possibly slow) indexing.
    check_exposure(resolves_to_loopback(&opts.host, opts.port)?, &opts.http, opts.allow_unauthenticated)?;
    let registry = load_registry(&load)?;
    let rt = runtime()?;
    let grace = opts.http.shutdown_timeout;
    let result = rt.block_on(async move {
        let shutdown = Shutdown::new();
        shutdown.on_signals();
        let listener = tokio::net::TcpListener::bind((opts.host.as_str(), opts.port))
            .await
            .map_err(|e| anyhow::anyhow!("cannot bind {}:{}: {e}", opts.host, opts.port))?;
        let addr = listener.local_addr()?;
        check_exposure(addr.ip().is_loopback(), &opts.http, opts.allow_unauthenticated)?;
        let refresher = opts.refresh_interval.map(|every| registry.spawn_refresh_task(every, shutdown.clone()));
        let auth = if opts.http.token.is_some() { "bearer token" } else { "none" };
        eprintln!("sillage: MCP Streamable HTTP on http://{addr}/mcp (auth: {auth}; health: /healthz)");
        if opts.http.token.is_none() && !addr.ip().is_loopback() {
            eprintln!("sillage: warning: --allow-unauthenticated on a non-loopback address; anyone who can reach it can query the corpora");
        }
        let dispatcher = Arc::new(Dispatcher::new(registry));
        let served = http::serve(listener, dispatcher, opts.http, shutdown.clone()).await;
        shutdown.trigger();
        if let Some(r) = refresher
            && tokio::time::timeout(grace, r).await.is_err()
        {
            eprintln!("sillage: background refresh still running at shutdown; abandoning it");
        }
        served.map_err(anyhow::Error::from)
    });
    // Tool calls or a refresh still on the blocking pool are not waited for.
    rt.shutdown_timeout(Duration::from_millis(500));
    result
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_tokens_are_rejected() {
        assert_eq!(resolve_token(Some("  t0k ".into()), None).unwrap().as_deref(), Some("t0k"));
        assert_eq!(resolve_token(None, Some("env-tok".into())).unwrap().as_deref(), Some("env-tok"));
        assert_eq!(resolve_token(Some("flag".into()), Some("".into())).unwrap().as_deref(), Some("flag"), "flag wins");
        assert_eq!(resolve_token(None, None).unwrap(), None);
        let e = resolve_token(Some("   ".into()), None).unwrap_err().to_string();
        assert!(e.contains("--token") && e.contains("blank"), "{e}");
        let e = resolve_token(None, Some("\t".into())).unwrap_err().to_string();
        assert!(e.contains(TOKEN_ENV) && e.contains("blank"), "{e}");
    }

    #[test]
    fn exposure_needs_a_token_or_explicit_opt_out() {
        let open = HttpConfig::default();
        let tok = HttpConfig { token: Some("t".into()), ..HttpConfig::default() };
        assert!(check_exposure(true, &open, false).is_ok(), "loopback without a token is fine");
        let e = check_exposure(false, &open, false).unwrap_err().to_string();
        assert!(e.contains("non-loopback") && e.contains("--allow-unauthenticated"), "{e}");
        assert!(check_exposure(false, &open, true).is_ok());
        assert!(check_exposure(false, &tok, false).is_ok());
        let star = HttpConfig { cors_origins: vec!["*".into()], ..HttpConfig::default() };
        assert!(check_exposure(true, &star, false).unwrap_err().to_string().contains("--cors-origin"));
        assert!(check_exposure(true, &star, true).is_ok());
        assert!(check_exposure(true, &HttpConfig { token: Some("t".into()), ..star }, false).is_ok());
        assert!(resolves_to_loopback("127.0.0.1", 0).unwrap());
        assert!(resolves_to_loopback("::1", 0).unwrap());
        assert!(!resolves_to_loopback("0.0.0.0", 0).unwrap());
    }
}
