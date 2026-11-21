//! The tools and the HTTP API are one surface. Both come from the operation
//! table (`ops::OPS`); these tests fail when anything drifts from it: the
//! manifest, `tools/list` in either MCP mode, the routes the server actually
//! answers, a route registered by hand, or a call the SPA makes. Anything on
//! one side only must be in `ops::HTTP_ONLY` or `ops::TOOL_ONLY`, with why.

mod support;

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Value, json};
use support::Plugin;
use workboard::ops::{self, HTTP_ONLY, OPS, TOOL_ONLY};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn routes() -> BTreeSet<(String, String)> {
    OPS.iter()
        .map(|op| (op.method.to_string(), op.path.to_string()))
        .collect()
}

#[test]
fn the_manifest_declares_each_operation_once_with_its_hint() {
    let text = std::fs::read_to_string(Path::new(ROOT).join("tracon-plugin.toml")).unwrap();
    let manifest: toml::Table = toml::from_str(&text).unwrap();
    let declared: Vec<(String, String, bool)> = manifest["tool"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["name"].as_str().unwrap().to_string(),
                t["hint"].as_str().unwrap().to_string(),
                t.get("consequential").and_then(toml::Value::as_bool) == Some(true),
            )
        })
        .collect();
    let listed: Vec<(String, String, bool)> = ops::tools("")
        .iter()
        .map(|t| {
            let read = t["annotations"]["readOnlyHint"] == true;
            let hint = if read { "read" } else { "write" };
            (t["name"].as_str().unwrap().to_string(), hint.into(), !read)
        })
        .collect();
    // Every write is consequential: asked call by call, never allowed by name.
    assert_eq!(declared, listed);
}

#[test]
fn each_operation_has_one_route_and_one_tool() {
    let names: BTreeSet<&str> = OPS.iter().map(|op| op.name).collect();
    assert_eq!(names.len(), OPS.len(), "a tool name is used twice");
    assert_eq!(routes().len(), OPS.len(), "a route is used twice");
    for op in OPS {
        // The path's parameters are arguments the tool takes too.
        let schema = op.input_schema();
        for param in op.path_params() {
            assert!(
                schema["properties"].get(param).is_some(),
                "{}: {param} is in the path but not the tool's arguments",
                op.name
            );
        }
        assert_eq!(op.write, op.method != "GET", "{}: only GET reads", op.name);
        assert!(
            format!("workboard__{}", op.name).len() <= 64,
            "{} is too long",
            op.name
        );
    }
    for (method, path, why) in HTTP_ONLY {
        assert!(!why.is_empty());
        assert!(
            !routes().contains(&(method.to_string(), path.to_string())),
            "{method} {path} is both an operation and HTTP only"
        );
    }
    for (name, why) in TOOL_ONLY {
        assert!(!why.is_empty());
        assert!(ops::find(name).is_none(), "{name} is both");
    }
}

/// Every `.route("<literal>", ...)` in the source: hand-registered routes,
/// which the operation table does not generate.
fn literal_routes() -> Vec<(String, String)> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(Path::new(ROOT).join("src")).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        let mut rest = text.as_str();
        while let Some(at) = rest.find(".route(") {
            rest = &rest[at + ".route(".len()..];
            let trimmed = rest.trim_start();
            if let Some(literal) = trimmed.strip_prefix('"') {
                let end = literal.find('"').unwrap();
                found.push((
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    literal[..end].to_string(),
                ));
            }
        }
    }
    found
}

#[test]
fn no_api_route_is_registered_outside_the_table_unless_listed() {
    let only: Vec<&str> = HTTP_ONLY.iter().map(|(_, path, _)| *path).collect();
    let found = literal_routes();
    assert!(!found.is_empty(), "the scan found no routes at all");
    for (file, route) in found {
        if route.starts_with("/api") {
            assert!(
                only.contains(&route.as_str()),
                "{file} registers {route} by hand: add it to ops::OPS, or to ops::HTTP_ONLY with why"
            );
        } else {
            // The SPA's assets (standalone) and MCP (the plugin socket).
            assert!(
                ["/", "/assets/{*path}"].contains(&route.as_str()),
                "{file} registers {route}"
            );
        }
    }
}

/// Every call in the SPA's API client, as (method, route).
fn spa_calls() -> Vec<(String, String)> {
    let text = std::fs::read_to_string(Path::new(ROOT).join("spa/src/lib/api.ts")).unwrap();
    let mut calls = Vec::new();
    for line in text.lines() {
        let Some(at) = line.find("request<") else {
            continue;
        };
        let call = &line[at..];
        let Some(open) = call.find(['\'', '`']) else {
            continue;
        };
        let quote = call.as_bytes()[open] as char;
        let after = &call[open + 1..];
        let end = after.find(quote).unwrap();
        let mut path = format!("/{}", &after[..end]);
        // `${encodeURIComponent(id)}` is a path parameter.
        while let Some(start) = path.find("${encodeURIComponent(") {
            let stop = path[start..].find(")}").unwrap() + start;
            let name = path[start + "${encodeURIComponent(".len()..stop].to_string();
            path.replace_range(start..stop + 2, &format!("{{{name}}}"));
        }
        let method = call
            .split("method: '")
            .nth(1)
            .and_then(|m| m.split('\'').next())
            .unwrap_or("GET")
            .to_string();
        calls.push((method, path));
    }
    calls
}

#[test]
fn every_call_the_spa_makes_is_an_operation_or_listed() {
    let calls = spa_calls();
    assert!(
        calls.len() >= 10,
        "the scan missed the SPA's calls: {calls:?}"
    );
    for (method, path) in calls {
        let listed = HTTP_ONLY.iter().any(|(m, p, _)| *m == method && *p == path);
        assert!(
            routes().contains(&(method.clone(), path.clone())) || listed,
            "the SPA calls {method} {path}, which is neither an operation nor HTTP only"
        );
    }
}

fn probe_path(path: &str) -> String {
    path.split('/')
        .map(|s| if s.starts_with('{') { "missing" } else { s })
        .collect::<Vec<_>>()
        .join("/")
}

/// Answered by a route, not the fallback: anything but 405 or the API's
/// own "API route not found".
fn routed(status: u16, answer: &Value) -> bool {
    status != 405 && !(status == 404 && answer["message"] == "API route not found.")
}

#[tokio::test]
async fn the_server_answers_every_route_and_mcp_lists_every_tool() {
    let dir = tempfile::tempdir().unwrap();
    let mut plugin = Plugin::start(dir.path(), None).await;
    for op in OPS {
        let body = if op.method == "GET" { "" } else { "{}" };
        let path = probe_path(op.path);
        let (status, answer) = plugin.api(op.method, &path, body).await;
        assert!(
            routed(status, &answer),
            "{} {path}: {status} {answer}",
            op.method
        );
    }
    for (method, path, _) in HTTP_ONLY {
        let (status, answer) = plugin.api(method, path, "{}").await;
        assert!(
            routed(status, &answer),
            "{method} {path}: {status} {answer}"
        );
    }
    // The probe can tell: a route that does not exist is not answered.
    let (status, answer) = plugin.api("GET", "/api/nothing", "").await;
    assert!(!routed(status, &answer));
    let (status, answer) = plugin.api("PATCH", "/api/board", "{}").await;
    assert!(!routed(status, &answer));

    let (_, listed) = plugin.rpc("tools/list", json!({})).await;
    let names: Vec<String> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    let ops: Vec<String> = OPS.iter().map(|op| op.name.to_string()).collect();
    assert_eq!(names, ops, "plugin tools/list");
}

#[test]
fn standalone_mcp_lists_every_tool_plus_its_review_tool() {
    let dir = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(support::BIN)
        .arg("--data-dir")
        .arg(dir.path())
        .arg("mcp")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n")?;
            child.wait_with_output()
        })
        .unwrap();
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<String> = answer["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    let mut expected: Vec<String> = OPS.iter().map(|op| op.name.to_string()).collect();
    expected.extend(TOOL_ONLY.iter().map(|(name, _)| name.to_string()));
    assert_eq!(names, expected);
}

#[tokio::test]
async fn an_http_request_and_a_tool_call_are_the_same_operation() {
    let dir = tempfile::tempdir().unwrap();
    let mut plugin = Plugin::start(dir.path(), None).await;
    let (_, task) = plugin
        .api("POST", "/api/tasks", r#"{"title":"Same"}"#)
        .await;
    let id = task["id"].as_str().unwrap();
    let (status, by_http) = plugin.api("GET", &format!("/api/tasks/{id}"), "").await;
    assert_eq!(status, 200);
    let by_tool = plugin.read("task", json!({ "id": id })).await;
    assert_eq!(by_http, by_tool);
    // The query string is the tool's arguments for a GET.
    let (status, board) = plugin.api("GET", "/api/board?state=inbox", "").await;
    assert_eq!(status, 200);
    let tool_board = plugin.read("board", json!({ "state": "inbox" })).await;
    assert_eq!(board["tasks"], tool_board["tasks"]);
    let (status, answer) = plugin.api("GET", "/api/board?colour=red", "").await;
    assert_eq!((status, &answer["error"]), (400, &json!("invalid_request")));
    // A path parameter cannot be contradicted by the body.
    let (status, answer) = plugin
        .api(
            "PUT",
            &format!("/api/tasks/{id}"),
            r#"{"id":"another","version":1}"#,
        )
        .await;
    assert_eq!(status, 400, "{answer}");
    assert_eq!(answer["message"], "id is given by the path.");
}
