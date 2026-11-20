//! Shared fixtures for the MCP integration tests: small real repositories on
//! disk, a server on an ephemeral port, and a minimal raw HTTP/1.1 client.

#![allow(dead_code)]

use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use sillage_mcp::{CorpusHandle, CorpusRegistry, Dispatcher, HttpConfig, Shutdown};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

/// A Go "load one row" function with the repo's two guards: the query error
/// check and the deferred close.
pub fn loader(func: &str, typ: &str, table: &str, rows: &str, v: &str) -> String {
    format!(
        "func {func}(db *DB, id string) (*{typ}, error) {{
\t{rows}, err := db.Query(\"SELECT id, name FROM {table} WHERE id = ?\", id)
\tif err != nil {{
\t\treturn nil, err
\t}}
\tdefer {rows}.Close()
\t{v} := &{typ}{{}}
\tif err := {rows}.Scan(&{v}.ID, &{v}.Name); err != nil {{
\t\treturn nil, err
\t}}
\treturn {v}, nil
}}
"
    )
}

/// A finished loader missing both guards.
pub const DRAFT_MISSING: &str = "func LoadInvoice(db *DB, id string) (*Invoice, error) {
\tres, err := db.Query(\"SELECT id, name FROM invoices WHERE id = ?\", id)
\tinv := &Invoice{}
\tif err := res.Scan(&inv.ID, &inv.Name); err != nil {
\t\treturn nil, err
\t}
\treturn inv, nil
}
";

/// The agent's half-written body: query and scan, no error check, no close.
pub const DRAFT_BODY: &str = "res, err := db.Query(\"SELECT id, name FROM invoices WHERE id = ?\", id)\ninv := &Invoice{}\nif err := res.Scan(&inv.ID, &inv.Name); err != nil {\n\treturn nil, err\n}\nreturn inv, nil";

fn joiner(name: &str, v: &str) -> String {
    format!(
        "func {name}({v} []string) string {{\n\tif len({v}) == 0 {{\n\t\treturn \"\"\n\t}}\n\tout := strings.Join({v}, \",\")\n\tlog.Printf(\"joined %d\", len({v}))\n\treturn out\n}}\n"
    )
}

/// A multi-package Go service: three store files sharing the loader idiom,
/// a text package whose join idiom needs `log`, and tests.
pub fn go_repo(root: &Path) {
    write(
        root,
        "store/users.go",
        &format!(
            "package store\n\nimport (\n\t\"database/sql\"\n\t\"fmt\"\n)\n\n{}\n{}",
            loader("LoadUser", "User", "users", "row", "u"),
            loader("LoadAdmin", "Admin", "admins", "r", "a")
        ),
    );
    write(root, "store/orders.go", &format!("package store\n\nimport \"strings\"\n\n{}", loader("LoadOrder", "Order", "orders", "rs", "o")));
    write(root, "store/items.go", &format!("package store\n\n{}", loader("LoadItem", "Item", "items", "q", "it")));
    write(
        root,
        "store/users_test.go",
        "package store\n\nimport \"testing\"\n\nfunc TestLoadUser(t *testing.T) {\n\tdb := openTestDB(t)\n\tu, err := LoadUser(db, \"1\")\n\tif err != nil {\n\t\tt.Fatalf(\"load: %v\", err)\n\t}\n\tif u.Name != \"ann\" {\n\t\tt.Errorf(\"name = %q\", u.Name)\n\t}\n}\n\nfunc loadRaw(db *DB, id string) (*User, error) {\n\trow, _ := db.Query(\"SELECT id, name FROM users WHERE id = ?\", id)\n\tu := &User{}\n\trow.Scan(&u.ID, &u.Name)\n\treturn u, nil\n}\n",
    );
    write(
        root,
        "store/orders_test.go",
        "package store\n\nimport (\n\t\"testing\"\n\t\"strings\"\n)\n\nfunc TestLoadOrder(t *testing.T) {\n\tdb := openTestDB(t)\n\to, err := LoadOrder(db, \"7\")\n\tif err != nil {\n\t\tt.Fatalf(\"load: %v\", err)\n\t}\n\tif o.ID != \"7\" {\n\t\tt.Errorf(\"id = %q\", o.ID)\n\t}\n}\n",
    );
    write(
        root,
        "text/join.go",
        &format!("package text\n\nimport (\n\t\"log\"\n\t\"strings\"\n\t\"os\"\n)\n\n{}\n{}", joiner("JoinA", "xs"), joiner("JoinB", "ys")),
    );
    write(root, "text/names.go", &format!("package text\n\nimport (\n\t\"log\"\n\t\"strings\"\n)\n\n{}", joiner("JoinC", "zs")));
}

/// A small Python package.
pub fn py_repo(root: &Path) {
    let f = |name: &str, v: &str| {
        format!(
            "def {name}(path):\n    with open(path) as fh:\n        {v} = fh.read()\n    if not {v}:\n        raise ValueError(\"empty\")\n    return {v}.splitlines()\n\n"
        )
    };
    write(root, "pkg/io_a.py", &format!("import os\n\n\n{}{}", f("read_lines", "data"), f("load_rows", "text")));
    write(root, "pkg/io_b.py", &format!("import sys\n\n\n{}", f("slurp", "content")));
}

pub fn registry(handles: Vec<CorpusHandle>, default: Option<&str>) -> Arc<CorpusRegistry> {
    Arc::new(CorpusRegistry::from_handles(handles, default).unwrap())
}

pub struct TestServer {
    pub addr: SocketAddr,
    pub registry: Arc<CorpusRegistry>,
    shutdown: Shutdown,
    handle: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl TestServer {
    pub async fn start(registry: Arc<CorpusRegistry>, cfg: HttpConfig) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let shutdown = Shutdown::new();
        let d = Arc::new(Dispatcher::new(Arc::clone(&registry)));
        let handle = tokio::spawn(sillage_mcp::http::serve(listener, d, cfg, shutdown.clone()));
        Self { addr, registry, shutdown, handle }
    }

    /// Graceful shutdown; fails the test if it hangs.
    pub async fn stop(self) {
        self.shutdown.trigger();
        tokio::time::timeout(Duration::from_secs(10), self.handle).await.expect("graceful shutdown").unwrap().unwrap();
    }
}

#[derive(Debug)]
pub struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Resp {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", self.body))
    }
}

fn parse_head(raw: &[u8]) -> Option<(u16, Vec<(String, String)>, usize)> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..end]).ok()?;
    let mut lines = head.split("\r\n");
    let status = lines.next()?.split(' ').nth(1)?.parse().ok()?;
    let headers = lines
        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect();
    Some((status, headers, end + 4))
}

fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(i) = b.windows(2).position(|w| w == b"\r\n") {
        let size = usize::from_str_radix(std::str::from_utf8(&b[..i]).unwrap().split(';').next().unwrap().trim(), 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        let start = i + 2;
        out.extend_from_slice(&b[start..(start + size).min(b.len())]);
        b = &b[(start + size + 2).min(b.len())..];
    }
    out
}

/// One HTTP/1.1 request on a fresh connection (`Connection: close`).
pub async fn http(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)], body: Option<&[u8]>) -> Resp {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    let chunked = headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v.contains("chunked"));
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if let (Some(b), false) = (body, chunked) {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    let mut bytes = req.into_bytes();
    if let Some(b) = body {
        if chunked {
            bytes.extend_from_slice(format!("{:x}\r\n", b.len()).as_bytes());
            bytes.extend_from_slice(b);
            bytes.extend_from_slice(b"\r\n0\r\n\r\n");
        } else {
            bytes.extend_from_slice(b);
        }
    }
    let (mut rd, mut wr) = s.split();
    // The server may answer (413) before reading the whole body: ignore write
    // errors and keep whatever response bytes arrived before a reset.
    let write = async {
        let _ = wr.write_all(&bytes).await;
    };
    let read = async {
        let mut raw = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match rd.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => raw.extend_from_slice(&buf[..n]),
                Err(_) if !raw.is_empty() => break,
                Err(e) => panic!("read failed: {e}"),
            }
        }
        raw
    };
    let (_, raw) = tokio::time::timeout(Duration::from_secs(60), async { tokio::join!(write, read) }).await.expect("response");
    let (status, headers, start) = parse_head(&raw).unwrap_or_else(|| panic!("bad response: {:?}", String::from_utf8_lossy(&raw)));
    let is_chunked = headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v.contains("chunked"));
    let body = if is_chunked { dechunk(&raw[start..]) } else { raw[start..].to_vec() };
    Resp { status, headers, body: String::from_utf8_lossy(&body).into_owned() }
}

/// A JSON-RPC client for one server.
#[derive(Clone)]
pub struct Client {
    pub addr: SocketAddr,
    pub token: Option<String>,
    pub session: Option<String>,
}

impl Client {
    pub fn new(addr: SocketAddr) -> Self {
        Self { addr, token: None, session: None }
    }

    pub fn with_token(mut self, t: &str) -> Self {
        self.token = Some(t.to_string());
        self
    }

    fn headers(&self) -> Vec<(String, String)> {
        let mut h = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "application/json, text/event-stream".to_string()),
        ];
        if let Some(t) = &self.token {
            h.push(("Authorization".into(), format!("Bearer {t}")));
        }
        if let Some(s) = &self.session {
            h.push(("Mcp-Session-Id".into(), s.clone()));
        }
        h
    }

    pub async fn post(&self, msg: &Value) -> Resp {
        let h = self.headers();
        let hs: Vec<(&str, &str)> = h.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        http(self.addr, "POST", "/mcp", &hs, Some(msg.to_string().as_bytes())).await
    }

    pub async fn rpc(&self, id: u64, method: &str, params: Value) -> Resp {
        self.post(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).await
    }

    /// `tools/call`, asserting a 200 result; returns `(text, isError)`.
    pub async fn tool(&self, name: &str, args: Value) -> (String, bool) {
        let r = self.rpc(7, "tools/call", json!({ "name": name, "arguments": args })).await;
        assert_eq!(r.status, 200, "{r:?}");
        let v = r.json();
        let res = &v["result"];
        assert!(res.is_object(), "expected a result: {v}");
        assert_eq!(res["content"][0]["type"], "text");
        (res["content"][0]["text"].as_str().unwrap().to_string(), res["isError"].as_bool().unwrap())
    }

    /// `tools/call` that must succeed.
    pub async fn ok(&self, name: &str, args: Value) -> String {
        let (text, err) = self.tool(name, args).await;
        assert!(!err, "{name} failed: {text}");
        text
    }

    pub async fn initialize(&mut self) -> Resp {
        let r = self
            .rpc(1, "initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } }))
            .await;
        assert_eq!(r.status, 200, "{r:?}");
        self.session = r.header("mcp-session-id").map(str::to_string);
        r
    }
}
