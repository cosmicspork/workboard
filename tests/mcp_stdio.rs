//! `workboard mcp` on stdio with no Tracon: reads answer, writes wait as
//! proposals, and `workboard review` applies or rejects them against the
//! board they were previewed on.

mod support;

use std::path::Path;
use std::process::Stdio;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

struct Mcp {
    _child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    next: u64,
}

impl Mcp {
    fn start(data: &Path) -> Self {
        let mut child = tokio::process::Command::new(support::BIN)
            .env_clear()
            .arg("--data-dir")
            .arg(data)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            _child: child,
            stdin,
            lines,
            next: 0,
        }
    }

    async fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.next += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": self.next, "method": "tools/call",
                          "params": { "name": tool, "arguments": arguments } });
        self.stdin
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .unwrap();
        let line = self.lines.next_line().await.unwrap().unwrap();
        let answer: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(answer["result"]["isError"], false, "{answer}");
        serde_json::from_str(answer["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }
}

fn workboard(data: &Path, args: &[&str]) -> (bool, String) {
    let out = std::process::Command::new(support::BIN)
        .env_clear()
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn json_out(data: &Path, args: &[&str]) -> Value {
    let (ok, text) = workboard(data, args);
    assert!(ok, "{args:?}: {text}");
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("{args:?}: {text}"))
}

#[tokio::test]
async fn writes_wait_for_review_and_apply_only_on_an_unmoved_board() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let mut mcp = Mcp::start(data);

    let proposed = mcp
        .call("task_create", json!({ "title": "From an agent" }))
        .await;
    assert_eq!(proposed["status"], "pending");
    assert_eq!(proposed["summary"], "add task “From an agent”");
    let id = proposed["proposal_id"].as_str().unwrap().to_string();
    let board = mcp.call("board", json!({})).await;
    assert_eq!(board["tasks"], json!([]), "a proposal writes nothing");

    let (ok, listed) = workboard(data, &["review"]);
    assert!(ok && listed.contains(&id), "{listed}");
    let approved = json_out(data, &["review", "approve", &id]);
    assert_eq!(approved["status"], "approved", "{approved}");
    let task = approved["result"].clone();
    assert_eq!(task["title"], "From an agent");
    let status = mcp
        .call("proposal_status", json!({ "id": id, "wait_secs": 0 }))
        .await;
    assert_eq!(status["status"], "approved");
    let (ok, again) = workboard(data, &["review", "approve", &id]);
    assert!(!ok && again.contains("already decided"), "{again}");

    // A proposal the board moves under is refused as stale at approval.
    let update = json!({
        "id": task["id"], "version": task["version"], "title_override": "From an agent",
        "next_action": "", "notes": "agent", "state": "next", "today": true,
        "waiting_for": null, "review_on": null, "links": [], "project_id": null, "tags": [],
    });
    let proposed = mcp.call("task_update", update.clone()).await;
    assert!(
        proposed["preview"]
            .as_str()
            .unwrap()
            .contains("| Today | no | yes |"),
        "{proposed}"
    );
    let stale_id = proposed["proposal_id"].as_str().unwrap().to_string();
    let (_, shown) = workboard(data, &["review", "show", &stale_id]);
    assert!(
        shown.contains("as it was when this was proposed"),
        "{shown}"
    );
    let mut mine = update.clone();
    mine["notes"] = json!("mine");
    mine["today"] = json!(false);
    json_out(data, &["op", "task_update", &mine.to_string()]);
    let (_, shown) = workboard(data, &["review", "show", &stale_id]);
    assert!(shown.contains("has moved"), "{shown}");
    let decided = json_out(data, &["review", "approve", &stale_id]);
    assert_eq!(decided["status"], "stale", "{decided}");
    let now = mcp.call("task", json!({ "id": task["id"] })).await;
    assert_eq!(now["notes"], "mine");

    // Rejected, with the reviewer's reason.
    let proposed = mcp.call("project_create", json!({ "name": "Alpha" })).await;
    let reject_id = proposed["proposal_id"].as_str().unwrap().to_string();
    json_out(
        data,
        &["review", "reject", &reject_id, "--reason", "not yet"],
    );
    let status = mcp
        .call(
            "proposal_status",
            json!({ "id": reject_id, "wait_secs": 0 }),
        )
        .await;
    assert_eq!(
        (&status["status"], &status["reason"]),
        (&json!("rejected"), &json!("not yet"))
    );
    assert_eq!(mcp.call("projects", json!({})).await, json!([]));

    // A write the board would refuse is never proposed.
    let refused = {
        mcp.next += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": mcp.next, "method": "tools/call",
                          "params": { "name": "task_create", "arguments": { "title": "" } } });
        mcp.stdin
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .unwrap();
        let line = mcp.lines.next_line().await.unwrap().unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };
    assert_eq!(refused["result"]["isError"], true, "{refused}");
    let (_, listed) = workboard(data, &["review", "list"]);
    assert!(listed.contains("nothing is waiting"), "{listed}");
}

#[test]
fn op_lists_and_runs_the_operations() {
    let dir = tempfile::tempdir().unwrap();
    let (ok, listed) = workboard(dir.path(), &["op"]);
    assert!(ok);
    for op in workboard::ops::OPS {
        assert!(listed.contains(op.name), "{listed}");
    }
    let task = json_out(dir.path(), &["op", "task_create", r#"{"title":"Mine"}"#]);
    assert_eq!(task["state"], "inbox");
    let (ok, err) = workboard(dir.path(), &["op", "task", r#"{"id":"nope"}"#]);
    assert!(!ok && err.contains("not_found"), "{err}");
}
