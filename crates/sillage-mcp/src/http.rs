//! MCP Streamable HTTP transport.
//!
//! - `POST /mcp`: one JSON-RPC message (or batch) per request. Requests get
//!   `200 application/json`; notifications and client responses `202`.
//! - `GET /mcp`: an SSE stream (`text/event-stream`) for server-initiated
//!   messages. This server sends none, so the stream carries only keep-alive
//!   comments; it ends on shutdown or when its session is deleted.
//! - `DELETE /mcp`: terminates the session named by `Mcp-Session-Id` (`204`).
//! - `OPTIONS /mcp`: CORS preflight.
//! - `GET /healthz`: unauthenticated `{"status":"ok"}`.
//!
//! **Sessions.** A successful `initialize` issues a random `Mcp-Session-Id`.
//! Requests carrying an `Mcp-Session-Id` must name a live session (else
//! `404`, after which clients re-initialize). Requests without one are served
//! statelessly: every tool is a pure function of its arguments and the corpus,
//! so no per-session state is needed. Sessions expire after
//! [`HttpConfig::session_ttl`] of inactivity and at most
//! [`HttpConfig::max_sessions`] are kept (least recently used evicted).
//!
//! **Security.** With a token, every `/mcp` request needs
//! `Authorization: Bearer <token>` (compared in constant time); failures get
//! `401` with `WWW-Authenticate: Bearer`. Browser requests (with an `Origin`
//! header) are rejected with `403` unless the origin is allowed by
//! [`HttpConfig::cors_origins`] (DNS-rebinding protection); non-browser
//! clients send no `Origin`. Bodies over [`HttpConfig::max_body_bytes`] get
//! `413` without being buffered.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use http_body_util::{BodyExt, LengthLimitError, Limited};
use serde_json::Value;
use subtle::ConstantTimeEq;
use tokio::sync::{Semaphore, watch};

use crate::Shutdown;
use crate::protocol::{self, Dispatcher, INVALID_REQUEST, MessageKind, PARSE_ERROR, SUPPORTED_PROTOCOL_VERSIONS};

pub const SESSION_HEADER: &str = "mcp-session-id";
pub const PROTOCOL_HEADER: &str = "mcp-protocol-version";
pub const DEFAULT_MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

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
        }
    }
}

struct Session {
    last_seen: Instant,
    /// Dropped with the session; open SSE streams of the session then end.
    closed: watch::Sender<()>,
}

/// Bounded, expiring session table.
pub struct Sessions {
    map: Mutex<HashMap<String, Session>>,
    max: usize,
    ttl: Duration,
}

impl Sessions {
    pub fn new(max: usize, ttl: Duration) -> Self {
        Self { map: Mutex::new(HashMap::new()), max: max.max(1), ttl }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Session>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Creates a session, evicting expired ones and then the least recently used.
    pub fn create(&self) -> Option<String> {
        let id = new_session_id()?;
        let now = Instant::now();
        let mut map = self.lock();
        map.retain(|_, s| now.duration_since(s.last_seen) < self.ttl);
        while map.len() >= self.max {
            let Some(oldest) = map.iter().min_by_key(|(_, s)| s.last_seen).map(|(k, _)| k.clone()) else { break };
            map.remove(&oldest);
        }
        map.insert(id.clone(), Session { last_seen: now, closed: watch::channel(()).0 });
        Some(id)
    }

    /// Marks a session used; `None` when unknown or expired. Returns a
    /// receiver that errors once the session is gone.
    pub fn touch(&self, id: &str) -> Option<watch::Receiver<()>> {
        let now = Instant::now();
        let mut map = self.lock();
        let expired = match map.get_mut(id) {
            None => return None,
            Some(s) if now.duration_since(s.last_seen) >= self.ttl => true,
            Some(s) => {
                s.last_seen = now;
                return Some(s.closed.subscribe());
            }
        };
        if expired {
            map.remove(id);
        }
        None
    }

    pub fn remove(&self, id: &str) -> bool {
        self.lock().remove(id).is_some()
    }

    pub fn len(&self) -> usize {
        self.lock().len()
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
}

/// The `/mcp` + `/healthz` router.
pub fn router(dispatcher: Arc<Dispatcher>, cfg: HttpConfig, shutdown: Shutdown) -> Router {
    let state = AppState {
        dispatcher,
        sessions: Arc::new(Sessions::new(cfg.max_sessions, cfg.session_ttl)),
        streams: Arc::new(Semaphore::new(cfg.max_streams.max(1))),
        cfg: Arc::new(cfg),
        shutdown,
    };
    Router::new()
        .route("/mcp", get(mcp_get).post(mcp_post).delete(mcp_delete).options(|| async { StatusCode::NO_CONTENT }))
        .route_layer(middleware::from_fn_with_state(state.clone(), guard))
        .route("/healthz", get(|| async { json_response(StatusCode::OK, r#"{"status":"ok"}"#.into()) }))
        .with_state(state)
}

/// Serves until `shutdown` triggers; open SSE streams end at shutdown.
pub async fn serve(
    listener: tokio::net::TcpListener,
    dispatcher: Arc<Dispatcher>,
    cfg: HttpConfig,
    shutdown: Shutdown,
) -> std::io::Result<()> {
    let app = router(dispatcher, cfg, shutdown.clone());
    let sd = shutdown.clone();
    axum::serve(listener, app).with_graceful_shutdown(async move { sd.wait().await }).await
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
    bool::from(presented.ct_eq(token.as_bytes()))
}

/// Origin check, CORS (incl. preflight), auth and protocol-version check for `/mcp`.
async fn guard(State(st): State<AppState>, req: Request, next: Next) -> Response {
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
    } else if header_str(req.headers(), PROTOCOL_HEADER).is_some_and(|v| !SUPPORTED_PROTOCOL_VERSIONS.contains(&v.trim())) {
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

/// Session id of a request: `Ok(None)` when absent, `Err(404)` when unknown.
fn session_of(st: &AppState, headers: &HeaderMap) -> Result<Option<(String, watch::Receiver<()>)>, Response> {
    match header_str(headers, SESSION_HEADER) {
        None => Ok(None),
        Some(id) => match st.sessions.touch(id) {
            Some(rx) => Ok(Some((id.to_string(), rx))),
            None => Err(plain(StatusCode::NOT_FOUND, "unknown or expired session; re-initialize")),
        },
    }
}

async fn read_body(body: Body, headers: &HeaderMap, limit: usize) -> Result<Bytes, Response> {
    let declared = header_str(headers, "content-length").and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > limit as u64) {
        return Err(plain(StatusCode::PAYLOAD_TOO_LARGE, "request body too large"));
    }
    match Limited::new(body, limit).collect().await {
        Ok(c) => Ok(c.to_bytes()),
        Err(e) if e.downcast_ref::<LengthLimitError>().is_some() => {
            Err(plain(StatusCode::PAYLOAD_TOO_LARGE, "request body too large"))
        }
        Err(_) => Err(plain(StatusCode::BAD_REQUEST, "could not read request body")),
    }
}

async fn mcp_post(State(st): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let headers = &parts.headers;
    let is_json = header_str(headers, "content-type").is_some_and(|c| c.to_ascii_lowercase().contains("application/json"));
    if !is_json {
        return plain(StatusCode::UNSUPPORTED_MEDIA_TYPE, "Content-Type must be application/json");
    }
    if !accepts(headers, "application/json") {
        return plain(StatusCode::NOT_ACCEPTABLE, "Accept must include application/json");
    }
    let bytes = match read_body(body, headers, st.cfg.max_body_bytes).await {
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
    if protocol::classify(&msg) == MessageKind::Invalid && !msg.is_array() {
        let err = protocol::error_response(Value::Null, INVALID_REQUEST, "invalid JSON-RPC 2.0 message");
        return json_response(StatusCode::BAD_REQUEST, err.to_string());
    }
    if !is_initialize && let Err(r) = session_of(&st, headers) {
        return r;
    }
    let Some(reply) = st.dispatcher.handle_value(msg).await else {
        return StatusCode::ACCEPTED.into_response();
    };
    let mut resp = json_response(StatusCode::OK, reply.to_string());
    if is_initialize && reply.get("result").is_some() {
        match st.sessions.create() {
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

async fn mcp_get(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let wants_sse = header_str(&headers, "accept").is_some_and(|a| a.to_ascii_lowercase().contains("text/event-stream"));
    if !wants_sse {
        return plain(StatusCode::NOT_ACCEPTABLE, "GET /mcp needs Accept: text/event-stream");
    }
    let session = match session_of(&st, &headers) {
        Ok(s) => s.map(|(_, rx)| rx),
        Err(r) => return r,
    };
    let Ok(permit) = Arc::clone(&st.streams).try_acquire_owned() else {
        return plain(StatusCode::SERVICE_UNAVAILABLE, "too many open streams");
    };
    let shutdown = st.shutdown.clone();
    // One comment right away, then nothing until shutdown / session end
    // (keep-alive comments fill the gap).
    let opened = futures_util::stream::once(async { Ok::<Event, Infallible>(Event::default().comment("sillage stream open")) });
    let closed = futures_util::stream::once(async move {
        let _permit = permit;
        let session_end = async move {
            match session {
                Some(mut rx) => while rx.changed().await.is_ok() {},
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = shutdown.wait() => {}
            _ = session_end => {}
        }
        None::<Result<Event, Infallible>>
    });
    use futures_util::StreamExt;
    let stream = opened.chain(closed.filter_map(std::future::ready));
    Sse::new(stream).keep_alive(KeepAlive::new().interval(st.cfg.keep_alive).text("keep-alive")).into_response()
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
        let a = s.create().unwrap();
        let b = s.create().unwrap();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(s.touch(&a).is_some()); // a is now more recent than b
        let c = s.create().unwrap();
        assert_eq!(s.len(), 2);
        assert!(s.touch(&b).is_none(), "least recently used evicted");
        assert!(s.touch(&a).is_some() && s.touch(&c).is_some());
        assert!(s.remove(&a) && !s.remove(&a));

        let short = Sessions::new(4, Duration::from_millis(0));
        let x = short.create().unwrap();
        assert!(short.touch(&x).is_none(), "expired");
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
        for bad in ["Bearer s3cre", "Bearer s3cret2", "Basic s3cret", "s3cret", "Bearer "] {
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
