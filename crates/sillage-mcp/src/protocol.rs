//! Transport-independent MCP JSON-RPC 2.0 dispatch.
//!
//! Handles `initialize`, `ping`, `tools/list`, `tools/call` and notifications
//! (`notifications/initialized` and any other: no response). Responses sent by
//! the client (we never send requests) are ignored. Batches (JSON arrays,
//! protocol 2025-03-26) of at most [`MAX_BATCH`] messages are accepted; a
//! batch of only notifications yields no response. (Protocol 2025-06-18
//! removed batching; the HTTP transport rejects batches on sessions that
//! negotiated it.)
//!
//! Tool calls run on the blocking pool behind a global concurrency limit and
//! a per-call timeout ([`ToolLimits`]), so a flood of calls cannot pin every
//! blocking thread or queue unbounded work.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;

use serde_json::{Map, Value, json};

use crate::registry::CorpusRegistry;
use crate::tools;

/// Protocol versions this server speaks, newest first.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
/// Answered to `initialize` requests for any other version: the latest one.
pub const FALLBACK_PROTOCOL_VERSION: &str = "2025-06-18";
/// The first protocol version without JSON-RPC batching.
pub const NO_BATCH_PROTOCOL_VERSION: &str = "2025-06-18";
/// Largest JSON-RPC batch accepted (larger ones are `-32600`).
pub const MAX_BATCH: usize = 32;
/// Default per-call tool timeout.
pub const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(30);

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

/// When agents should call each tool (sent as `initialize.instructions`).
pub const INSTRUCTIONS: &str = "\
sillage indexes this codebase structurally (Aroma-style AST features) and answers \
\"how does THIS repo write this?\" with file:line citations. Every tool stays silent \
rather than guess: an empty result means no repo consensus, so keep your own code.

When to call:
- find_idioms: right after drafting a function body or snippet, before finalizing it. \
Returns the repo's consensus completion as a diff in YOUR variable names, plus the imports \
the added lines need.
- find_similar_code: before writing a helper or pattern that may already exist, or to find \
call sites/implementations shaped like a snippet (scope=prod|test).
- check_missing_guards: on finished code or a raw `git diff` before committing. Reports only \
error/null/bounds checks, cleanup, logging or validation that most similar repo code has and \
yours lacks.
- find_test_idioms: when writing tests for new code. Returns the repo's tests for similar code \
as skeletons renamed to your function, with assertions, fixtures and test imports.
- corpus_status / refresh_corpus: check what is indexed; call refresh_corpus after creating or \
editing files so new code becomes searchable.

Tips: pass exclude_path=<the file you are editing> so it never matches itself; `lang` is one of \
java, python, javascript, typescript, tsx, go, rust; open cited file:line locations for context.";

/// Bounds on tool execution shared by every transport.
#[derive(Clone)]
pub struct ToolLimits {
    /// Concurrent tool calls running on the blocking pool.
    pub permits: Arc<Semaphore>,
    /// Wall-clock limit per call, including the wait for a permit.
    pub timeout: Duration,
}

impl ToolLimits {
    pub fn new(max_concurrent: usize, timeout: Duration) -> Self {
        Self { permits: Arc::new(Semaphore::new(max_concurrent.max(1))), timeout }
    }

    /// `2 × available_parallelism`, clamped to `4..=32`.
    pub fn default_concurrency() -> usize {
        let n = std::thread::available_parallelism().map_or(4, |n| n.get());
        (2 * n).clamp(4, 32)
    }
}

impl Default for ToolLimits {
    fn default() -> Self {
        Self::new(Self::default_concurrency(), DEFAULT_TOOL_TIMEOUT)
    }
}

/// The MCP request dispatcher shared by all transports.
pub struct Dispatcher {
    registry: Arc<CorpusRegistry>,
    limits: ToolLimits,
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }
}

/// A JSON-RPC error response.
pub fn error_response(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

fn result_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// The protocol version to answer an `initialize` request with.
pub fn negotiate_version(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|r| SUPPORTED_PROTOCOL_VERSIONS.iter().copied().find(|v| *v == r))
        .unwrap_or(FALLBACK_PROTOCOL_VERSION)
}

/// The `-32600` reply to an invalid message, echoing its `id` when that is a
/// valid JSON-RPC id (string or number).
pub fn invalid_request(msg: &Value) -> Value {
    let id = msg.get("id").filter(|i| matches!(i, Value::String(_) | Value::Number(_))).cloned().unwrap_or(Value::Null);
    error_response(id, INVALID_REQUEST, "invalid JSON-RPC 2.0 request")
}

/// What kind of JSON-RPC message a value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Request,
    Notification,
    /// A client response (to a server request) — ignored.
    Response,
    Invalid,
}

pub fn classify(msg: &Value) -> MessageKind {
    let Some(obj) = msg.as_object() else { return MessageKind::Invalid };
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return MessageKind::Invalid;
    }
    match (obj.get("method"), obj.get("id")) {
        (Some(Value::String(_)), None) => MessageKind::Notification,
        (Some(Value::String(_)), Some(Value::String(_) | Value::Number(_))) => MessageKind::Request,
        (None, Some(_)) if obj.contains_key("result") || obj.contains_key("error") => MessageKind::Response,
        _ => MessageKind::Invalid,
    }
}

impl Dispatcher {
    pub fn new(registry: Arc<CorpusRegistry>) -> Self {
        Self { registry, limits: ToolLimits::default() }
    }

    /// Replaces the tool concurrency limit and timeout.
    pub fn with_tool_limits(mut self, limits: ToolLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn tool_limits(&self) -> &ToolLimits {
        &self.limits
    }

    pub fn registry(&self) -> &Arc<CorpusRegistry> {
        &self.registry
    }

    /// Handles raw bytes: a parse error yields a `-32700` response.
    pub async fn handle_bytes(&self, bytes: &[u8]) -> Option<Value> {
        match serde_json::from_slice::<Value>(bytes) {
            Ok(v) => self.handle_value(v).await,
            Err(e) => Some(error_response(Value::Null, PARSE_ERROR, format!("parse error: {e}"))),
        }
    }

    /// Handles one message or a batch; `None` when nothing is to be sent back.
    pub async fn handle_value(&self, v: Value) -> Option<Value> {
        match v {
            Value::Array(items) => {
                if items.is_empty() {
                    return Some(error_response(Value::Null, INVALID_REQUEST, "empty batch"));
                }
                if items.len() > MAX_BATCH {
                    return Some(error_response(
                        Value::Null,
                        INVALID_REQUEST,
                        format!("batch of {} messages exceeds the limit of {MAX_BATCH}", items.len()),
                    ));
                }
                let futs = items.into_iter().map(|m| self.handle_message(m));
                let out: Vec<Value> = futures_util::future::join_all(futs).await.into_iter().flatten().collect();
                (!out.is_empty()).then_some(Value::Array(out))
            }
            m => self.handle_message(m).await,
        }
    }

    /// Handles one (non-batch) message.
    pub async fn handle_message(&self, msg: Value) -> Option<Value> {
        match classify(&msg) {
            MessageKind::Notification | MessageKind::Response => None,
            MessageKind::Invalid => Some(invalid_request(&msg)),
            MessageKind::Request => {
                let id = msg["id"].clone();
                let method = msg["method"].as_str().unwrap_or_default();
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                Some(match self.request(method, params).await {
                    Ok(result) => result_response(id, result),
                    Err(e) => error_response(id, e.code, e.message),
                })
            }
        }
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let params = match params {
            Value::Null => Map::new(),
            Value::Object(m) => m,
            _ => return Err(RpcError::invalid_params("params must be an object")),
        };
        match method {
            "initialize" => {
                let requested = params.get("protocolVersion").and_then(Value::as_str);
                Ok(json!({
                    "protocolVersion": negotiate_version(requested),
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "sillage", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools::tool_definitions() })),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| RpcError::invalid_params("tools/call needs a string `name`"))?;
                let args = match params.get("arguments") {
                    None | Some(Value::Null) => Value::Object(Map::new()),
                    Some(a @ Value::Object(_)) => a.clone(),
                    Some(_) => return Err(RpcError::invalid_params("`arguments` must be an object")),
                };
                let out = tools::call_tool(&self.registry, &self.limits, name, args).await?;
                Ok(json!({
                    "content": [{ "type": "text", "text": out.text }],
                    "isError": out.is_error,
                }))
            }
            other => Err(RpcError::new(METHOD_NOT_FOUND, format!("method not found: {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_negotiation() {
        assert_eq!(negotiate_version(Some("2025-06-18")), "2025-06-18");
        assert_eq!(negotiate_version(Some("2025-03-26")), "2025-03-26");
        assert_eq!(negotiate_version(Some("2024-11-05")), "2024-11-05");
        assert_eq!(negotiate_version(Some("2099-01-01")), "2025-06-18", "unknown future version: latest");
        assert_eq!(negotiate_version(None), "2025-06-18");
    }

    #[test]
    fn invalid_requests_echo_valid_ids() {
        assert_eq!(invalid_request(&json!({ "jsonrpc": "1.0", "id": 7, "method": "ping" }))["id"], 7);
        assert_eq!(invalid_request(&json!({ "jsonrpc": "1.0", "id": "a", "method": "ping" }))["id"], "a");
        assert_eq!(invalid_request(&json!({ "jsonrpc": "1.0", "id": [1], "method": "ping" }))["id"], Value::Null);
        assert_eq!(invalid_request(&json!(5))["id"], Value::Null);
    }

    #[test]
    fn default_tool_concurrency_is_bounded() {
        let n = ToolLimits::default_concurrency();
        assert!((4..=32).contains(&n));
        assert_eq!(ToolLimits::new(0, DEFAULT_TOOL_TIMEOUT).permits.available_permits(), 1);
    }
}
