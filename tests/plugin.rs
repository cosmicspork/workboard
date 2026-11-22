//! Plugin mode as a Tracon node drives it: the binary started with only the
//! contract's environment, MCP at `POST /`, and the API behind the signed
//! caller context.

mod support;

use std::sync::Arc;

use serde_json::{Value, json};
use support::{Plugin, http, key, unsigned};
use workboard::contract::{self, sign};

#[tokio::test]
async fn mcp_answers_initialize_and_lists_the_operation_table() {
    let dir = tempfile::tempdir().unwrap();
    let mut plugin = Plugin::start(dir.path(), None).await;
    let (status, init) = plugin
        .rpc("initialize", json!({ "protocolVersion": "2025-03-26" }))
        .await;
    assert_eq!(status, 200);
    assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(init["result"]["capabilities"], json!({ "tools": {} }));
    assert_eq!(
        init["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    let (status, _) = plugin
        .post_json(
            "/",
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )
        .await
        .unwrap();
    assert_eq!(status, 202);
    let (status, ping) = plugin.rpc("ping", json!({})).await;
    assert_eq!((status, &ping["result"]), (200, &json!({})));
    let (status, tools) = plugin.rpc("tools/list", json!({})).await;
    assert_eq!(status, 200);
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    let ops: Vec<&str> = workboard::ops::OPS.iter().map(|op| op.name).collect();
    assert_eq!(names, ops);
    let (_, call) = plugin.rpc("tools/call", json!({ "name": "x" })).await;
    assert!(call["error"].is_object());
    // The node serves the SPA; this process does not.
    let (status, _) = http(&plugin.socket, "GET", "/", &[], b"").await.unwrap();
    assert_eq!(status, 405);
    let (status, _) = http(&plugin.socket, "GET", "/index.html", &[], b"")
        .await
        .unwrap();
    assert_eq!(status, 404);
    // The database sits in the data directory under the standalone file name.
    assert!(plugin.data.join("workboard.sqlite3").is_file());
}

#[tokio::test]
async fn a_signed_operator_request_is_served() {
    let dir = tempfile::tempdir().unwrap();
    let plugin = Plugin::start(dir.path(), None).await;
    assert_eq!(plugin.task_count().await, 0);
    let (status, task) = plugin
        .api("POST", "/api/tasks", r#"{"title":"From Tracon"}"#)
        .await;
    assert_eq!(status, 201, "{task}");
    assert_eq!(task["title"], "From Tracon");
    let id = task["id"].as_str().unwrap();
    let (status, read) = plugin.api("GET", &format!("/api/tasks/{id}"), "").await;
    assert_eq!((status, &read["id"]), (200, &task["id"]));
    let (status, missing) = plugin.api("GET", "/api/nothing", "").await;
    assert_eq!((status, &missing["error"]), (404, &json!("not_found")));
    assert_eq!(plugin.task_count().await, 1);
}

#[tokio::test]
async fn refused_requests_are_401_and_change_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let plugin = Plugin::start(dir.path(), None).await;
    let body = r#"{"title":"Refused"}"#;
    let signed = |ctx: Value| sign(&ctx, &key());
    let good = || unsigned("POST", "/api/tasks", body.as_bytes());

    let mut cases: Vec<(&str, &str, &str, Value)> = Vec::new();
    cases.push(("no context", "/api/tasks", body, Value::Null));
    let mut bad_signature = signed(good());
    bad_signature["channel"] = json!("personal");
    cases.push(("bad signature", "/api/tasks", body, bad_signature));
    let other_key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    cases.push(("other key", "/api/tasks", body, sign(&good(), &other_key)));
    let mut instance = good();
    instance["instance"] = json!(contract::instance_of("another-token"));
    cases.push(("wrong instance", "/api/tasks", body, signed(instance)));
    let mut plugin_name = good();
    plugin_name["plugin"] = json!("jira-review");
    cases.push(("wrong plugin", "/api/tasks", body, signed(plugin_name)));
    let mut tool = good();
    tool["tool"] = json!("search");
    cases.push(("wrong tool", "/api/tasks", body, signed(tool)));
    cases.push((
        "body not the one signed",
        "/api/tasks",
        r#"{"title":"Swapped"}"#,
        signed(good()),
    ));
    cases.push((
        "path not the one signed",
        "/api/tasks?x=1",
        body,
        signed(good()),
    ));
    let put = unsigned("PUT", "/api/tasks", body.as_bytes());
    cases.push(("method not the one signed", "/api/tasks", body, signed(put)));
    let mut agent = good();
    agent["caller"] = json!({ "kind": "agent", "session": "s-1", "harness": "claude" });
    cases.push(("agent caller", "/api/tasks", body, signed(agent)));
    let mut expired = good();
    let past = support::now_ms() - 120_000;
    expired["issued_ms"] = json!(past);
    expired["expires_ms"] = json!(past + 30_000);
    cases.push(("expired", "/api/tasks", body, signed(expired)));
    let mut long = good();
    long["expires_ms"] = json!(long["issued_ms"].as_i64().unwrap() + 120_000);
    cases.push(("window too long", "/api/tasks", body, signed(long)));

    for (name, target, sent, context) in cases {
        let (status, answer) = plugin.api_with("POST", target, sent, &context).await;
        assert_eq!(status, 401, "{name}: {answer}");
        assert_eq!(answer["error"], "unauthorized", "{name}");
    }
    assert_eq!(plugin.task_count().await, 0);

    // A replayed context is refused even though it was good the first time.
    let once = signed(good());
    let (status, _) = plugin.api_with("POST", "/api/tasks", body, &once).await;
    assert_eq!(status, 201);
    let (status, answer) = plugin.api_with("POST", "/api/tasks", body, &once).await;
    assert_eq!(status, 401, "{answer}");
    assert_eq!(plugin.task_count().await, 1);

    // Reads are gated too.
    let (status, _) = plugin.api_with("GET", "/api/board", "", &Value::Null).await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn refresh_without_a_jira_site_names_plugin_toml() {
    let dir = tempfile::tempdir().unwrap();
    let plugin = Plugin::start(dir.path(), None).await;
    let (status, answer) = plugin.api("POST", "/api/jira/refresh", "{}").await;
    assert_eq!(status, 409, "{answer}");
    assert_eq!(answer["error"], "jira_site_missing");
    let toml = plugin.data.join("plugin.toml");
    assert!(
        answer["message"]
            .as_str()
            .unwrap()
            .contains(&toml.display().to_string()),
        "{answer}"
    );
    let (status, board) = plugin.api("GET", "/api/board", "").await;
    assert_eq!(status, 200);
    assert_eq!(board["sync"]["error_code"], Value::Null);
}

#[tokio::test]
async fn refresh_calls_jira_review_through_the_node_socket() {
    let dir = tempfile::tempdir().unwrap();
    let node_socket = dir.path().join("node.sock");
    let seen = support::fake_node(&node_socket, support::two_pages()).await;
    let config = "jira_site = \"https://example.atlassian.net\"\n";
    let plugin = Plugin::start_with_node(dir.path(), Some(config), &node_socket).await;
    let (status, sync) = plugin.api("POST", "/api/jira/refresh", "{}").await;
    assert_eq!(status, 200, "{sync}");
    assert_eq!(sync["issue_count"], 2);
    let requests = seen.requests();
    assert_eq!(requests.len(), 2);
    for (bearer, request) in &requests {
        // The token the node started the plugin with.
        assert_eq!(bearer.as_deref(), Some(support::TOKEN));
        assert_eq!(request["method"], "tools/call");
        assert_eq!(request["params"]["name"], "jira-review__search");
        // No profile configured: the node lends the bare `jira` binding.
        assert!(request["params"]["arguments"].get("profile").is_none());
    }
    let (_, board) = plugin.api("GET", "/api/board", "").await;
    assert_eq!(
        board["tasks"][0]["jira"]["url"],
        "https://example.atlassian.net/browse/ABC-2"
    );
}

#[tokio::test]
async fn refresh_passes_the_configured_profile() {
    let dir = tempfile::tempdir().unwrap();
    let node_socket = dir.path().join("node.sock");
    let seen = support::fake_node(&node_socket, support::two_pages()).await;
    let config = "jira_site = \"https://example.atlassian.net\"\njira_profile = \"client-x\"\n";
    let plugin = Plugin::start_with_node(dir.path(), Some(config), &node_socket).await;
    let (status, sync) = plugin.api("POST", "/api/jira/refresh", "{}").await;
    assert_eq!(status, 200, "{sync}");
    for (_, request) in seen.requests() {
        assert_eq!(request["params"]["arguments"]["profile"], "client-x");
    }
}

#[tokio::test]
async fn a_refused_refresh_keeps_the_board_and_records_the_code() {
    let dir = tempfile::tempdir().unwrap();
    let node_socket = dir.path().join("node.sock");
    support::fake_node(
        &node_socket,
        Arc::new(|request: &Value| {
            (
                200,
                json!({ "jsonrpc": "2.0", "id": request["id"],
                        "error": { "code": -32003, "data": { "decision": "asked" },
                                   "message": "jira-review__search would be put to the operator" } }),
            )
        }),
    )
    .await;
    let config = "jira_site = \"https://example.atlassian.net\"\n";
    let plugin = Plugin::start_with_node(dir.path(), Some(config), &node_socket).await;
    let (status, answer) = plugin.api("POST", "/api/jira/refresh", "{}").await;
    assert_eq!(status, 502, "{answer}");
    assert_eq!(answer["error"], "jira_refused");
    let (_, board) = plugin.api("GET", "/api/board", "").await;
    assert_eq!(board["sync"]["error_code"], "jira_refused");
}

#[tokio::test]
async fn without_a_node_socket_refresh_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let config = "jira_site = \"https://example.atlassian.net\"\n";
    let plugin = Plugin::start(dir.path(), Some(config)).await;
    let (status, answer) = plugin.api("POST", "/api/jira/refresh", "{}").await;
    assert_eq!(status, 502, "{answer}");
    assert_eq!(answer["error"], "tracon_unsupported");
}

#[tokio::test]
async fn start_is_refused_without_the_contract_environment() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("s.sock");
    let out = tokio::process::Command::new(support::BIN)
        .env_clear()
        .env("TRACON_NODE_KEY", support::node_key())
        .args(["serve", "--socket"])
        .arg(&socket)
        .arg("--data")
        .arg(dir.path().join("d"))
        .output()
        .await
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("TRACON_PLUGIN_TOKEN"));
    assert!(!socket.exists());

    let mut cmd = support::command(
        std::path::Path::new(support::BIN),
        &socket,
        &dir.path().join("d"),
    );
    std::fs::create_dir_all(dir.path().join("d")).unwrap();
    std::fs::write(
        dir.path().join("d/plugin.toml"),
        "jira_site = \"http://insecure\"\n",
    )
    .unwrap();
    let out = cmd.output().await.unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("plugin.toml"));
}

#[tokio::test]
async fn a_non_socket_is_not_replaced_and_sigterm_removes_the_socket() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("file");
    std::fs::write(&blocker, "keep").unwrap();
    let out = support::command(
        std::path::Path::new(support::BIN),
        &blocker,
        &dir.path().join("d"),
    )
    .output()
    .await
    .unwrap();
    assert!(!out.status.success());
    assert_eq!(std::fs::read_to_string(&blocker).unwrap(), "keep");

    let mut plugin = Plugin::start(dir.path(), None).await;
    let pid = plugin.child.id().unwrap();
    let killed = std::process::Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let status = tokio::time::timeout(std::time::Duration::from_secs(10), plugin.child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    assert!(!plugin.socket.exists());
}
