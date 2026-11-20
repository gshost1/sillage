//! End-to-end tests of the Streamable HTTP server on an ephemeral port, over
//! real multi-file repositories written to temporary directories.

mod common;

use std::time::Duration;

use common::*;
use serde_json::json;
use sillage_mcp::{CorpusHandle, HttpConfig};
use sillage_core::Lang;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TOKEN: &str = "test-token-9f8e7d";

fn go_corpus(dir: &std::path::Path, name: &str) -> CorpusHandle {
    go_repo(dir);
    CorpusHandle::open_dir(name, dir, &[]).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn healthz_cors_and_origin_checks() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(vec![go_corpus(dir.path(), "app")], None);
    let cfg = HttpConfig { token: Some(TOKEN.into()), cors_origins: vec!["https://app.example".into()], ..HttpConfig::default() };
    let srv = TestServer::start(reg, cfg).await;

    let r = http(srv.addr, "GET", "/healthz", &[], None).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json(), json!({ "status": "ok" }), "unauthenticated and minimal");
    assert!(!r.body.contains(TOKEN) && !r.body.contains(dir.path().to_str().unwrap()));

    let pre = http(
        srv.addr,
        "OPTIONS",
        "/mcp",
        &[
            ("Origin", "https://app.example"),
            ("Access-Control-Request-Method", "POST"),
            ("Access-Control-Request-Headers", "content-type, authorization, mcp-session-id"),
        ],
        None,
    )
    .await;
    assert_eq!(pre.status, 204, "{pre:?}");
    assert_eq!(pre.header("access-control-allow-origin"), Some("https://app.example"));
    assert_eq!(pre.header("access-control-allow-methods"), Some("GET, POST, DELETE, OPTIONS"));
    let allow = pre.header("access-control-allow-headers").unwrap();
    for h in ["Content-Type", "Authorization", "Mcp-Session-Id", "Mcp-Protocol-Version"] {
        assert!(allow.contains(h), "{allow}");
    }
    assert_eq!(pre.header("access-control-expose-headers"), Some("Mcp-Session-Id"));

    // A foreign browser origin is refused outright (DNS-rebinding protection).
    let evil = http(srv.addr, "OPTIONS", "/mcp", &[("Origin", "https://evil.example")], None).await;
    assert_eq!(evil.status, 403);
    assert!(evil.header("access-control-allow-origin").is_none());
    let auth = format!("Bearer {TOKEN}");
    let evil_post = http(
        srv.addr,
        "POST",
        "/mcp",
        &[("Origin", "https://evil.example"), ("Authorization", &auth), ("Content-Type", "application/json")],
        Some(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#),
    )
    .await;
    assert_eq!(evil_post.status, 403);

    // An allowed origin gets CORS headers on real responses too (even 401s).
    let unauth = http(srv.addr, "POST", "/mcp", &[("Origin", "https://app.example"), ("Content-Type", "application/json")], Some(b"{}")).await;
    assert_eq!(unauth.status, 401);
    assert_eq!(unauth.header("access-control-allow-origin"), Some("https://app.example"));
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bearer_auth_guards_every_mcp_method() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(vec![go_corpus(dir.path(), "app")], None);
    let srv = TestServer::start(reg, HttpConfig { token: Some(TOKEN.into()), ..HttpConfig::default() }).await;
    let ping = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let bad_auths: [&[(&str, &str)]; 4] = [
        &[],
        &[("Authorization", "Bearer wrong-token")],
        &[("Authorization", "Bearer test-token-9f8e7")],
        &[("Authorization", "Basic dGVzdA==")],
    ];
    for auth in bad_auths {
        let mut h = vec![("Content-Type", "application/json"), ("Accept", "application/json, text/event-stream")];
        h.extend_from_slice(auth);
        let post = http(srv.addr, "POST", "/mcp", &h, Some(ping)).await;
        assert_eq!(post.status, 401, "{auth:?}");
        assert!(post.header("www-authenticate").unwrap().starts_with("Bearer"));
        assert!(!post.body.contains(TOKEN));
        let get = http(srv.addr, "GET", "/mcp", &[&[("Accept", "text/event-stream")], auth].concat(), None).await;
        assert_eq!(get.status, 401);
        let del = http(srv.addr, "DELETE", "/mcp", &[&[("Mcp-Session-Id", "abc")], auth].concat(), None).await;
        assert_eq!(del.status, 401);
    }
    let c = Client::new(srv.addr).with_token(TOKEN);
    let ok = c.rpc(1, "ping", json!({})).await;
    assert_eq!(ok.status, 200);
    assert_eq!(ok.json(), json!({ "jsonrpc": "2.0", "id": 1, "result": {} }));
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn body_limit_content_type_and_malformed_json() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(vec![go_corpus(dir.path(), "app")], None);
    let srv = TestServer::start(reg, HttpConfig { max_body_bytes: 1024, ..HttpConfig::default() }).await;
    let big = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"ping","params":{{"pad":"{}"}}}}"#, "x".repeat(4096));
    let json_h = [("Content-Type", "application/json")];
    // Declared length over the limit: rejected before reading the body.
    let r = http(srv.addr, "POST", "/mcp", &json_h, Some(big.as_bytes())).await;
    assert_eq!(r.status, 413, "{r:?}");
    // Chunked body (no length): rejected once the limit is crossed.
    let r = http(srv.addr, "POST", "/mcp", &[("Content-Type", "application/json"), ("Transfer-Encoding", "chunked")], Some(big.as_bytes())).await;
    assert_eq!(r.status, 413, "{r:?}");
    // Under the limit: fine.
    let r = http(srv.addr, "POST", "/mcp", &json_h, Some(br#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#)).await;
    assert_eq!(r.status, 200);

    let r = http(srv.addr, "POST", "/mcp", &[("Content-Type", "text/plain")], Some(b"{}")).await;
    assert_eq!(r.status, 415);
    // The media type is parsed, not substring-matched.
    let r = http(srv.addr, "POST", "/mcp", &[("Content-Type", "text/plain; x=application/json")], Some(b"{}")).await;
    assert_eq!(r.status, 415);
    let r = http(srv.addr, "POST", "/mcp", &[("Content-Type", "Application/JSON; charset=utf-8")], Some(br#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#)).await;
    assert_eq!(r.status, 200);
    // An invalid single message echoes its (valid) id.
    let r = http(srv.addr, "POST", "/mcp", &json_h, Some(br#"{"jsonrpc":"1.0","id":7,"method":"ping"}"#)).await;
    assert_eq!(r.status, 400);
    assert_eq!((r.json()["id"].clone(), r.json()["error"]["code"].clone()), (json!(7), json!(-32600)));
    let r = http(srv.addr, "POST", "/mcp", &json_h, Some(b"{not json")).await;
    assert_eq!(r.status, 400);
    assert_eq!(r.json()["error"]["code"], -32700);
    let r = http(srv.addr, "POST", "/mcp", &json_h, Some(br#"{"id":1,"method":"ping"}"#)).await;
    assert_eq!(r.status, 400);
    assert_eq!(r.json()["error"]["code"], -32600);
    let r = http(srv.addr, "POST", "/mcp", &[("Content-Type", "application/json"), ("Mcp-Protocol-Version", "1999-01-01")], Some(b"{}")).await;
    assert_eq!(r.status, 400);
    let r = http(srv.addr, "POST", "/mcp", &[("Content-Type", "application/json"), ("Mcp-Protocol-Version", "1999-01-01")], Some(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)).await;
    assert_eq!(r.status, 400, "non-initialize requests must declare a supported version");
    // `initialize` negotiates, so an unknown header version does not block it.
    let init = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2030-01-01"}}"#;
    let r = http(srv.addr, "POST", "/mcp", &[("Content-Type", "application/json"), ("Mcp-Protocol-Version", "2030-01-01")], Some(init)).await;
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json()["result"]["protocolVersion"], "2025-06-18");
    let r = http(srv.addr, "DELETE", "/mcp", &[("Mcp-Protocol-Version", "1999-01-01"), ("Mcp-Session-Id", "x")], None).await;
    assert_eq!(r.status, 400);
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_mcp_lifecycle_with_sessions_and_sse() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(vec![go_corpus(dir.path(), "app")], None);
    let srv = TestServer::start(reg, HttpConfig { token: Some(TOKEN.into()), ..HttpConfig::default() }).await;
    let mut c = Client::new(srv.addr).with_token(TOKEN);

    let init = c.initialize().await;
    let v = init.json();
    assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(v["result"]["serverInfo"]["name"], "sillage");
    assert_eq!(v["result"]["capabilities"]["tools"]["listChanged"], false);
    let instructions = v["result"]["instructions"].as_str().unwrap();
    for t in ["find_idioms", "find_similar_code", "check_missing_guards", "find_test_idioms", "refresh_corpus"] {
        assert!(instructions.contains(t), "{instructions}");
    }
    let sid = c.session.clone().expect("Mcp-Session-Id issued");
    assert_eq!(sid.len(), 32);
    assert!(sid.bytes().all(|b| b.is_ascii_hexdigit()));
    // Unknown (future) versions negotiate the latest supported one.
    let old = Client::new(srv.addr).with_token(TOKEN).rpc(1, "initialize", json!({ "protocolVersion": "2030-01-01" })).await;
    assert_eq!(old.json()["result"]["protocolVersion"], "2025-06-18");
    assert_ne!(old.header("mcp-session-id"), Some(sid.as_str()));
    let v2024 = Client::new(srv.addr).with_token(TOKEN).rpc(1, "initialize", json!({ "protocolVersion": "2024-11-05" })).await;
    assert_eq!(v2024.json()["result"]["protocolVersion"], "2024-11-05");

    let note = c.post(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;
    assert_eq!(note.status, 202);
    assert!(note.body.is_empty());

    let list = c.rpc(2, "tools/list", json!({})).await.json();
    let tools = list["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        ["find_idioms", "find_similar_code", "check_missing_guards", "find_test_idioms", "corpus_status", "refresh_corpus"]
    );
    let schema = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap()["inputSchema"].clone();
    assert_eq!(schema("find_idioms")["required"], json!(["snippet", "lang"]));
    assert_eq!(schema("find_idioms")["properties"]["detail"]["enum"], json!(["diff", "compact", "full"]));
    assert_eq!(schema("find_idioms")["properties"]["preset"]["enum"], json!(["repo", "paper"]));
    assert_eq!(schema("find_similar_code")["properties"]["scope"]["enum"], json!(["all", "prod", "test"]));
    assert_eq!(schema("find_similar_code")["properties"]["limit"]["maximum"], 20);
    assert!(schema("check_missing_guards")["properties"]["diff"].is_object());
    assert_eq!(schema("find_test_idioms")["required"], json!(["code", "lang"]));
    assert!(schema("corpus_status")["properties"]["corpus"].is_object());

    let e = c.rpc(3, "resources/list", json!({})).await.json();
    assert_eq!(e["error"]["code"], -32601);
    let e = c.rpc(4, "tools/call", json!({ "name": "nope", "arguments": {} })).await.json();
    assert_eq!(e["error"]["code"], -32602);
    // Batches work on 2025-03-26 sessions and stateless calls; notifications
    // inside yield no entry. 2025-06-18 removed batching: rejected there.
    let batch = json!([
        { "jsonrpc": "2.0", "id": 10, "method": "ping" },
        { "jsonrpc": "2.0", "method": "notifications/progress" },
        { "jsonrpc": "2.0", "id": 11, "method": "ping" }
    ]);
    let mut c25 = Client::new(srv.addr).with_token(TOKEN);
    let init25 = c25.rpc(1, "initialize", json!({ "protocolVersion": "2025-03-26" })).await;
    c25.session = init25.header("mcp-session-id").map(str::to_string);
    assert!(c25.session.is_some());
    assert_eq!(c25.post(&batch).await.json().as_array().unwrap().len(), 2);
    assert_eq!(Client::new(srv.addr).with_token(TOKEN).post(&batch).await.json().as_array().unwrap().len(), 2, "stateless");
    let r = c.post(&batch).await;
    assert_eq!(r.status, 400, "{r:?}");
    assert_eq!(r.json()["error"]["code"], -32600);
    assert!(r.json()["error"]["message"].as_str().unwrap().contains("2025-06-18"));
    let auth = format!("Bearer {TOKEN}");
    let r = http(
        srv.addr,
        "POST",
        "/mcp",
        &[("Authorization", &auth), ("Content-Type", "application/json"), ("Mcp-Protocol-Version", "2025-06-18")],
        Some(batch.to_string().as_bytes()),
    )
    .await;
    assert_eq!(r.status, 400, "declared 2025-06-18: no batches");
    // Oversized batches are rejected whatever the version.
    let big: Vec<_> = (0..33).map(|i| json!({ "jsonrpc": "2.0", "id": i, "method": "ping" })).collect();
    let r = Client::new(srv.addr).with_token(TOKEN).post(&json!(big)).await.json();
    assert_eq!(r["error"]["code"], -32600, "{r}");
    assert!(r["error"]["message"].as_str().unwrap().contains("limit of 32"), "{r}");

    // Stateless calls (no session header) are served too.
    let stateless = Client::new(srv.addr).with_token(TOKEN);
    assert!(stateless.ok("corpus_status", json!({})).await.contains("`app`"));

    // GET /mcp: needs the SSE Accept type and a session; opens a stream bound to it.
    let r = http(srv.addr, "GET", "/mcp", &[("Authorization", &auth), ("Accept", "application/json")], None).await;
    assert_eq!(r.status, 406);
    let r = http(srv.addr, "GET", "/mcp", &[("Authorization", &auth), ("Accept", "text/event-stream"), ("Mcp-Session-Id", "0000")], None).await;
    assert_eq!(r.status, 404);
    let r = http(srv.addr, "GET", "/mcp", &[("Authorization", &auth), ("Accept", "text/event-stream")], None).await;
    assert_eq!(r.status, 400, "GET /mcp without a session");
    assert!(r.body.contains("Mcp-Session-Id"), "{r:?}");
    let mut sse = tokio::net::TcpStream::connect(srv.addr).await.unwrap();
    sse.write_all(
        format!("GET /mcp HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: {auth}\r\nAccept: text/event-stream\r\nMcp-Session-Id: {sid}\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !String::from_utf8_lossy(&head).contains("sillage stream open") {
        let n = tokio::time::timeout(Duration::from_secs(5), sse.read(&mut buf)).await.expect("sse data").unwrap();
        assert!(n > 0, "stream closed early");
        head.extend_from_slice(&buf[..n]);
    }
    let head = String::from_utf8_lossy(&head).to_string();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.to_ascii_lowercase().contains("content-type: text/event-stream"), "{head}");

    // DELETE ends the session and its stream.
    let del = http(srv.addr, "DELETE", "/mcp", &[("Authorization", &auth), ("Mcp-Session-Id", &sid)], None).await;
    assert_eq!(del.status, 204);
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), sse.read_to_end(&mut rest)).await.expect("stream ends with its session").unwrap();
    let after = c.rpc(5, "ping", json!({})).await;
    assert_eq!(after.status, 404, "terminated session id");
    let again = http(srv.addr, "DELETE", "/mcp", &[("Authorization", &auth), ("Mcp-Session-Id", &sid)], None).await;
    assert_eq!(again.status, 404);
    let none = http(srv.addr, "DELETE", "/mcp", &[("Authorization", &auth)], None).await;
    assert_eq!(none.status, 400);

    // An open stream does not block graceful shutdown.
    let mut c2 = Client::new(srv.addr).with_token(TOKEN);
    c2.initialize().await;
    let sid2 = c2.session.clone().unwrap();
    let mut sse2 = tokio::net::TcpStream::connect(srv.addr).await.unwrap();
    sse2.write_all(format!("GET /mcp HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: {auth}\r\nAccept: text/event-stream\r\nMcp-Session-Id: {sid2}\r\n\r\n").as_bytes()).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), sse2.read(&mut buf)).await.unwrap().unwrap();
    srv.stop().await;
    let mut tail = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), sse2.read_to_end(&mut tail)).await.expect("stream closed at shutdown").ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_tools_over_http() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(vec![go_corpus(dir.path(), "app")], None);
    let srv = TestServer::start(reg, HttpConfig::default()).await;
    let mut c = Client::new(srv.addr);
    c.initialize().await;

    // find_idioms: consensus diff in the draft's own variable names.
    let out = c.ok("find_idioms", json!({ "snippet": DRAFT_BODY, "lang": "go" })).await;
    println!("find_idioms:\n{out}");
    assert!(out.contains("confidence high"), "{out}");
    assert!(out.contains("```diff\n--- draft\n+++ recommendation"), "{out}");
    assert!(out.contains("+defer res.Close()"), "aligned to `res`: {out}");
    assert!(out.contains("+\tif err != nil {") || out.contains("+if err != nil {"), "{out}");
    assert!(out.contains("store/users.go:") && out.contains("LoadUser"), "provenance: {out}");
    assert!(out.contains("row→res") || out.contains("→res"), "renames shown: {out}");
    assert!(!out.contains("relevant_imports"), "no import needed here: {out}");
    let full = c.ok("find_idioms", json!({ "snippet": DRAFT_BODY, "lang": "go", "detail": "full", "max_lines": 5 })).await;
    assert!(full.contains("```go\nres, err := db.Query("), "{full}");
    assert!(full.contains("more lines)"), "max_lines clips: {full}");
    let compact = c.ok("find_idioms", json!({ "snippet": DRAFT_BODY, "lang": "go", "detail": "compact" })).await;
    assert!(compact.contains("defer res.Close()") && !compact.contains("--- draft"), "{compact}");
    // Imports needed by the added lines are reported.
    let joins = c.ok("find_idioms", json!({ "snippet": "out := strings.Join(names, \",\")\nreturn out", "lang": "go" })).await;
    println!("find_idioms (imports):\n{joins}");
    assert!(joins.contains("relevant_imports: import \"log\""), "{joins}");
    assert!(!joins.contains("\"os\""), "{joins}");
    assert!(joins.contains("if len(names) == 0"), "{joins}");
    // Nothing similar: short and silent.
    let none = c.ok("find_idioms", json!({ "snippet": "x := 1", "lang": "go" })).await;
    assert!(none.starts_with("No consensus idioms"), "{none}");
    // Semantic errors are tool errors; shape errors are JSON-RPC errors.
    let (t, err) = c.tool("find_idioms", json!({ "snippet": "x", "lang": "cobol" })).await;
    assert!(err && t.contains("unsupported lang"), "{t}");
    let (t, err) = c.tool("find_idioms", json!({ "snippet": "x".repeat(70 * 1024), "lang": "go" })).await;
    assert!(err && t.contains("limit"), "{t}");
    let e = c.rpc(9, "tools/call", json!({ "name": "find_idioms", "arguments": { "lang": "go" } })).await.json();
    assert_eq!(e["error"]["code"], -32602, "{e}");
    let e = c.rpc(9, "tools/call", json!({ "name": "find_idioms", "arguments": { "snippet": "x", "lang": "go", "detail": "huge" } })).await.json();
    assert_eq!(e["error"]["code"], -32602, "{e}");

    // find_similar_code: scope and exclude_path filter before ranking.
    let all = c.ok("find_similar_code", json!({ "snippet": DRAFT_BODY, "lang": "go", "limit": 10 })).await;
    println!("find_similar_code:\n{all}");
    assert!(all.contains("[prod]") && all.contains("[test]"), "{all}");
    assert!(all.contains("store/users.go:"), "{all}");
    let prod = c.ok("find_similar_code", json!({ "snippet": DRAFT_BODY, "lang": "go", "scope": "prod", "limit": 10 })).await;
    assert!(prod.contains("[prod]") && !prod.contains("[test]"), "{prod}");
    let test = c.ok("find_similar_code", json!({ "snippet": DRAFT_BODY, "lang": "go", "scope": "test" })).await;
    assert!(test.contains("[test]") && !test.contains("[prod]") && test.contains("loadRaw"), "{test}");
    let excl = c.ok("find_similar_code", json!({ "snippet": DRAFT_BODY, "lang": "go", "scope": "prod", "exclude_path": "store/users.go", "limit": 10 })).await;
    assert!(!excl.contains("store/users.go") && excl.contains("store/orders.go"), "{excl}");
    let dir_excl = c.ok("find_similar_code", json!({ "snippet": DRAFT_BODY, "lang": "go", "exclude_path": "store/", "limit": 10 })).await;
    assert!(!dir_excl.contains("store/"), "{dir_excl}");
    let one = c.ok("find_similar_code", json!({ "snippet": DRAFT_BODY, "lang": "go", "limit": 1 })).await;
    assert!(one.starts_with("1 similar method"), "{one}");
    let capped = c.ok("find_similar_code", json!({ "snippet": DRAFT_BODY, "lang": "go", "limit": 1000 })).await;
    assert!(capped.matches("\n```go\n").count() <= 20);
    // Junk queries stay silent instead of listing weak matches.
    for junk in ["x := 1", "zz.Qux(7)", "return"] {
        let j = c.ok("find_similar_code", json!({ "snippet": junk, "lang": "go" })).await;
        assert!(j.starts_with("No structurally similar go code"), "{junk}: {j}");
    }

    // check_missing_guards with code + lang.
    let g = c.ok("check_missing_guards", json!({ "code": DRAFT_MISSING, "lang": "go" })).await;
    println!("check_missing_guards:\n{g}");
    assert!(g.contains("high-confidence missing guard"), "{g}");
    assert!(g.contains("error_check: `if err != nil {`"), "{g}");
    assert!(g.contains("cleanup: `defer res.Close()`"), "aligned: {g}");
    assert!(g.contains("seen at: store/"), "{g}");
    assert!(!g.contains("_test.go"), "prod peers only: {g}");
    let clean = c.ok("check_missing_guards", json!({ "code": loader("LoadInvoice", "Invoice", "invoices", "res", "inv"), "lang": "go" })).await;
    assert_eq!(clean, "No high-confidence missing guards detected.");
    // ... and with a raw unified diff.
    let added: String = ["package store", ""].iter().copied().chain(DRAFT_MISSING.lines()).map(|l| format!("+{l}\n")).collect();
    let diff = format!(
        "diff --git a/store/invoices.go b/store/invoices.go\nnew file mode 100644\n--- /dev/null\n+++ b/store/invoices.go\n@@ -0,0 +1,{} @@\n{added}",
        DRAFT_MISSING.lines().count() + 2
    );
    let gd = c.ok("check_missing_guards", json!({ "diff": diff })).await;
    println!("check_missing_guards (diff):\n{gd}");
    assert!(gd.contains("## store/invoices.go:"), "{gd}");
    assert!(gd.contains("defer res.Close()"), "{gd}");
    // Extensionless path: `lang` is the fallback.
    let odd = diff.replace("store/invoices.go", "store/invoices.tmpl");
    assert_eq!(c.ok("check_missing_guards", json!({ "diff": odd })).await, "No high-confidence missing guards detected.");
    assert!(c.ok("check_missing_guards", json!({ "diff": odd, "lang": "go" })).await.contains("defer res.Close()"));
    let (t, err) = c.tool("check_missing_guards", json!({})).await;
    assert!(err && t.contains("`code`") && t.contains("`diff`"), "{t}");
    let (t, err) = c.tool("check_missing_guards", json!({ "code": DRAFT_MISSING })).await;
    assert!(err && t.contains("needs `lang`"), "{t}");
    let (t, err) = c.tool("check_missing_guards", json!({ "diff": "not a diff" })).await;
    assert!(err && t.contains("no unified-diff hunks"), "{t}");

    // find_test_idioms: repo test skeletons renamed to the draft.
    let ti = c.ok("find_test_idioms", json!({ "code": DRAFT_MISSING, "lang": "go" })).await;
    println!("find_test_idioms:\n{ti}");
    assert!(ti.contains("draft `LoadInvoice`"), "{ti}");
    assert!(ti.contains("store/users_test.go:") && ti.contains("TestLoadUser"), "{ti}");
    assert!(ti.contains("via references_peer (peer store/users.go:"), "{ti}");
    assert!(ti.contains("LoadInvoice(db, \"1\")"), "skeleton renamed: {ti}");
    assert!(ti.contains("test imports: import \"testing\""), "{ti}");
    assert!(ti.contains("assertions: `t.Fatalf("), "{ti}");
    let no_tests = c.ok("find_test_idioms", json!({ "code": DRAFT_MISSING, "lang": "go", "exclude_path": ["store/users_test.go", "store/orders_test.go"] })).await;
    assert!(!no_tests.contains("_test.go:"), "{no_tests}");
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refresh_makes_new_code_searchable_and_status_tracks_it() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(vec![go_corpus(dir.path(), "app")], None);
    let srv = TestServer::start(reg, HttpConfig::default()).await;
    let c = Client::new(srv.addr);

    let status = c.ok("corpus_status", json!({})).await;
    println!("corpus_status:\n{status}");
    assert!(status.starts_with("1 corpus loaded"), "{status}");
    assert!(status.contains("`app` (default) · directory · 10 methods (7 prod / 3 test) in 7 files"), "{status}");
    assert!(status.contains("langs java, python, javascript, typescript, tsx, go, rust"), "{status}");
    assert!(!status.contains(dir.path().to_str().unwrap()), "no host paths: {status}");

    let probe = "acc := 0\nfor _, w := range weights {\n\tacc += w * w\n}\nreturn math.Sqrt(float64(acc))";
    let before = c.ok("find_similar_code", json!({ "snippet": probe, "lang": "go" })).await;
    assert!(!before.contains("Norm2"), "{before}");

    write(dir.path(), "mathx/norm.go", "package mathx\n\nimport \"math\"\n\nfunc Norm2(weights []int) float64 {\n\tacc := 0\n\tfor _, w := range weights {\n\t\tacc += w * w\n\t}\n\treturn math.Sqrt(float64(acc))\n}\n");
    let r = c.ok("refresh_corpus", json!({ "corpus": "app" })).await;
    println!("refresh_corpus:\n{r}");
    assert!(r.contains("files_added 1, files_modified 0, files_removed 0, files_unchanged 7 → total_methods 11"), "{r}");
    let after = c.ok("find_similar_code", json!({ "snippet": probe, "lang": "go", "limit": 1 })).await;
    assert!(after.contains("mathx/norm.go:5 Norm2 [prod] score 1.00"), "{after}");

    // Modify and remove; concurrent queries keep working during the refresh.
    write(dir.path(), "mathx/norm.go", "package mathx\n\nfunc Norm1(weights []int) int {\n\tacc := 0\n\tfor _, w := range weights {\n\t\tacc += w\n\t}\n\treturn acc\n}\n");
    std::fs::remove_file(dir.path().join("text/names.go")).unwrap();
    let mut qs = Vec::new();
    for _ in 0..8 {
        let c = c.clone();
        qs.push(tokio::spawn(async move { c.ok("find_similar_code", json!({ "snippet": DRAFT_BODY, "lang": "go" })).await }));
    }
    let r = c.ok("refresh_corpus", json!({})).await;
    for q in qs {
        assert!(q.await.unwrap().contains("similar method"));
    }
    assert!(r.contains("`app`: files_added 0, files_modified 1, files_removed 1, files_unchanged 6 → total_methods 10"), "{r}");
    let after = c.ok("find_similar_code", json!({ "snippet": probe, "lang": "go", "limit": 1 })).await;
    assert!(after.contains("Norm1") && !after.contains("Norm2"), "{after}");
    let status = c.ok("corpus_status", json!({ "corpus": "app" })).await;
    assert!(status.contains("10 methods") && status.contains("refreshed 0s ago (+0 ~1 -1 files"), "{status}");
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_corpus_routing_and_snapshots() {
    let go = tempfile::tempdir().unwrap();
    let py = tempfile::tempdir().unwrap();
    let snap_src = tempfile::tempdir().unwrap();
    py_repo(py.path());
    // A prebuilt .sil snapshot of another Go repo.
    write(snap_src.path(), "lib/sum.go", "package lib\n\nfunc Total(xs []int) int {\n\tt := 0\n\tfor _, x := range xs {\n\t\tt += x\n\t}\n\treturn t\n}\n");
    let (parts, _) = sillage_index::index_dir(snap_src.path(), &[Lang::Go], &sillage_index::AromaFeaturizer).unwrap();
    let sil = snap_src.path().join("lib.sil");
    sillage_index::save(&sil, &parts.corpus, &parts.index, &parts.bm25).unwrap();

    let reg = registry(
        vec![
            go_corpus(go.path(), "backend"),
            CorpusHandle::open_dir("scripts", py.path(), &[Lang::Python]).unwrap(),
            CorpusHandle::open_snapshot("lib", &sil).unwrap(),
        ],
        Some("scripts"),
    );
    let srv = TestServer::start(reg, HttpConfig::default()).await;
    let c = Client::new(srv.addr);

    let status = c.ok("corpus_status", json!({})).await;
    println!("{status}");
    assert!(status.starts_with("3 corpora loaded"), "{status}");
    assert!(status.contains("`scripts` (default) · directory · 3 methods (3 prod / 0 test) in 2 files · langs python"), "{status}");
    assert!(status.contains("`lib` · snapshot · 1 methods (1 prod / 0 test) in 1 file · langs go · static snapshot"), "{status}");
    assert!(!status.contains(".sil") && !status.contains(go.path().to_str().unwrap()), "{status}");

    // Routing: the default corpus is python; `corpus` selects the others.
    let py_q = "with open(path) as fh:\n    data = fh.read()\nreturn data.splitlines()";
    let s = c.ok("find_similar_code", json!({ "snippet": py_q, "lang": "python" })).await;
    assert!(s.contains("corpus `scripts`") && s.contains("pkg/io_a.py:4 read_lines"), "{s}");
    let s = c.ok("find_similar_code", json!({ "snippet": DRAFT_BODY, "lang": "go", "corpus": "backend" })).await;
    assert!(s.contains("corpus `backend`") && s.contains("store/"), "{s}");
    let s = c.ok("find_similar_code", json!({ "snippet": "t := 0\nfor _, x := range xs {\n\tt += x\n}", "lang": "go", "corpus": "lib" })).await;
    assert!(s.contains("lib/sum.go:3 Total"), "{s}");
    let idioms = c.ok("find_idioms", json!({ "snippet": DRAFT_BODY, "lang": "go", "corpus": "backend", "preset": "paper" })).await;
    assert!(idioms.contains("preset paper") || idioms.starts_with("No consensus"), "{idioms}");

    let (t, err) = c.tool("find_similar_code", json!({ "snippet": "x", "lang": "go", "corpus": "nope" })).await;
    assert!(err && t.contains("unknown corpus \"nope\"") && t.contains("backend, scripts, lib"), "{t}");
    let (t, err) = c.tool("corpus_status", json!({ "corpus": "nope" })).await;
    assert!(err && t.contains("unknown corpus"), "{t}");

    // Snapshots cannot be refreshed; refreshing all skips them.
    let (t, err) = c.tool("refresh_corpus", json!({ "corpus": "lib" })).await;
    assert!(err && t.contains("static .sil snapshot"), "{t}");
    let all = c.ok("refresh_corpus", json!({})).await;
    assert!(all.contains("`backend`:") && all.contains("`scripts`:") && all.contains("skipped static snapshots: lib"), "{all}");
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_refresh_picks_up_changes() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(vec![go_corpus(dir.path(), "app")], None);
    let shutdown = sillage_mcp::Shutdown::new();
    let task = reg.spawn_refresh_task(Duration::from_millis(100), shutdown.clone());
    write(dir.path(), "extra/x.go", "package extra\n\nfunc Extra(a int) int {\n\treturn a + 1\n}\n");
    let h = reg.get(Some("app")).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while h.meta().methods != 11 {
        assert!(std::time::Instant::now() < deadline, "background refresh did not run");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    shutdown.trigger();
    tokio::time::timeout(Duration::from_secs(5), task).await.expect("refresh task stops on shutdown").unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_header_is_checked_on_loopback() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(vec![go_corpus(dir.path(), "app")], None);
    let srv = TestServer::start(reg, HttpConfig { allowed_hosts: vec!["mcp.internal".into()], ..HttpConfig::default() }).await;
    let ping = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let post = |host: &'static str| async move {
        let mut s = tokio::net::TcpStream::connect(srv.addr).await.unwrap();
        let req = format!(
            "POST /mcp HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            ping.len()
        );
        s.write_all(req.as_bytes()).await.unwrap();
        s.write_all(ping).await.unwrap();
        let mut out = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await.unwrap().unwrap();
        String::from_utf8_lossy(&out).split(' ').nth(1).unwrap_or("").to_string()
    };
    assert_eq!(post("evil.example").await, "403", "DNS-rebinding Host refused");
    assert_eq!(post("evil.example:8765").await, "403");
    assert_eq!(post("localhost:8765").await, "200");
    assert_eq!(post("[::1]:8765").await, "200");
    assert_eq!(post("mcp.internal").await, "200", "--allow-host");
    // /healthz stays open for probes.
    let mut s = tokio::net::TcpStream::connect(srv.addr).await.unwrap();
    s.write_all(b"GET /healthz HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    assert!(String::from_utf8_lossy(&out).starts_with("HTTP/1.1 200"));
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stalled_clients_time_out_and_shutdown_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(vec![go_corpus(dir.path(), "app")], None);
    let cfg = HttpConfig {
        header_read_timeout: Duration::from_millis(300),
        request_timeout: Duration::from_millis(300),
        shutdown_timeout: Duration::from_millis(300),
        ..HttpConfig::default()
    };
    let srv = TestServer::start(reg, cfg).await;
    let read_all = |mut s: tokio::net::TcpStream| async move {
        let mut out = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await.expect("server closes the connection").ok();
        String::from_utf8_lossy(&out).to_string()
    };

    // A body that never arrives: 408 once the body timeout passes.
    let t0 = std::time::Instant::now();
    let mut s = tokio::net::TcpStream::connect(srv.addr).await.unwrap();
    s.write_all(b"POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{\"jsonrpc\"").await.unwrap();
    let out = read_all(s).await;
    assert!(out.starts_with("HTTP/1.1 408"), "{out}");
    assert!(t0.elapsed() < Duration::from_secs(3));

    // Headers that never finish, and an idle connection: closed by the server.
    let mut s = tokio::net::TcpStream::connect(srv.addr).await.unwrap();
    s.write_all(b"POST /mcp HTTP/1.1\r\nHost: local").await.unwrap();
    read_all(s).await;
    let idle = tokio::net::TcpStream::connect(srv.addr).await.unwrap();
    read_all(idle).await;
    assert!(t0.elapsed() < Duration::from_secs(5));

    // Health stays reachable meanwhile.
    assert_eq!(http(srv.addr, "GET", "/healthz", &[], None).await.status, 200);

    // A client stalled mid-body at shutdown cannot hold the server open.
    let mut stalled = tokio::net::TcpStream::connect(srv.addr).await.unwrap();
    stalled.write_all(b"POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{").await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let t1 = std::time::Instant::now();
    srv.stop().await;
    assert!(t1.elapsed() < Duration::from_secs(2), "shutdown took {:?}", t1.elapsed());
    drop(stalled);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connection_cap_sheds_excess_connections() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(vec![go_corpus(dir.path(), "app")], None);
    let srv = TestServer::start(reg, HttpConfig { max_connections: 2, header_read_timeout: Duration::from_secs(30), ..HttpConfig::default() }).await;
    let _a = tokio::net::TcpStream::connect(srv.addr).await.unwrap();
    let _b = tokio::net::TcpStream::connect(srv.addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    // The third is closed at once instead of queueing behind the idle two.
    let mut c = tokio::net::TcpStream::connect(srv.addr).await.unwrap();
    let _ = c.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    let mut out = Vec::new();
    let n = tokio::time::timeout(Duration::from_secs(5), c.read_to_end(&mut out)).await.expect("shed promptly");
    assert!(n.is_err() || out.is_empty(), "{:?}", String::from_utf8_lossy(&out));
    drop((_a, _b));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(http(srv.addr, "GET", "/healthz", &[], None).await.status, 200, "slots free again");
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tool_calls_time_out_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(vec![go_corpus(dir.path(), "app")], None);
    let limits = sillage_mcp::ToolLimits::new(1, Duration::ZERO);
    let srv = TestServer::start_with(reg, HttpConfig::default(), limits).await;
    let c = Client::new(srv.addr);
    let (t, err) = c.tool("find_similar_code", json!({ "snippet": DRAFT_BODY, "lang": "go" })).await;
    assert!(err && t.contains("timed out"), "{t}");
    // Non-tool requests are unaffected.
    assert_eq!(c.rpc(2, "ping", json!({})).await.status, 200);
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sse_keep_alives_refresh_the_session_and_streams_are_capped() {
    let dir = tempfile::tempdir().unwrap();
    let reg = registry(vec![go_corpus(dir.path(), "app")], None);
    let cfg = HttpConfig {
        session_ttl: Duration::from_millis(400),
        keep_alive: Duration::from_millis(50),
        max_stream_lifetime: Duration::from_millis(900),
        ..HttpConfig::default()
    };
    let srv = TestServer::start(reg, cfg).await;
    let mut c = Client::new(srv.addr);
    c.initialize().await;
    let sid = c.session.clone().unwrap();
    let t0 = std::time::Instant::now();
    let mut sse = tokio::net::TcpStream::connect(srv.addr).await.unwrap();
    sse.write_all(format!("GET /mcp HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAccept: text/event-stream\r\nMcp-Session-Id: {sid}\r\n\r\n").as_bytes()).await.unwrap();
    // Past the idle TTL: only the stream's keep-alives kept the session alive.
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(c.rpc(2, "ping", json!({})).await.status, 200, "session refreshed by keep-alives");
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), sse.read_to_end(&mut out)).await.expect("stream lifetime capped").unwrap();
    assert!(t0.elapsed() >= Duration::from_millis(850), "{:?}", t0.elapsed());
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("sillage stream open") && text.contains("keep-alive"), "{text}");
    srv.stop().await;
}
