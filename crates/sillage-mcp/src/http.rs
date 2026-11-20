//! MCP Streamable HTTP transport.
//!
//! - `POST /mcp`: one JSON-RPC message (or batch) per request. Requests get
//!   `200 application/json`; notifications and client responses `202`.
//! - `GET /mcp`: an SSE stream (`text/event-stream`) for server-initiated
//!   messages, bound to an `Mcp-Session-Id` (required). This server sends
//!   none, so the stream carries only keep-alive comments (each one also
//!   keeps its session alive); it ends on shutdown, when its session is
//!   deleted or expires, or after [`HttpConfig::max_stream_lifetime`].
//! - `DELETE /mcp`: terminates the session named by `Mcp-Session-Id` (`204`).
//! - `OPTIONS /mcp`: CORS preflight.
//! - `GET /healthz`: unauthenticated `{"status":"ok"}`.
//!
//! **Sessions.** A successful `initialize` issues a random `Mcp-Session-Id`.
//! Requests carrying an `Mcp-Session-Id` must name a live session (else
//! `404`, after which clients re-initialize). `POST`s without one are served
//! statelessly: every tool is a pure function of its arguments and the corpus,
//! so no per-session state is needed. Sessions expire after
//! [`HttpConfig::session_ttl`] of inactivity (swept lazily) and at most
//! [`HttpConfig::max_sessions`] are kept (least recently used evicted).
//! Sessions that negotiated protocol 2025-06-18 (or requests declaring it in
//! `MCP-Protocol-Version`) cannot send JSON-RPC batches, which that version
//! removed.
//!
//! **Security.** With a token, every `/mcp` request needs
//! `Authorization: Bearer <token>` (SHA-256 digests compared in constant
//! time, so neither content nor length leaks); failures get `401` with
//! `WWW-Authenticate: Bearer`. Browser requests (with an `Origin` header) are
//! rejected with `403` unless the origin is allowed by
//! [`HttpConfig::cors_origins`]; on a loopback bind, `Host` must name the
//! loopback host (or an [`HttpConfig::allowed_hosts`] entry), else `403`
//! (DNS-rebinding protection). Bodies over [`HttpConfig::max_body_bytes`]
//! get `413` without being buffered.
//!
//! **Resource limits.** At most [`HttpConfig::max_connections`] connections
//! are served at once (excess ones are closed on accept); request headers
//! must arrive within [`HttpConfig::header_read_timeout`] (which also closes
//! idle keep-alive connections); a request body must arrive within
//! [`HttpConfig::request_timeout`] (`408`), and every non-SSE request is cut
//! off after a backstop deadline (`503`). Graceful shutdown waits at most
//! [`HttpConfig::shutdown_timeout`] for open connections, then drops them.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use http_body_util::{BodyExt, LengthLimitError, Limited};
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::{Semaphore, watch};

use crate::Shutdown;
use crate::protocol::{
    self, Dispatcher, INVALID_REQUEST, MessageKind, NO_BATCH_PROTOCOL_VERSION, PARSE_ERROR, SUPPORTED_PROTOCOL_VERSIONS,
};

pub const SESSION_HEADER: &str = "mcp-session-id";
pub const PROTOCOL_HEADER: &str = "mcp-protocol-version";
pub const DEFAULT_MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
pub const DEFAULT_MAX_CONNECTIONS: usize = 256;
/// Hosts always accepted on a loopback bind.
const LOOPBACK_HOSTS: &[&str] = &["localhost", "127.0.0.1", "[::1]"];

/// HTTP transport settings.
#[derive(Clone)]
pub struct HttpConfig {
    /// Required bearer token (`None`: no auth).
    pub token: Option<String>,
    /// Allowed browser origins (`*` allows any). Requests without `Origin`
    /// are always allowed.
    pub cors_origins: Vec<String>,
    pub max_body_bytes: usize,
    pub max_sessions: usize,
    pub session_ttl: Duration,
    /// Concurrent `GET /mcp` streams.
    pub max_streams: usize,
    pub keep_alive: Duration,
    /// Extra `Host` values accepted on a loopback bind (`*`: any).
    pub allowed_hosts: Vec<String>,
    /// Concurrent TCP connections; excess ones are closed on accept.
    pub max_connections: usize,
    /// Time allowed for a request's headers (and idle keep-alive).
    pub header_read_timeout: Duration,
    /// Time allowed for reading a request body.
    pub request_timeout: Duration,
    /// Longest life of one `GET /mcp` stream.
    pub max_stream_lifetime: Duration,
    /// How long graceful shutdown waits for open connections.
    pub shutdown_timeout: Duration,
}

impl std::fmt::Debug for HttpConfig {
    // Never print the token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpConfig")
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("cors_origins", &self.cors_origins)
            .field("max_body_bytes", &self.max_body_bytes)
            .field("max_sessions", &self.max_sessions)
            .field("session_ttl", &self.session_ttl)
            .field("max_streams", &self.max_streams)
            .field("keep_alive", &self.keep_alive)
            .field("allowed_hosts", &self.allowed_hosts)
            .field("max_connections", &self.max_connections)
            .field("header_read_timeout", &self.header_read_timeout)
            .field("request_timeout", &self.request_timeout)
            .field("max_stream_lifetime", &self.max_stream_lifetime)
            .field("shutdown_timeout", &self.shutdown_timeout)
            .finish()
    }
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            token: None,
            cors_origins: Vec::new(),
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            max_sessions: 1024,
            session_ttl: Duration::from_secs(3600),
            max_streams: 64,
            keep_alive: Duration::from_secs(15),
            allowed_hosts: Vec::new(),
            max_connections: DEFAULT_MAX_CONNECTIONS,
            header_read_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            max_stream_lifetime: Duration::from_secs(3600),
            shutdown_timeout: Duration::from_secs(5),
        }
    }
}

struct Session {
    last_seen: Instant,
    /// Dropped with the session; open SSE streams of the session then end.
    closed: watch::Sender<()>,
    /// The protocol version negotiated by its `initialize`.
    protocol: Arc<str>,
}

/// A live session, as seen by one request.
pub struct SessionRef {
    /// Errors once the session is gone.
    pub closed: watch::Receiver<()>,
    pub protocol: Arc<str>,
}

struct Table {
    map: HashMap<String, Session>,
    last_sweep: Instant,
}

/// Bounded, expiring session table. Expired sessions are swept on create and,
/// at most every `min(ttl, 60 s)`, on lookup.
pub struct Sessions {
    table: Mutex<Table>,
    max: usize,
    ttl: Duration,
}

impl Sessions {
    pub fn new(max: usize, ttl: Duration) -> Self {
        Self { table: Mutex::new(Table { map: HashMap::new(), last_sweep: Instant::now() }), max: max.max(1), ttl }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn sweep(&self, t: &mut Table, now: Instant) {
        t.map.retain(|_, s| now.duration_since(s.last_seen) < self.ttl);
        t.last_sweep = now;
    }

    /// Creates a session for `protocol`, evicting expired ones and then the
    /// least recently used.
    pub fn create(&self, protocol: &str) -> Option<String> {
        let id = new_session_id()?;
        let now = Instant::now();
        let mut t = self.lock();
        self.sweep(&mut t, now);
        while t.map.len() >= self.max {
            let Some(oldest) = t.map.iter().min_by_key(|(_, s)| s.last_seen).map(|(k, _)| k.clone()) else { break };
            t.map.remove(&oldest);
        }
        t.map.insert(id.clone(), Session { last_seen: now, closed: watch::channel(()).0, protocol: protocol.into() });
        Some(id)
    }

    /// Marks a session used; `None` when unknown or expired.
    pub fn touch(&self, id: &str) -> Option<SessionRef> {
        let now = Instant::now();
        let mut t = self.lock();
        if now.duration_since(t.last_sweep) >= self.ttl.min(Duration::from_secs(60)) {
            self.sweep(&mut t, now);
        }
        let expired = match t.map.get_mut(id) {
            None => return None,
            Some(s) if now.duration_since(s.last_seen) >= self.ttl => true,
            Some(s) => {
                s.last_seen = now;
                return Some(SessionRef { closed: s.closed.subscribe(), protocol: Arc::clone(&s.protocol) });
            }
        };
        if expired {
            t.map.remove(id);
        }
        None
    }

    pub fn remove(&self, id: &str) -> bool {
        self.lock().map.remove(id).is_some()
    }

    pub fn len(&self) -> usize {
        self.lock().map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 128 random bits as 32 hex characters.
fn new_session_id() -> Option<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).ok()?;
    Some(b.iter().map(|x| format!("{x:02x}")).collect())
}

#[derive(Clone)]
struct AppState {
    dispatcher: Arc<Dispatcher>,
    cfg: Arc<HttpConfig>,
    sessions: Arc<Sessions>,
    streams: Arc<Semaphore>,
    shutdown: Shutdown,
    /// Validate `Host` against the loopback names (loopback bind).
    check_host: bool,
}

/// The `/mcp` + `/healthz` router. `check_host` enables the loopback `Host`
/// check ([`serve`] sets it for loopback binds).
pub fn router(dispatcher: Arc<Dispatcher>, cfg: HttpConfig, shutdown: Shutdown, check_host: bool) -> Router {
    let state = AppState {
        dispatcher,
        sessions: Arc::new(Sessions::new(cfg.max_sessions, cfg.session_ttl)),
        streams: Arc::new(Semaphore::new(cfg.max_streams.max(1))),
        check_host: check_host && !cfg.allowed_hosts.iter().any(|h| h == "*"),
        cfg: Arc::new(cfg),
        shutdown,
    };
    Router::new()
        .route("/mcp", get(mcp_get).post(mcp_post).delete(mcp_delete).options(|| async { StatusCode::NO_CONTENT }))
        .route_layer(middleware::from_fn_with_state(state.clone(), guard))
        .route("/healthz", get(|| async { json_response(StatusCode::OK, r#"{"status":"ok"}"#.into()) }))
        .layer(middleware::from_fn_with_state(state.clone(), deadline))
        .with_state(state)
}

/// Serves until `shutdown` triggers, then drains open connections for at
/// most [`HttpConfig::shutdown_timeout`] (open SSE streams end at shutdown).
pub async fn serve(
    listener: tokio::net::TcpListener,
    dispatcher: Arc<Dispatcher>,
    cfg: HttpConfig,
    shutdown: Shutdown,
) -> std::io::Result<()> {
    let local = listener.local_addr()?;
    let header_timeout = cfg.header_read_timeout;
    let drain_timeout = cfg.shutdown_timeout;
    let conns = Arc::new(Semaphore::new(cfg.max_connections.max(1)));
    let app = router(dispatcher, cfg, shutdown.clone(), local.ip().is_loopback());
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        let accepted = tokio::select! {
            _ = shutdown.wait() => break,
            Some(_) = tasks.join_next(), if !tasks.is_empty() => continue,
            a = listener.accept() => a,
        };
        let stream = match accepted {
            Ok((stream, _)) => stream,
            Err(e) => {
                // EMFILE / ENFILE / aborted handshakes: back off instead of spinning.
                eprintln!("sillage: accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let Ok(permit) = Arc::clone(&conns).try_acquire_owned() else {
            drop(stream); // over the connection cap: shed it, freeing its descriptor at once
            continue;
        };
        let svc = TowerToHyperService::new(app.clone());
        let sd = shutdown.clone();
        tasks.spawn(async move {
            let _permit = permit;
            let _ = stream.set_nodelay(true);
            let mut builder = hyper::server::conn::http1::Builder::new();
            builder.timer(TokioTimer::new()).header_read_timeout(header_timeout);
            let conn = builder.serve_connection(TokioIo::new(stream), svc);
            tokio::pin!(conn);
            tokio::select! {
                _ = conn.as_mut() => {}
                _ = sd.wait() => {
                    conn.as_mut().graceful_shutdown();
                    let _ = conn.await;
                }
            }
        });
    }
    drop(listener);
    let drain = async { while tasks.join_next().await.is_some() {} };
    if tokio::time::timeout(drain_timeout, drain).await.is_err() {
        eprintln!("sillage: shutdown deadline reached; dropping {} open connection(s)", tasks.len());
        tasks.shutdown().await;
    }
    Ok(())
}

/// Backstop deadline on every request except the SSE stream (`GET /mcp`):
/// body read + the slowest tool call + slack.
async fn deadline(State(st): State<AppState>, req: Request, next: Next) -> Response {
    if req.method() == Method::GET && req.uri().path() == "/mcp" {
        return next.run(req).await;
    }
    let limit = st.cfg.request_timeout + st.dispatcher.tool_limits().timeout + Duration::from_secs(5);
    match tokio::time::timeout(limit, next.run(req)).await {
        Ok(r) => r,
        Err(_) => plain(StatusCode::SERVICE_UNAVAILABLE, "request timed out"),
    }
}

fn json_response(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))], body).into_response()
}

fn plain(status: StatusCode, msg: &'static str) -> Response {
    json_response(status, serde_json::json!({ "error": msg }).to_string())
}

fn header_str<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

fn origin_allowed(cfg: &HttpConfig, origin: &str) -> bool {
    cfg.cors_origins.iter().any(|o| o == "*" || o.eq_ignore_ascii_case(origin))
}

fn cors_headers(cfg: &HttpConfig, origin: Option<&str>, h: &mut HeaderMap) {
    let Some(origin) = origin else { return };
    let allow = if cfg.cors_origins.iter().any(|o| o == "*") {
        HeaderValue::from_static("*")
    } else {
        match HeaderValue::from_str(origin) {
            Ok(v) => v,
            Err(_) => return,
        }
    };
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, allow);
    h.insert(header::ACCESS_CONTROL_EXPOSE_HEADERS, HeaderValue::from_static("Mcp-Session-Id"));
    h.append(header::VARY, HeaderValue::from_static("Origin"));
}

fn authorized(cfg: &HttpConfig, headers: &HeaderMap) -> bool {
    let Some(token) = &cfg.token else { return true };
    let Some(value) = headers.get(header::AUTHORIZATION) else { return false };
    let v = value.as_bytes();
    if v.len() < 7 || !v[..7].eq_ignore_ascii_case(b"bearer ") {
        return false;
    }
    let presented = v[7..].trim_ascii();
    // Equal-length digests: the comparison leaks neither content nor length.
    let (a, b) = (Sha256::digest(presented), Sha256::digest(token.as_bytes()));
    bool::from(a.as_slice().ct_eq(b.as_slice()))
}

/// `Host` without its `:port` (`[::1]:80` → `[::1]`).
fn host_name(host: &str) -> &str {
    let host = host.trim();
    if host.starts_with('[') {
        return host.find(']').map_or(host, |i| &host[..=i]);
    }
    match host.rsplit_once(':') {
        Some((h, port)) if !h.contains(':') && port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    }
}

/// On a loopback bind, `Host` must be a loopback name or an allowed host.
fn host_allowed(cfg: &HttpConfig, headers: &HeaderMap) -> bool {
    let Some(raw) = header_str(headers, "host") else { return false };
    let name = host_name(raw);
    LOOPBACK_HOSTS.iter().any(|h| h.eq_ignore_ascii_case(name))
        || cfg.allowed_hosts.iter().any(|h| h.eq_ignore_ascii_case(name) || h.eq_ignore_ascii_case(raw.trim()))
}

fn protocol_header_ok(headers: &HeaderMap) -> bool {
    header_str(headers, PROTOCOL_HEADER).is_none_or(|v| SUPPORTED_PROTOCOL_VERSIONS.contains(&v.trim()))
}

/// Host and origin checks, CORS (incl. preflight), auth and (except for
/// `POST`, checked once the body shows whether it is `initialize`) the
/// protocol-version check for `/mcp`.
async fn guard(State(st): State<AppState>, req: Request, next: Next) -> Response {
    if st.check_host && !host_allowed(&st.cfg, req.headers()) {
        return plain(StatusCode::FORBIDDEN, "host not allowed");
    }
    let origin = header_str(req.headers(), "origin").map(str::to_string);
    if let Some(o) = &origin
        && !origin_allowed(&st.cfg, o)
    {
        return plain(StatusCode::FORBIDDEN, "origin not allowed");
    }
    let mut resp = if req.method() == Method::OPTIONS {
        let mut r = StatusCode::NO_CONTENT.into_response();
        let h = r.headers_mut();
        h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, POST, DELETE, OPTIONS"));
        h.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("Content-Type, Authorization, Mcp-Session-Id, Mcp-Protocol-Version, Last-Event-ID"),
        );
        h.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
        r
    } else if !authorized(&st.cfg, req.headers()) {
        let mut r = plain(StatusCode::UNAUTHORIZED, "unauthorized");
        let challenge = if req.headers().contains_key(header::AUTHORIZATION) {
            r#"Bearer realm="sillage", error="invalid_token""#
        } else {
            r#"Bearer realm="sillage""#
        };
        r.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static(challenge));
        r
    } else if req.method() != Method::POST && !protocol_header_ok(req.headers()) {
        plain(StatusCode::BAD_REQUEST, "unsupported Mcp-Protocol-Version")
    } else {
        next.run(req).await
    };
    cors_headers(&st.cfg, origin.as_deref(), resp.headers_mut());
    resp
}

/// Whether an `Accept` header admits `mime` (absent header: yes).
fn accepts(headers: &HeaderMap, mime: &str) -> bool {
    let Some(accept) = header_str(headers, "accept") else { return true };
    let (ty, _) = mime.split_once('/').unwrap_or((mime, ""));
    accept.split(',').map(|p| p.split(';').next().unwrap_or("").trim().to_ascii_lowercase()).any(|p| {
        p == mime || p == "*/*" || p.strip_suffix("/*").is_some_and(|t| t == ty)
    })
}

/// Session of a request: `Ok(None)` when absent, `Err(404)` when unknown.
fn session_of(st: &AppState, headers: &HeaderMap) -> Result<Option<(String, SessionRef)>, Response> {
    match header_str(headers, SESSION_HEADER) {
        None => Ok(None),
        Some(id) => match st.sessions.touch(id) {
            Some(r) => Ok(Some((id.to_string(), r))),
            None => Err(plain(StatusCode::NOT_FOUND, "unknown or expired session; re-initialize")),
        },
    }
}

async fn read_body(body: Body, headers: &HeaderMap, limit: usize, timeout: Duration) -> Result<Bytes, Response> {
    let declared = header_str(headers, "content-length").and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > limit as u64) {
        return Err(plain(StatusCode::PAYLOAD_TOO_LARGE, "request body too large"));
    }
    let Ok(collected) = tokio::time::timeout(timeout, Limited::new(body, limit).collect()).await else {
        return Err(plain(StatusCode::REQUEST_TIMEOUT, "request body not received in time"));
    };
    match collected {
        Ok(c) => Ok(c.to_bytes()),
        Err(e) if e.downcast_ref::<LengthLimitError>().is_some() => {
            Err(plain(StatusCode::PAYLOAD_TOO_LARGE, "request body too large"))
        }
        Err(_) => Err(plain(StatusCode::BAD_REQUEST, "could not read request body")),
    }
}

/// Whether a `Content-Type` value's media type is `application/json`.
fn is_json_media_type(value: &str) -> bool {
    value.split(';').next().is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
}

async fn mcp_post(State(st): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let headers = &parts.headers;
    if !header_str(headers, "content-type").is_some_and(is_json_media_type) {
        return plain(StatusCode::UNSUPPORTED_MEDIA_TYPE, "Content-Type must be application/json");
    }
    if !accepts(headers, "application/json") {
        return plain(StatusCode::NOT_ACCEPTABLE, "Accept must include application/json");
    }
    let bytes = match read_body(body, headers, st.cfg.max_body_bytes, st.cfg.request_timeout).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let msg: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            let err = protocol::error_response(Value::Null, PARSE_ERROR, format!("parse error: {e}"));
            return json_response(StatusCode::BAD_REQUEST, err.to_string());
        }
    };
    let is_initialize = protocol::classify(&msg) == MessageKind::Request && msg["method"] == "initialize";
    // `initialize` negotiates the version, so it is exempt from the header check.
    if !is_initialize && !protocol_header_ok(headers) {
        return plain(StatusCode::BAD_REQUEST, "unsupported Mcp-Protocol-Version");
    }
    if protocol::classify(&msg) == MessageKind::Invalid && !msg.is_array() {
        return json_response(StatusCode::BAD_REQUEST, protocol::invalid_request(&msg).to_string());
    }
    let session = if is_initialize {
        None
    } else {
        match session_of(&st, headers) {
            Ok(s) => s,
            Err(r) => return r,
        }
    };
    if msg.is_array() {
        let declared = header_str(headers, PROTOCOL_HEADER).map(str::trim);
        let version = declared.or(session.as_ref().map(|(_, s)| &*s.protocol));
        if version == Some(NO_BATCH_PROTOCOL_VERSION) {
            let err = protocol::error_response(
                Value::Null,
                INVALID_REQUEST,
                format!("JSON-RPC batches are not supported in protocol {NO_BATCH_PROTOCOL_VERSION}; send one message per request"),
            );
            return json_response(StatusCode::BAD_REQUEST, err.to_string());
        }
    }
    let Some(reply) = st.dispatcher.handle_value(msg).await else {
        return StatusCode::ACCEPTED.into_response();
    };
    let mut resp = json_response(StatusCode::OK, reply.to_string());
    if is_initialize && let Some(version) = reply["result"]["protocolVersion"].as_str() {
        match st.sessions.create(version) {
            Some(id) => {
                if let Ok(v) = HeaderValue::from_str(&id) {
                    resp.headers_mut().insert(SESSION_HEADER, v);
                }
            }
            None => return plain(StatusCode::INTERNAL_SERVER_ERROR, "could not create a session"),
        }
    }
    resp
}

/// State of one `GET /mcp` stream.
struct Stream {
    opened: bool,
    id: String,
    closed: watch::Receiver<()>,
    sessions: Arc<Sessions>,
    shutdown: Shutdown,
    ticks: tokio::time::Interval,
    deadline: tokio::time::Instant,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

async fn mcp_get(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let wants_sse = header_str(&headers, "accept").is_some_and(|a| a.to_ascii_lowercase().contains("text/event-stream"));
    if !wants_sse {
        return plain(StatusCode::NOT_ACCEPTABLE, "GET /mcp needs Accept: text/event-stream");
    }
    let (id, session) = match session_of(&st, &headers) {
        Ok(Some(s)) => s,
        Ok(None) => return plain(StatusCode::BAD_REQUEST, "GET /mcp needs an Mcp-Session-Id header; initialize first"),
        Err(r) => return r,
    };
    let Ok(permit) = Arc::clone(&st.streams).try_acquire_owned() else {
        return plain(StatusCode::SERVICE_UNAVAILABLE, "too many open streams");
    };
    let every = st.cfg.keep_alive.max(Duration::from_millis(10));
    let now = tokio::time::Instant::now();
    let state = Stream {
        opened: false,
        id,
        closed: session.closed,
        sessions: Arc::clone(&st.sessions),
        shutdown: st.shutdown.clone(),
        ticks: tokio::time::interval_at(now + every, every),
        deadline: now + st.cfg.max_stream_lifetime,
        _permit: permit,
    };
    // One comment right away, then a keep-alive comment per tick (each one
    // refreshes the session) until shutdown, session end or the lifetime cap.
    let stream = futures_util::stream::unfold(state, |mut s| async move {
        if !s.opened {
            s.opened = true;
            return Some((Ok::<Event, Infallible>(Event::default().comment("sillage stream open")), s));
        }
        loop {
            tokio::select! {
                _ = s.shutdown.wait() => return None,
                _ = tokio::time::sleep_until(s.deadline) => return None,
                changed = s.closed.changed() => {
                    if changed.is_err() {
                        return None; // session deleted or evicted
                    }
                }
                _ = s.ticks.tick() => {
                    s.sessions.touch(&s.id)?;
                    return Some((Ok(Event::default().comment("keep-alive")), s));
                }
            }
        }
    });
    Sse::new(stream).into_response()
}

async fn mcp_delete(State(st): State<AppState>, headers: HeaderMap) -> Response {
    match header_str(&headers, SESSION_HEADER) {
        None => plain(StatusCode::BAD_REQUEST, "DELETE /mcp needs an Mcp-Session-Id header"),
        Some(id) if st.sessions.remove(id) => StatusCode::NO_CONTENT.into_response(),
        Some(_) => plain(StatusCode::NOT_FOUND, "unknown or expired session"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_are_bounded_and_expire() {
        let s = Sessions::new(2, Duration::from_secs(60));
        let a = s.create("2025-06-18").unwrap();
        let b = s.create("2025-03-26").unwrap();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert_eq!(&*s.touch(&a).unwrap().protocol, "2025-06-18"); // a is now more recent than b
        let c = s.create("2025-06-18").unwrap();
        assert_eq!(s.len(), 2);
        assert!(s.touch(&b).is_none(), "least recently used evicted");
        assert!(s.touch(&a).is_some() && s.touch(&c).is_some());
        assert!(s.remove(&a) && !s.remove(&a));

        let short = Sessions::new(4, Duration::from_millis(0));
        let x = short.create("2025-06-18").unwrap();
        assert!(short.touch(&x).is_none(), "expired");
    }

    #[test]
    fn expired_sessions_are_swept_on_lookup() {
        let s = Sessions::new(16, Duration::from_millis(30));
        for _ in 0..5 {
            s.create("2025-06-18").unwrap();
        }
        assert_eq!(s.len(), 5);
        std::thread::sleep(Duration::from_millis(40));
        assert!(s.touch("unknown").is_none());
        assert!(s.is_empty(), "lookups sweep expired sessions");
    }

    #[test]
    fn host_header_checks() {
        let cfg = HttpConfig { allowed_hosts: vec!["mcp.internal".into()], ..HttpConfig::default() };
        let check = |v: &str| {
            let mut h = HeaderMap::new();
            h.insert(header::HOST, HeaderValue::from_str(v).unwrap());
            host_allowed(&cfg, &h)
        };
        for ok in ["localhost", "localhost:8765", "127.0.0.1:1", "[::1]:8765", "[::1]", "LOCALHOST", "mcp.internal:443"] {
            assert!(check(ok), "{ok}");
        }
        for bad in ["evil.example", "evil.example:8765", "127.0.0.1.evil.example", "localhost.evil", "[::2]:1"] {
            assert!(!check(bad), "{bad}");
        }
        assert!(!host_allowed(&cfg, &HeaderMap::new()), "missing Host");
        assert_eq!(host_name("a:b:c"), "a:b:c");
    }

    #[test]
    fn json_media_type_parsing() {
        for ok in ["application/json", "Application/JSON; charset=utf-8", " application/json ;x=y"] {
            assert!(is_json_media_type(ok), "{ok}");
        }
        for bad in ["text/plain; x=application/json", "application/json-seq", "text/plain", "application/jsonx"] {
            assert!(!is_json_media_type(bad), "{bad}");
        }
    }

    #[test]
    fn bearer_auth_is_exact() {
        let cfg = HttpConfig { token: Some("s3cret".into()), ..HttpConfig::default() };
        let mut h = HeaderMap::new();
        assert!(!authorized(&cfg, &h));
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer s3cret"));
        assert!(authorized(&cfg, &h));
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("bearer s3cret"));
        assert!(authorized(&cfg, &h));
        for bad in ["Bearer s3cre", "Bearer s3cret2", "Basic s3cret", "s3cret", "Bearer ", "Bearer s3cret s3cret"] {
            h.insert(header::AUTHORIZATION, HeaderValue::from_static(bad));
            assert!(!authorized(&cfg, &h), "{bad}");
        }
        assert!(authorized(&HttpConfig::default(), &HeaderMap::new()));
        assert!(!format!("{cfg:?}").contains("s3cret"));
    }

    #[test]
    fn accept_matching() {
        let mut h = HeaderMap::new();
        assert!(accepts(&h, "application/json"));
        h.insert("accept", HeaderValue::from_static("application/json, text/event-stream"));
        assert!(accepts(&h, "application/json"));
        h.insert("accept", HeaderValue::from_static("text/event-stream"));
        assert!(!accepts(&h, "application/json"));
        h.insert("accept", HeaderValue::from_static("application/*;q=0.9"));
        assert!(accepts(&h, "application/json"));
    }
}
