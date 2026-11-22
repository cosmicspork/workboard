//! Starting `workboard serve --socket --data` as a Tracon node would, and
//! speaking to it over its socket.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine;
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use workboard::contract;

pub const BIN: &str = env!("CARGO_BIN_EXE_workboard");
pub const TOKEN: &str = "plugin-test-token";

pub fn key() -> SigningKey {
    SigningKey::from_bytes(&std::array::from_fn(|i| i as u8 + 1))
}

pub fn node_key() -> String {
    hex::encode(key().verifying_key().to_bytes())
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// `exec` with the arguments and environment a node starts a plugin with.
pub fn command(exec: &Path, socket: &Path, data: &Path) -> Command {
    let mut cmd = Command::new(exec);
    cmd.env_clear()
        .env("TRACON_PLUGIN_TOKEN", TOKEN)
        .env("TRACON_NODE_KEY", node_key())
        .env("TRACON_CONTRACT", "1")
        .arg("serve")
        .arg("--socket")
        .arg(socket)
        .arg("--data")
        .arg(data)
        .kill_on_drop(true);
    cmd
}

/// One HTTP/1.1 exchange on the socket: the status and the body as JSON
/// (null when empty or not JSON).
pub async fn http(
    socket: &Path,
    method: &str,
    target: &str,
    headers: &[(&str, String)],
    body: &[u8],
) -> std::io::Result<(u16, Value)> {
    let mut stream = UnixStream::connect(socket).await?;
    let mut request = format!(
        "{method} {target} HTTP/1.1\r\nHost: plugin\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await?;
    stream.write_all(body).await?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await?;
    let raw = String::from_utf8_lossy(&raw);
    let status = raw[9..12].parse().unwrap();
    let body = raw.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    Ok((status, serde_json::from_str(body).unwrap_or(Value::Null)))
}

/// The context a node would sign for `method target` with `body`, before
/// signing; callers may change it to make it wrong.
pub fn unsigned(method: &str, target: &str, body: &[u8]) -> Value {
    let now = now_ms();
    json!({
        "v": 1, "plugin": "workboard", "instance": contract::instance_of(TOKEN),
        "tool": "tracon/ui",
        "arguments_sha256": contract::ui_arguments_sha256(method, target, body),
        "caller": { "kind": "operator", "session": null, "harness": null },
        "channel": "work", "approval_id": null,
        "issued_ms": now, "expires_ms": now + 30_000,
        "nonce": uuid::Uuid::new_v4().simple().to_string(),
        "credentials": {},
    })
}

/// The context a node would sign onto a `tools/call` or `tracon/preview` of
/// `tool` with `arguments` from an agent, before signing.
pub fn unsigned_call(
    tool: &str,
    arguments: &Value,
    approval_id: Option<&str>,
    preview_token: Option<&str>,
) -> Value {
    let now = now_ms();
    let mut context = json!({
        "v": 1, "plugin": "workboard", "instance": contract::instance_of(TOKEN),
        "tool": tool,
        "arguments_sha256": contract::arguments_sha256(Some(arguments)),
        "caller": { "kind": "agent", "session": "s-1", "harness": "claude" },
        "channel": "work", "approval_id": approval_id,
        "issued_ms": now, "expires_ms": now + 30_000,
        "nonce": uuid::Uuid::new_v4().simple().to_string(),
        "credentials": {},
    });
    if let Some(token) = preview_token {
        context["preview_token"] = json!(token);
    }
    context
}

pub fn header(context: &Value) -> (&'static str, String) {
    (
        "tracon-context",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(context.to_string()),
    )
}

pub struct Plugin {
    pub child: Child,
    pub socket: PathBuf,
    pub data: PathBuf,
    next: u64,
}

impl Plugin {
    /// Start `exec` on a socket in `dir` and wait until `initialize` answers,
    /// which is what ready means.
    pub async fn start_exec(exec: &Path, dir: &Path, plugin_toml: Option<&str>) -> Self {
        Self::start_full(exec, dir, plugin_toml, None).await
    }

    /// As [`Self::start_exec`], with `--node-socket` when `node_socket` is set.
    pub async fn start_full(
        exec: &Path,
        dir: &Path,
        plugin_toml: Option<&str>,
        node_socket: Option<&Path>,
    ) -> Self {
        let socket = dir.join("workboard.sock");
        let data = dir.join("data");
        if let Some(text) = plugin_toml {
            std::fs::create_dir_all(&data).unwrap();
            std::fs::write(data.join("plugin.toml"), text).unwrap();
        }
        let mut command = command(exec, &socket, &data);
        if let Some(node_socket) = node_socket {
            command.arg("--node-socket").arg(node_socket);
        }
        let child = command.spawn().unwrap();
        let plugin = Self {
            child,
            socket,
            data,
            next: 0,
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let init = json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize",
                               "params": { "protocolVersion": "2025-06-18", "capabilities": {},
                                           "clientInfo": { "name": "node", "version": "0" } } });
            if let Ok((200, v)) = plugin.post_json("/", &init).await {
                assert_eq!(v["result"]["serverInfo"]["name"], "workboard");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "initialize did not answer within 30 s"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        plugin
    }

    pub async fn start(dir: &Path, plugin_toml: Option<&str>) -> Self {
        Self::start_exec(Path::new(BIN), dir, plugin_toml).await
    }

    /// Started with `--node-socket node_socket`, as a node starts a plugin
    /// whose manifest names a node method.
    pub async fn start_with_node(
        dir: &Path,
        plugin_toml: Option<&str>,
        node_socket: &Path,
    ) -> Self {
        Self::start_full(Path::new(BIN), dir, plugin_toml, Some(node_socket)).await
    }

    pub async fn post_json(&self, target: &str, body: &Value) -> std::io::Result<(u16, Value)> {
        let headers = [("content-type", "application/json".to_string())];
        http(
            &self.socket,
            "POST",
            target,
            &headers,
            body.to_string().as_bytes(),
        )
        .await
    }

    pub async fn rpc(&mut self, method: &str, params: Value) -> (u16, Value) {
        self.next += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": self.next, "method": method, "params": params });
        self.post_json("/", &msg).await.unwrap()
    }

    /// An API request with `context` as its header (none when null).
    pub async fn api_with(
        &self,
        method: &str,
        target: &str,
        body: &str,
        context: &Value,
    ) -> (u16, Value) {
        let mut headers = vec![("content-type", "application/json".to_string())];
        if !context.is_null() {
            headers.push(header(context));
        }
        http(&self.socket, method, target, &headers, body.as_bytes())
            .await
            .unwrap()
    }

    /// An API request signed as the node signs one from the operator's UI.
    pub async fn api(&self, method: &str, target: &str, body: &str) -> (u16, Value) {
        let context = contract::sign(&unsigned(method, target, body.as_bytes()), &key());
        self.api_with(method, target, body, &context).await
    }

    /// `method` (`tools/call` or `tracon/preview`) of `tool`, signed as the
    /// node signs an agent's call.
    pub async fn tool_with(
        &mut self,
        method: &str,
        tool: &str,
        arguments: Value,
        approval_id: Option<&str>,
        preview_token: Option<&str>,
    ) -> Value {
        let context = contract::sign(
            &unsigned_call(tool, &arguments, approval_id, preview_token),
            &key(),
        );
        let params = json!({ "name": tool, "arguments": arguments,
                             "_meta": { "tracon": context } });
        let (status, answer) = self.rpc(method, params).await;
        assert_eq!(status, 200, "{answer}");
        answer
    }

    /// A read tool's result, which must succeed.
    pub async fn read(&mut self, tool: &str, arguments: Value) -> Value {
        let answer = self
            .tool_with("tools/call", tool, arguments, None, None)
            .await;
        assert_eq!(answer["result"]["isError"], false, "{answer}");
        let text = answer["result"]["content"][0]["text"].as_str().unwrap();
        serde_json::from_str(text).unwrap()
    }

    pub async fn task_count(&self) -> usize {
        let (status, board) = self.api("GET", "/api/board", "").await;
        assert_eq!(status, 200, "{board}");
        board["tasks"].as_array().unwrap().len()
    }
}

/// What a fake node's socket was sent: each request's bearer token and its
/// JSON-RPC body.
pub type NodeRequest = (Option<String>, Value);

#[derive(Clone, Default)]
pub struct NodeSeen(pub std::sync::Arc<std::sync::Mutex<Vec<NodeRequest>>>);

impl NodeSeen {
    pub fn requests(&self) -> Vec<NodeRequest> {
        self.0.lock().unwrap().clone()
    }
}

/// How a fake node answers a request: an HTTP status and a body.
pub type NodeReply = std::sync::Arc<dyn Fn(&Value) -> (u16, Value) + Send + Sync>;

/// A stand-in for the node's plugin-to-node socket at `socket`, answering
/// `POST /` with `reply`.
pub async fn fake_node(socket: &Path, reply: NodeReply) -> NodeSeen {
    use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
    let seen = NodeSeen::default();
    let app = Router::new()
        .route(
            "/",
            post(
                |State((seen, reply)): State<(NodeSeen, NodeReply)>,
                 headers: HeaderMap,
                 Json(body): Json<Value>| async move {
                    let bearer = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.strip_prefix("Bearer "))
                        .map(str::to_owned);
                    seen.0.lock().unwrap().push((bearer, body.clone()));
                    let (status, answer) = reply(&body);
                    (
                        axum::http::StatusCode::from_u16(status).unwrap(),
                        Json(answer),
                    )
                },
            ),
        )
        .with_state((seen.clone(), reply));
    let listener = tokio::net::UnixListener::bind(socket).unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    seen
}

pub const SITE: &str = "https://example.atlassian.net";

pub fn issue(id: &str, key: &str, category: &str) -> Value {
    json!({
        "id": id, "key": key, "summary": "A summary", "status": "In Progress",
        "status_category": category, "priority": "Planned", "issue_type": "Task",
        "labels": ["phase-3"],
        "parent": {"key": "ABC-1", "summary": "The epic", "issue_type": "Epic"}
    })
}

pub fn page(issues: Vec<Value>, next: Option<&str>) -> Value {
    json!({"site": SITE, "account_email": "user@example.com", "issues": issues, "next_cursor": next})
}

/// A `tools/call` answer from the node carrying `page` as jira-review does,
/// as JSON text.
pub fn page_answer(id: &Value, page: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id,
            "result": { "content": [{ "type": "text", "text": page.to_string() }] } })
}

/// Two pages joined by the cursor `next-page`.
pub fn two_pages() -> NodeReply {
    std::sync::Arc::new(|request: &Value| {
        let cursor = request["params"]["arguments"]["cursor"].as_str();
        let body = match cursor {
            None => page(vec![issue("1", "ABC-2", "new")], Some("next-page")),
            Some("next-page") => page(vec![issue("2", "ABC-3", "done")], None),
            Some(other) => panic!("unexpected cursor {other}"),
        };
        (200, page_answer(&request["id"], &body))
    })
}

const FAKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/support/fake_jira_review.py"
);

/// A `jira-review` stand-in for `mode` that logs to `log`: a shell script
/// running the Python fake with its own options first.
pub fn fake_jira_review(dir: &Path, mode: &str) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let log = dir.join(format!("{mode}.log"));
    let program = dir.join(format!("jira-review-{mode}"));
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\nexec python3 '{FAKE}' --log '{}' --mode {mode} \"$@\"\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    (program, log)
}

pub fn logged(log: &Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
