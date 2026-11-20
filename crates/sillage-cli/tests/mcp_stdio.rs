//! End-to-end test of `sillage mcp` (stdio transport): the compiled binary,
//! a real repository on disk, newline-delimited JSON-RPC on stdin/stdout.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn loader(func: &str, typ: &str, table: &str, rows: &str, v: &str) -> String {
    format!(
        "func {func}(db *DB, id string) (*{typ}, error) {{\n\t{rows}, err := db.Query(\"SELECT id, name FROM {table} WHERE id = ?\", id)\n\tif err != nil {{\n\t\treturn nil, err\n\t}}\n\tdefer {rows}.Close()\n\t{v} := &{typ}{{}}\n\tif err := {rows}.Scan(&{v}.ID, &{v}.Name); err != nil {{\n\t\treturn nil, err\n\t}}\n\treturn {v}, nil\n}}\n"
    )
}

#[test]
fn stdio_mcp_session_emits_only_json_rpc_on_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("svc");
    write(&root, "store/users.go", &format!("package store\n\n{}\n{}", loader("LoadUser", "User", "users", "row", "u"), loader("LoadAdmin", "Admin", "admins", "r", "a")));
    write(&root, "store/orders.go", &format!("package store\n\n{}", loader("LoadOrder", "Order", "orders", "rs", "o")));
    write(&root, "store/items.go", &format!("package store\n\n{}", loader("LoadItem", "Item", "items", "q", "it")));

    let draft = "res, err := db.Query(\"SELECT id, name FROM invoices WHERE id = ?\", id)\ninv := &Invoice{}\nif err := res.Scan(&inv.ID, &inv.Name); err != nil {\n\treturn nil, err\n}\nreturn inv, nil";
    let msgs = [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } } }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "find_idioms", "arguments": { "snippet": draft, "lang": "go" } } }),
        json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "corpus_status", "arguments": {} } }),
        json!({ "jsonrpc": "2.0", "id": 5, "method": "ping" }),
    ];
    let mut input = String::new();
    for m in &msgs {
        input.push_str(&m.to_string());
        input.push('\n');
    }
    input.push_str("{this is not json\n\n");
    // A line over the 4 MiB limit is skipped, not buffered.
    input.push_str(&"x".repeat(5 * 1024 * 1024));
    input.push('\n');

    let mut child = Command::new(env!("CARGO_BIN_EXE_sillage"))
        .args(["mcp", "--root"])
        .arg(&root)
        .current_dir(dir.path())
        .env_remove("SILLAGE_MCP_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        stdin.write_all(input.as_bytes()).unwrap();
        // Dropping stdin sends EOF: the server finishes in-flight requests and exits.
    });
    let out = child.wait_with_output().unwrap();
    writer.join().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "exit {:?}\nstderr:\n{stderr}", out.status);
    assert!(stderr.contains("corpus svc [default] (directory): 4 methods"), "{stderr}");

    let replies: Vec<Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("non-JSON stdout line ({e}): {l:?}")))
        .collect();
    assert!(stdout.ends_with('\n'));
    assert_eq!(replies.len(), 7, "{stdout}");
    let by_id = |id: i64| replies.iter().find(|r| r["id"] == id).unwrap_or_else(|| panic!("no reply {id}: {stdout}"));

    let init = &by_id(1)["result"];
    assert_eq!(init["protocolVersion"], "2025-06-18");
    assert_eq!(init["serverInfo"]["name"], "sillage");
    assert!(init["instructions"].as_str().unwrap().contains("check_missing_guards"));

    let tools = by_id(2)["result"]["tools"].as_array().unwrap().len();
    assert_eq!(tools, 6);

    let idioms = &by_id(3)["result"];
    assert_eq!(idioms["isError"], false);
    let text = idioms["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("confidence high") && text.contains("+defer res.Close()"), "{text}");
    assert!(text.contains("store/users.go:3 LoadUser"), "{text}");

    let status = by_id(4)["result"]["content"][0]["text"].as_str().unwrap();
    assert!(status.contains("`svc` (default) · directory · 4 methods (4 prod / 0 test) in 3 files"), "{status}");
    assert!(!status.contains(dir.path().to_str().unwrap()), "{status}");

    assert_eq!(by_id(5)["result"], json!({}));
    let errors: Vec<i64> =
        replies.iter().filter(|r| r["id"].is_null()).map(|r| r["error"]["code"].as_i64().unwrap()).collect();
    assert_eq!(errors.len(), 2, "{stdout}");
    assert!(errors.contains(&-32700) && errors.contains(&-32600), "{errors:?}");
}
