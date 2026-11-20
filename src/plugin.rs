//! `workboard serve --socket <path> --data <dir>`: Workboard as a Tracon
//! plugin (contract v1). The node serves the SPA from the installed archive's
//! `ui/` and forwards the SPA's `api/...` requests here over a Unix socket,
//! each with a signed caller context. This process answers MCP at `POST /`
//! (with no tools) and the REST API at `/api/*`, and nothing else.

use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::Path;
use std::sync::{Arc, atomic::AtomicBool};

use anyhow::{Context, Result, anyhow, bail};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::post,
};
use base64::Engine;
use serde_json::{Value, json};

use crate::{
    config::PluginConfig,
    contract::{self, ContractEnv, NonceCache},
    http::{self, App, BODY_LIMIT, JiraChannel, JiraSite},
    store::Store,
};

/// The contract version this build serves.
pub const CONTRACT: &str = "1";
/// The request header carrying the signed caller context.
pub const CONTEXT_HEADER: &str = "tracon-context";

/// What checks each forwarded API request.
pub struct Gate {
    pub env: ContractEnv,
    pub nonces: NonceCache,
}

impl Gate {
    /// From the start environment, which a node fills with the contract's own
    /// variables and nothing else.
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let token = get("TRACON_PLUGIN_TOKEN")
            .filter(|t| !t.is_empty())
            .context("TRACON_PLUGIN_TOKEN is not set; plugin mode is started by a Tracon node")?;
        let key = get("TRACON_NODE_KEY")
            .filter(|k| !k.is_empty())
            .context("TRACON_NODE_KEY is not set; plugin mode is started by a Tracon node")?;
        if let Some(v) = get("TRACON_CONTRACT").filter(|v| v != CONTRACT) {
            bail!("TRACON_CONTRACT is {v}; this build serves contract {CONTRACT}");
        }
        Ok(Self {
            env: ContractEnv::new(&token, &key).map_err(|e| anyhow!(e))?,
            nonces: NonceCache::default(),
        })
    }
}

/// The plugin-mode app for the board in `data`, configured by
/// `<data>/plugin.toml`.
pub fn app(data: &Path) -> Result<App> {
    let config_path = data.join("plugin.toml");
    let config = PluginConfig::load(&config_path)?;
    let store = Arc::new(Store::open(data, config.jira_site.as_ref())?);
    let site = match config.jira_site {
        Some(site) => JiraSite::Configured(site),
        None => JiraSite::Missing {
            hint: format!(
                "Jira refresh needs a Jira site. Add jira_site = \"https://your-site.atlassian.net\" to {}, then restart Workboard in Tracon.",
                config_path.display()
            ),
        },
    };
    Ok(App {
        store,
        site,
        tracon: config.tracon_url,
        refresh_in_progress: Arc::new(AtomicBool::new(false)),
    })
}

/// The plugin-mode router: MCP at `POST /`, the API behind the caller-context
/// check, and no SPA.
pub fn router(app: App, gate: Arc<Gate>) -> Router {
    http::api()
        .route("/", post(mcp))
        .fallback(fallback)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(middleware::from_fn_with_state(gate, check))
        .with_state(app)
}

pub async fn run(socket: &Path, data: &Path) -> Result<()> {
    let gate = Arc::new(Gate::from_env(|k| std::env::var(k).ok())?);
    let app = app(data)?;
    match std::fs::symlink_metadata(socket) {
        Ok(m) if m.file_type().is_socket() => {
            std::fs::remove_file(socket).with_context(|| socket.display().to_string())?;
        }
        Ok(_) => bail!(
            "{} exists and is not a socket; not replacing it",
            socket.display()
        ),
        Err(_) => {}
    }
    let listener = tokio::net::UnixListener::bind(socket)
        .with_context(|| format!("binding {}", socket.display()))?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
        .with_context(|| socket.display().to_string())?;
    let served = axum::serve(listener, router(app, gate))
        .with_graceful_shutdown(shutdown())
        .await
        .context("serving workboard");
    let _ = std::fs::remove_file(socket);
    served
}

async fn shutdown() {
    let Ok(mut term) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    else {
        let _ = tokio::signal::ctrl_c().await;
        return;
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

fn is_api(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn context(request: &Request) -> Value {
    request
        .headers()
        .get(CONTEXT_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(v.trim())
                .ok()
        })
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

/// Every `/api` request must carry the operator's caller context, signed by
/// the node for exactly this method, target and body. This replaces the
/// standalone server's Host and Origin checks: the node has already decided
/// the browser's request, and only the node can reach this socket.
async fn check(State(gate): State<Arc<Gate>>, request: Request, next: Next) -> Response {
    if !is_api(request.uri().path()) {
        return next.run(request).await;
    }
    let meta = context(&request);
    let (mut parts, body) = request.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, BODY_LIMIT).await else {
        return http::error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "invalid_request",
            "Request body could not be read or exceeds 64 KiB.",
        );
    };
    let target = parts
        .uri
        .path_and_query()
        .map_or_else(|| parts.uri.path(), |p| p.as_str());
    let arguments = contract::ui_arguments_sha256(parts.method.as_str(), target, &bytes);
    let mut response =
        match contract::verify_ui(&meta, &arguments, &gate.env, now_ms(), &gate.nonces) {
            Err(refusal) => http::error(StatusCode::UNAUTHORIZED, "unauthorized", &refusal.reason),
            Ok(caller) => {
                parts.extensions.insert(JiraChannel(caller.channel));
                next.run(Request::from_parts(parts, Body::from(bytes)))
                    .await
            }
        };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn fallback(uri: Uri) -> Response {
    if is_api(uri.path()) {
        http::missing_api().await
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

/// The MCP protocol revisions this server knows, newest first.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// One JSON-RPC message per POST: its answer as JSON, or 202 for a
/// notification. Workboard offers no tools; it is a plugin for its UI.
async fn mcp(body: Bytes) -> Response {
    let message: Value = match serde_json::from_slice(&body) {
        Ok(message) => message,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(rpc_error(Value::Null, -32700, &format!("parse error: {e}"))),
            )
                .into_response();
        }
    };
    match answer(&message) {
        Some(answer) => Json(answer).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The answer to one JSON-RPC message; `None` for a notification or a
/// response, which get no answer.
pub fn answer(message: &Value) -> Option<Value> {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        if message.get("result").is_some() || message.get("error").is_some() {
            return None;
        }
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        return Some(rpc_error(id, -32600, "invalid request"));
    };
    let id = message.get("id").cloned()?;
    let result = match method {
        "initialize" => {
            let asked = message["params"]["protocolVersion"]
                .as_str()
                .unwrap_or_default();
            let version = PROTOCOL_VERSIONS
                .into_iter()
                .find(|v| *v == asked)
                .unwrap_or(PROTOCOL_VERSIONS[0]);
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": contract::PLUGIN, "version": env!("CARGO_PKG_VERSION") },
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({ "tools": [] }),
        "tools/call" => return Some(rpc_error(id, -32602, "Workboard has no tools")),
        other => return Some(rpc_error(id, -32601, &format!("no method {other}"))),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| {
            pairs
                .iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn start_needs_the_contract_environment() {
        let key = "11".repeat(32);
        assert!(Gate::from_env(env(&[])).is_err());
        assert!(Gate::from_env(env(&[("TRACON_PLUGIN_TOKEN", "t")])).is_err());
        assert!(Gate::from_env(env(&[("TRACON_NODE_KEY", &key)])).is_err());
        let ok = [("TRACON_PLUGIN_TOKEN", "t"), ("TRACON_NODE_KEY", &key)];
        assert!(Gate::from_env(env(&ok)).is_ok());
        let one = [("TRACON_CONTRACT", "1"), ok[0], ok[1]];
        assert!(Gate::from_env(env(&one)).is_ok());
        let two = [("TRACON_CONTRACT", "2"), ok[0], ok[1]];
        assert!(Gate::from_env(env(&two)).is_err());
    }

    #[test]
    fn notifications_get_no_answer() {
        assert_eq!(
            answer(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })),
            None
        );
        let ping = answer(&json!({ "jsonrpc": "2.0", "id": 3, "method": "ping" })).unwrap();
        assert_eq!(ping["result"], json!({}));
    }
}
