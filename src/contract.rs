//! The caller context a Tracon node signs onto each tool call and preview,
//! and onto each request it forwards from its operator UI, and its check.
//!
//! A `tools/call` or `tracon/preview` carries it as `params._meta.tracon`,
//! with `tool` the unprefixed tool name and `arguments_sha256` the hash of
//! the canonical JSON of the arguments, and optionally `preview_token` on a
//! call dispatched from an approved card. For the UI, the node proxies `/t/<token>/api/<rest>` from the
//! operator's browser to this plugin's socket as `/api/<rest>`, with a
//! `tracon-context` header holding the base64url (no padding) of the signed
//! context JSON. The object is the one Tracon signs onto `tools/call`
//! (`v`, `plugin`, `instance`, `tool`, `arguments_sha256`, `caller`,
//! `channel`, `approval_id`, `issued_ms`, `expires_ms`, `nonce`,
//! `credentials`, `signature`), with `tool = "tracon/ui"` and
//! `arguments_sha256` binding the request's method, target and body. The
//! signature is ed25519 by the node key over the ASCII label
//! `tracon/v1/plugin-call`, a big-endian u32 length and the canonical JSON of
//! the object without `signature`. Ported from jira-review's `contract.rs`.

use std::collections::HashMap;
use std::sync::Mutex;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// What the signature is over, ahead of the context.
pub const LABEL: &str = "tracon/v1/plugin-call";
/// The manifest name a context must be addressed to.
pub const PLUGIN: &str = "workboard";
/// The tool name the node signs UI requests as; no real tool can have it.
pub const UI_TOOL: &str = "tracon/ui";
/// Clock skew allowed either side of a context's window.
pub const SKEW_MS: i64 = 5_000;
/// The longest window a context may claim.
pub const MAX_WINDOW_MS: i64 = 60_000;

/// JSON with object keys sorted by byte order and no whitespace; strings and
/// keys escaped as serde_json escapes them, so both sides write the same
/// bytes as Tracon's `proto::admin::write_canonical`.
pub fn canonical(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String((*key).clone()).to_string());
                out.push(':');
                write_canonical(&map[*key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

fn has_float(v: &Value) -> bool {
    match v {
        Value::Number(n) => n.is_f64(),
        Value::Array(a) => a.iter().any(has_float),
        Value::Object(o) => o.values().any(has_float),
        _ => false,
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// What a context carries as `instance`: the hash of the plugin token, so the
/// token itself never travels.
pub fn instance_of(token: &str) -> String {
    sha256_hex(token.as_bytes())
}

/// The `arguments_sha256` of a UI request: the hash of the canonical JSON of
/// its uppercase method, its target (path plus `?` and the raw query when
/// there is one) and the hash of its exact body bytes.
pub fn ui_arguments_sha256(method: &str, path: &str, body: &[u8]) -> String {
    let arguments = json!({
        "body_sha256": sha256_hex(body),
        "method": method.to_ascii_uppercase(),
        "path": path,
    });
    sha256_hex(canonical(&arguments).as_bytes())
}

/// The bytes a context is signed over: the label, then the length-prefixed
/// canonical JSON of the context with any `signature` removed.
pub fn signing_bytes(context: &Value) -> Vec<u8> {
    let mut unsigned = context.clone();
    if let Some(o) = unsigned.as_object_mut() {
        o.remove("signature");
    }
    let json = canonical(&unsigned);
    let mut out = LABEL.as_bytes().to_vec();
    out.extend_from_slice(&(json.len() as u32).to_be_bytes());
    out.extend_from_slice(json.as_bytes());
    out
}

/// `context` signed by `key`, as the node signs it. For tests.
pub fn sign(context: &Value, key: &SigningKey) -> Value {
    let signature = key.sign(&signing_bytes(context));
    let mut signed = context.clone();
    signed["signature"] = Value::String(hex::encode(signature.to_bytes()));
    signed
}

/// What plugin mode was started with.
#[derive(Clone)]
pub struct ContractEnv {
    /// `hex(sha256(TRACON_PLUGIN_TOKEN))`.
    pub instance: String,
    pub node_key: VerifyingKey,
}

impl ContractEnv {
    pub fn new(plugin_token: &str, node_key_hex: &str) -> Result<Self, String> {
        if plugin_token.is_empty() {
            return Err("TRACON_PLUGIN_TOKEN is empty".into());
        }
        let bytes: [u8; 32] = hex::decode(node_key_hex.trim())
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or("TRACON_NODE_KEY is not 32 bytes of hex")?;
        let node_key = VerifyingKey::from_bytes(&bytes)
            .map_err(|e| format!("TRACON_NODE_KEY is not an ed25519 key: {e}"))?;
        Ok(Self {
            instance: instance_of(plugin_token),
            node_key,
        })
    }
}

/// Nonces seen and the `expires_ms` of the context each came in, pruned once a
/// context that old could no longer pass the clock check.
#[derive(Default)]
pub struct NonceCache(Mutex<HashMap<String, i64>>);

impl NonceCache {
    /// Record `nonce`; false when it was already there.
    fn fresh(&self, nonce: &str, expires_ms: i64, now_ms: i64) -> bool {
        let mut seen = self.0.lock().unwrap_or_else(|e| e.into_inner());
        seen.retain(|_, expires| *expires + SKEW_MS >= now_ms);
        if seen.contains_key(nonce) {
            return false;
        }
        seen.insert(nonce.to_string(), expires_ms);
        true
    }
}

/// Who a verified request is from, as the node signed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// `operator`, `agent` or `plugin`.
    pub kind: String,
    pub session: Option<String>,
    pub harness: Option<String>,
    pub channel: String,
    /// The card the call was allowed on, if any.
    pub approval_id: Option<String>,
    /// The token of the preview that card showed, if any.
    pub preview_token: Option<String>,
}

/// Why a request was refused, and the check (1–7) that refused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub check: u8,
    pub reason: String,
}

fn refuse<T>(check: u8, reason: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal {
        check,
        reason: reason.into(),
    })
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The hash a tool call's context carries of its arguments; missing or null
/// arguments count as `{}`.
pub fn arguments_sha256(arguments: Option<&Value>) -> String {
    let empty = json!({});
    let arguments = arguments.filter(|a| !a.is_null()).unwrap_or(&empty);
    sha256_hex(canonical(arguments).as_bytes())
}

/// Check a UI request's context, in the order the contract fixes:
///
/// 1. `v` is 1 and `plugin` is this plugin (and the context is well formed)
/// 2. `instance` is the hash of this process's plugin token
/// 3. `tool` is `tracon/ui` and `arguments_sha256` matches `arguments_sha256`,
///    recomputed from the request actually received
/// 4. the signature verifies under the node key
/// 5. now is within the window, give or take [`SKEW_MS`], and the window is at
///    most [`MAX_WINDOW_MS`]
/// 6. the nonce has not been seen
/// 7. the caller is the operator
pub fn verify_ui(
    meta: &Value,
    arguments_sha256: &str,
    env: &ContractEnv,
    now_ms: i64,
    nonces: &NonceCache,
) -> Result<Caller, Refusal> {
    let caller = verify(meta, UI_TOOL, arguments_sha256, env, now_ms, nonces)?;
    // 7
    if caller.kind != "operator" {
        return refuse(7, "only the operator may use the Workboard UI");
    }
    Ok(caller)
}

/// Check the context of a `tools/call` or `tracon/preview` of `tool` with
/// `arguments`, checks 1 to 6 as for [`verify_ui`], then 7: a call of a
/// tool that writes carries an `approval_id` (`write` is false for a
/// preview, which writes nothing).
pub fn verify_call(
    meta: &Value,
    tool: &str,
    arguments: Option<&Value>,
    write: bool,
    env: &ContractEnv,
    now_ms: i64,
    nonces: &NonceCache,
) -> Result<Caller, Refusal> {
    if tool == UI_TOOL {
        return refuse(3, format!("{UI_TOOL} is not a tool"));
    }
    let caller = verify(
        meta,
        tool,
        &arguments_sha256(arguments),
        env,
        now_ms,
        nonces,
    )?;
    // 7
    if write && caller.approval_id.is_none() {
        return refuse(
            7,
            format!("{tool} writes, and this call carries no approval"),
        );
    }
    Ok(caller)
}

/// Checks 1 to 6, for a context that must be for `tool` with arguments
/// hashing to `arguments_sha256`.
fn verify(
    meta: &Value,
    tool_wanted: &str,
    arguments_sha256: &str,
    env: &ContractEnv,
    now_ms: i64,
    nonces: &NonceCache,
) -> Result<Caller, Refusal> {
    // 1
    let Some(ctx) = meta.as_object() else {
        return refuse(1, "the request carries no caller context");
    };
    if ctx.get("v").and_then(Value::as_i64) != Some(1) {
        return refuse(1, "caller context version is not 1");
    }
    if ctx.get("plugin").and_then(Value::as_str) != Some(PLUGIN) {
        return refuse(1, format!("caller context is not for {PLUGIN}"));
    }
    if has_float(meta) {
        return refuse(1, "caller context holds a non-integer number");
    }
    let str_of = |k: &str| ctx.get(k).and_then(Value::as_str);
    let opt_str = |v: Option<&Value>, k: &str| -> Result<Option<String>, Refusal> {
        match v {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => refuse(1, format!("caller context {k} is not a string or null")),
        }
    };
    let (Some(instance), Some(tool), Some(args_hash), Some(channel), Some(nonce), Some(sig)) = (
        str_of("instance"),
        str_of("tool"),
        str_of("arguments_sha256"),
        str_of("channel"),
        str_of("nonce"),
        str_of("signature"),
    ) else {
        return refuse(1, "caller context is missing a field");
    };
    let (Some(issued_ms), Some(expires_ms)) = (
        ctx.get("issued_ms").and_then(Value::as_i64),
        ctx.get("expires_ms").and_then(Value::as_i64),
    ) else {
        return refuse(1, "caller context has no integer issued_ms and expires_ms");
    };
    let Some(caller) = ctx.get("caller").and_then(Value::as_object) else {
        return refuse(1, "caller context has no caller kind");
    };
    let Some(kind) = caller.get("kind").and_then(Value::as_str) else {
        return refuse(1, "caller context has no caller kind");
    };
    let session = opt_str(caller.get("session"), "caller.session")?;
    let harness = opt_str(caller.get("harness"), "caller.harness")?;
    let approval_id = opt_str(ctx.get("approval_id"), "approval_id")?;
    let preview_token = opt_str(ctx.get("preview_token"), "preview_token")?;

    // 2
    if instance != env.instance {
        return refuse(2, "caller context is for another instance of this plugin");
    }

    // 3
    if tool != tool_wanted {
        return refuse(
            3,
            format!("caller context is for tool {tool}, not {tool_wanted}"),
        );
    }
    if args_hash != arguments_sha256 {
        return refuse(3, "caller context is for another request");
    }

    // 4
    let signature = hex::decode(sig)
        .ok()
        .and_then(|b| <[u8; 64]>::try_from(b).ok())
        .map(|b| Signature::from_bytes(&b));
    let Some(signature) = signature else {
        return refuse(4, "caller context signature is not 64 bytes of hex");
    };
    if env
        .node_key
        .verify_strict(&signing_bytes(meta), &signature)
        .is_err()
    {
        return refuse(
            4,
            "caller context signature does not verify under the node key",
        );
    }

    // 5
    if expires_ms < issued_ms || expires_ms - issued_ms > MAX_WINDOW_MS {
        return refuse(5, "caller context window is longer than 60 s");
    }
    if now_ms < issued_ms - SKEW_MS {
        return refuse(5, "caller context is issued in the future");
    }
    if now_ms > expires_ms + SKEW_MS {
        return refuse(5, "caller context has expired");
    }

    // 6
    if !is_hex(nonce, 32) {
        return refuse(6, "caller context nonce is not 32 lowercase hex characters");
    }
    if !nonces.fresh(nonce, expires_ms, now_ms) {
        return refuse(6, "caller context nonce was already used");
    }

    Ok(Caller {
        kind: kind.to_string(),
        session,
        harness,
        channel: channel.to_string(),
        approval_id,
        preview_token,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7; 32])
    }

    fn env() -> ContractEnv {
        ContractEnv::new("token", &hex::encode(key().verifying_key().to_bytes())).unwrap()
    }

    const NONCE: &str = "00112233445566778899aabbccddeeff";

    fn context(args: &str, now: i64, nonce: &str, kind: &str) -> Value {
        sign(
            &json!({
                "v": 1, "plugin": PLUGIN, "instance": instance_of("token"), "tool": UI_TOOL,
                "arguments_sha256": args,
                "caller": { "kind": kind, "session": null, "harness": null },
                "channel": "work", "approval_id": null,
                "issued_ms": now, "expires_ms": now + 30_000, "nonce": nonce,
                "credentials": {},
            }),
            &key(),
        )
    }

    #[test]
    fn canonical_json_sorts_keys_and_drops_whitespace() {
        assert_eq!(
            canonical(&json!({ "b": [1, { "d": null, "c": "x\"y" }], "a": true })),
            r#"{"a":true,"b":[1,{"c":"x\"y","d":null}]}"#
        );
    }

    #[test]
    fn ui_arguments_bind_method_path_and_body() {
        let base = ui_arguments_sha256("POST", "/api/tasks", b"{}");
        assert_eq!(base, ui_arguments_sha256("post", "/api/tasks", b"{}"));
        assert_ne!(base, ui_arguments_sha256("PUT", "/api/tasks", b"{}"));
        assert_ne!(base, ui_arguments_sha256("POST", "/api/tasks?x=1", b"{}"));
        assert_ne!(base, ui_arguments_sha256("POST", "/api/tasks", b"{ }"));
        let expected = sha256_hex(
            format!(
                r#"{{"body_sha256":"{}","method":"GET","path":"/api/board"}}"#,
                sha256_hex(b"")
            )
            .as_bytes(),
        );
        assert_eq!(ui_arguments_sha256("GET", "/api/board", b""), expected);
    }

    #[test]
    fn a_good_operator_context_gives_its_channel() {
        let args = ui_arguments_sha256("GET", "/api/board", b"");
        let ctx = context(&args, 1_000_000, NONCE, "operator");
        let caller = verify_ui(&ctx, &args, &env(), 1_000_000, &NonceCache::default()).unwrap();
        assert_eq!(caller.channel, "work");
    }

    #[test]
    fn each_check_refuses_in_order() {
        let args = ui_arguments_sha256("GET", "/api/board", b"");
        let nonces = NonceCache::default();
        let check = |ctx: &Value, args: &str, now: i64, nonces: &NonceCache| {
            verify_ui(ctx, args, &env(), now, nonces).unwrap_err().check
        };
        assert_eq!(check(&Value::Null, &args, 0, &nonces), 1);
        let mut other = context(&args, 1_000_000, NONCE, "operator");
        other["instance"] = json!(instance_of("other"));
        assert_eq!(check(&sign(&other, &key()), &args, 1_000_000, &nonces), 2);
        let mut tool = context(&args, 1_000_000, NONCE, "operator");
        tool["tool"] = json!("search");
        assert_eq!(check(&sign(&tool, &key()), &args, 1_000_000, &nonces), 3);
        let ctx = context(&args, 1_000_000, NONCE, "operator");
        let elsewhere = ui_arguments_sha256("GET", "/api/tasks/x", b"");
        assert_eq!(check(&ctx, &elsewhere, 1_000_000, &nonces), 3);
        let mut forged = ctx.clone();
        forged["channel"] = json!("personal");
        assert_eq!(check(&forged, &args, 1_000_000, &nonces), 4);
        assert_eq!(check(&ctx, &args, 1_036_000, &nonces), 5);
        verify_ui(&ctx, &args, &env(), 1_000_000, &nonces).unwrap();
        assert_eq!(check(&ctx, &args, 1_000_001, &nonces), 6);
        let agent = context(
            &args,
            1_000_000,
            "ffeeddccbbaa99887766554433221100",
            "agent",
        );
        assert_eq!(check(&agent, &args, 1_000_000, &nonces), 7);
    }

    #[test]
    fn a_missing_or_malformed_node_key_is_refused_at_start() {
        assert!(ContractEnv::new("t", "").is_err());
        assert!(ContractEnv::new("t", "zz").is_err());
        assert!(ContractEnv::new("", &hex::encode([1u8; 32])).is_err());
    }

    #[test]
    fn nonces_are_forgotten_once_their_context_could_not_pass() {
        let nonces = NonceCache::default();
        assert!(nonces.fresh("a", 100, 0));
        assert!(!nonces.fresh("a", 100, 0));
        assert!(nonces.fresh("b", 10_000, 5_101));
        assert!(nonces.fresh("a", 10_000, 5_101));
    }
}
