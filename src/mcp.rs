//! A small MCP server: JSON-RPC 2.0 with `initialize`, `ping`, `tools/list`,
//! `tools/call` and Tracon's `tracon/preview`, independent of transport.
//! `serve` carries it over HTTP on the plugin socket, and `mcp` as lines on
//! stdio. The tools are the operation table's (`ops::OPS`), one per HTTP
//! route. What a write does is the [`Host`]'s: [`Plugin`] applies it only
//! with the node's approval and against the preview the operator saw;
//! [`Standalone`] proposes it for a person to review with `workboard
//! review`.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::contract::{self, Caller, ContractEnv, NonceCache};
use crate::ops::{self, Ctx, OpError};
use crate::preview::{self, ApplyError};
use crate::proposals::{self, Outcome, Proposal, Proposals};
use crate::store::Store;

/// The newest protocol revision this server speaks, and the one it answers
/// with when a client asks for one it does not know.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
const KNOWN_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Code for a call refused by its caller context.
pub const REFUSED: i64 = -32001;
/// Code for an approved write refused because what it names moved since it
/// was previewed; its `data` is `{"stale": true}`.
pub const STALE: i64 = -32002;
/// Code for a preview that could not be taken: the write would be refused.
pub const NOT_PREVIEWED: i64 = -32003;

#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

/// What a server does with the calls it is sent.
pub trait Host: Send + Sync {
    fn tools(&self) -> Vec<Value>;

    fn call(
        &self,
        name: &str,
        arguments: Option<&Value>,
        meta: Option<&Value>,
    ) -> impl Future<Output = Result<Value, RpcError>> + Send;

    fn preview(&self, _params: &Value) -> impl Future<Output = Result<Value, RpcError>> + Send {
        async { Err(RpcError::new(-32601, "no method tracon/preview")) }
    }
}

/// A tool's answer: the value as JSON text, and as structured content when
/// it is an object.
pub fn tool_ok(v: &Value) -> Value {
    let text = serde_json::to_string_pretty(v).unwrap_or_default();
    let mut result = json!({ "content": [{ "type": "text", "text": text }], "isError": false });
    if v.is_object() {
        result["structuredContent"] = v.clone();
    }
    result
}

/// A tool that ran and could not do what was asked: the error body the HTTP
/// API answers with, for the caller to read and act on.
pub fn tool_error(err: &OpError) -> Value {
    let text = serde_json::to_string_pretty(&err.body()).unwrap_or_default();
    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
}

fn error_response(id: Value, e: RpcError) -> Value {
    let mut error = json!({ "code": e.code, "message": e.message });
    if let Some(data) = e.data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

pub fn parse_error(e: &str) -> Value {
    error_response(
        Value::Null,
        RpcError::new(-32700, format!("parse error: {e}")),
    )
}

/// Answer one JSON-RPC message; `None` for a notification or a response,
/// which get no answer.
pub async fn handle<H: Host>(host: &H, msg: Value) -> Option<Value> {
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        if msg.get("result").is_some() || msg.get("error").is_some() {
            return None;
        }
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        return Some(error_response(id, RpcError::new(-32600, "invalid request")));
    };
    let id = msg.get("id").cloned()?;
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let version = KNOWN_VERSIONS
                .into_iter()
                .find(|v| *v == asked)
                .unwrap_or(PROTOCOL_VERSION);
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": contract::PLUGIN, "version": env!("CARGO_PKG_VERSION") },
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": host.tools() })),
        "tools/call" => match params["name"].as_str() {
            None => Err(RpcError::new(-32602, "tools/call needs a tool name")),
            Some(name) => {
                let meta = params.get("_meta").filter(|m| !m.is_null());
                host.call(name, params.get("arguments"), meta).await
            }
        },
        "tracon/preview" => host.preview(&params).await,
        other => Err(RpcError::new(-32601, format!("no method {other}"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(e) => error_response(id, e),
    })
}

fn unknown(name: &str) -> RpcError {
    RpcError::new(-32602, format!("workboard has no tool {name}"))
}

fn args_of(arguments: Option<&Value>) -> Value {
    arguments
        .filter(|a| !a.is_null())
        .cloned()
        .unwrap_or_else(|| json!({}))
}

async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> T + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(job)
        .await
        .map_err(|_| RpcError::new(-32603, "the call failed"))
}

fn stale(message: String) -> RpcError {
    RpcError {
        code: STALE,
        message,
        data: Some(json!({ "stale": true })),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// `serve`'s tools: every call checked against the node's signed caller
/// context, and a write applied only with the node's approval.
pub struct Plugin {
    pub store: Arc<Store>,
    pub refresh_in_progress: Arc<AtomicBool>,
    pub env: ContractEnv,
    pub nonces: NonceCache,
}

const PLUGIN_WRITE: &str = "Applied only once the operator approves this call on a card showing the change; a version or preview that has moved since is refused as stale, and nothing is written.";

impl Plugin {
    fn verify(
        &self,
        name: &str,
        arguments: Option<&Value>,
        meta: Option<&Value>,
        write: bool,
    ) -> Result<Caller, RpcError> {
        let context = meta.map_or(&Value::Null, |m| &m["tracon"]);
        contract::verify_call(
            context,
            name,
            arguments,
            write,
            &self.env,
            now_ms(),
            &self.nonces,
        )
        .map_err(|r| {
            eprintln!(
                "workboard: refused {name}: {} (check {})",
                r.reason, r.check
            );
            RpcError {
                code: REFUSED,
                message: format!("refused: {}", r.reason),
                data: Some(json!({ "check": r.check })),
            }
        })
    }
}

impl Host for Plugin {
    fn tools(&self) -> Vec<Value> {
        ops::tools(PLUGIN_WRITE)
    }

    async fn call(
        &self,
        name: &str,
        arguments: Option<&Value>,
        meta: Option<&Value>,
    ) -> Result<Value, RpcError> {
        let op = ops::find(name).ok_or_else(|| unknown(name))?;
        let caller = self.verify(name, arguments, meta, op.write)?;
        let args = args_of(arguments);
        let store = self.store.clone();
        let refresh = self.refresh_in_progress.load(Ordering::Acquire);
        let token = caller.preview_token.clone();
        let write = op.write;
        let ran = blocking(move || {
            let ctx = Ctx {
                store: &store,
                refresh_in_progress: refresh,
            };
            if write {
                preview::apply(&ctx, op.name, &args, token.as_deref())
            } else {
                ops::run(&ctx, op.name, &args).map_err(ApplyError::Op)
            }
        })
        .await?;
        if write {
            // The author of a change is the caller the node attests, never a
            // name in the arguments. Workboard keeps no history of its own, so
            // this line in the plugin log is its record.
            eprintln!(
                "workboard: {name} by {} (session {}, harness {}) on approval {}: {}",
                caller.kind,
                caller.session.as_deref().unwrap_or("-"),
                caller.harness.as_deref().unwrap_or("-"),
                caller.approval_id.as_deref().unwrap_or("-"),
                match &ran {
                    Ok(_) => "applied",
                    Err(ApplyError::Stale(_)) => "stale",
                    Err(ApplyError::Op(_)) => "refused",
                }
            );
        }
        match ran {
            Ok(value) => Ok(tool_ok(&value)),
            Err(ApplyError::Stale(message)) => Err(stale(message)),
            Err(ApplyError::Op(err)) => Ok(tool_error(&err)),
        }
    }

    /// Checks 1 to 6 of the caller context, as for a call; a preview needs
    /// no approval and writes nothing.
    async fn preview(&self, params: &Value) -> Result<Value, RpcError> {
        let name = params["name"]
            .as_str()
            .ok_or_else(|| RpcError::new(-32602, "tracon/preview needs a tool name"))?;
        let op = ops::find(name).ok_or_else(|| unknown(name))?;
        let arguments = params.get("arguments");
        let meta = params.get("_meta").filter(|m| !m.is_null());
        self.verify(name, arguments, meta, false)?;
        if !op.write {
            return Ok(json!({
                "previewable": false,
                "summary": format!("{name} only reads"),
                "body": format!("`{name}` reads the board and changes nothing."),
                "token": null,
            }));
        }
        let args = args_of(arguments);
        let store = self.store.clone();
        let shown = blocking(move || preview::preview(&store, op.name, &args)).await?;
        let shown = shown.map_err(|e| RpcError::new(NOT_PREVIEWED, e.to_string()))?;
        Ok(json!({
            "previewable": true,
            "summary": shown.summary,
            "body": shown.body,
            "token": shown.token,
        }))
    }
}

/// `workboard mcp`: reads answer from the board, and writes wait as
/// proposals for `workboard review`.
pub struct Standalone {
    pub store: Arc<Store>,
    pub proposals: Proposals,
}

const STANDALONE_WRITE: &str = "Nothing is written yet: this proposes the change and returns a proposal_id with a preview of it. A person approves or rejects it with `workboard review`; wait for that with proposal_status.";

/// The longest `proposal_status` waits in one call.
pub const MAX_WAIT_SECS: u64 = 45;

impl Standalone {
    async fn status(&self, args: &Value) -> Result<Value, OpError> {
        let id = args["id"]
            .as_str()
            .ok_or_else(|| OpError::BadInput("proposal_status needs an id".into()))?
            .to_string();
        let wait = match args.get("wait_secs") {
            None | Some(Value::Null) => 30,
            Some(v) => v
                .as_u64()
                .ok_or_else(|| OpError::BadInput("wait_secs expects a whole number".into()))?,
        }
        .min(MAX_WAIT_SECS);
        let deadline = Instant::now() + Duration::from_secs(wait);
        loop {
            let found = self.proposals.get(&id).map_err(OpError::BadInput)?;
            let Some(p) = found else {
                return Err(OpError::BadInput(format!("no proposal {id}")));
            };
            if p.outcome.is_some() || Instant::now() >= deadline {
                return Ok(p.status());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

impl Host for Standalone {
    fn tools(&self) -> Vec<Value> {
        let mut tools = ops::tools(STANDALONE_WRITE);
        tools.push(json!({
            "name": "proposal_status",
            "description": format!(
                "Wait for a proposed write to be decided, up to wait_secs (at most \
                 {MAX_WAIT_SECS}, 30 unless you say). status is pending while nobody has \
                 decided; approved with the result; rejected with the reviewer's reason; stale \
                 when the board moved after it was proposed (read again and propose anew); \
                 failed when the board refused it."
            ),
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "string", "description": "The proposal_id a write returned." },
                "wait_secs": { "type": "integer" },
            }, "required": ["id"] },
            "annotations": { "readOnlyHint": true, "openWorldHint": false },
        }));
        tools
    }

    async fn call(
        &self,
        name: &str,
        arguments: Option<&Value>,
        _meta: Option<&Value>,
    ) -> Result<Value, RpcError> {
        let args = args_of(arguments);
        if name == "proposal_status" {
            return Ok(match self.status(&args).await {
                Ok(v) => tool_ok(&v),
                Err(e) => tool_error(&e),
            });
        }
        let op = ops::find(name).ok_or_else(|| unknown(name))?;
        let store = self.store.clone();
        let proposals = self.proposals.clone();
        let answer = blocking(move || -> Result<Value, OpError> {
            if !op.write {
                let ctx = Ctx {
                    store: &store,
                    refresh_in_progress: false,
                };
                return ops::run(&ctx, op.name, &args);
            }
            let shown = preview::preview(&store, op.name, &args)?;
            let p = Proposal::new(op.name, args, shown);
            proposals.put(&p).map_err(OpError::BadInput)?;
            Ok(json!({
                "proposal_id": p.id,
                "status": "pending",
                "summary": p.summary,
                "preview": p.body,
                "review": "run `workboard review` in a terminal to approve or reject it; wait with proposal_status",
            }))
        })
        .await?;
        Ok(match answer {
            Ok(v) => tool_ok(&v),
            Err(e) => tool_error(&e),
        })
    }
}

/// Apply proposal `p` as its reviewer approved it, against the board it was
/// previewed on.
pub fn approve(store: &Store, p: &Proposal) -> Outcome {
    let ctx = Ctx {
        store,
        refresh_in_progress: false,
    };
    match preview::apply(&ctx, &p.tool, &p.arguments, p.token.as_deref()) {
        Ok(result) => Outcome::new(proposals::Status::Approved, Some(result), None),
        Err(ApplyError::Stale(m)) => Outcome::new(proposals::Status::Stale, None, Some(m)),
        Err(ApplyError::Op(e)) => Outcome::new(
            proposals::Status::Failed,
            Some(e.body()),
            Some(e.to_string()),
        ),
    }
}
