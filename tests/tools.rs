//! The board's tools in plugin mode, as a node calls them: reads answer an
//! agent directly; a write runs only from an approved card, against the
//! preview that card showed, and is refused as stale when the board moved.

mod support;

use serde_json::{Value, json};
use support::Plugin;

fn result(answer: &Value) -> Value {
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    serde_json::from_str(answer["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

/// A manual task, created through the operator's page.
async fn seed(plugin: &Plugin, title: &str) -> Value {
    let (status, task) = plugin
        .api(
            "POST",
            "/api/tasks",
            &json!({ "title": title, "notes": "first line" }).to_string(),
        )
        .await;
    assert_eq!(status, 201, "{task}");
    task
}

fn update_args(task: &Value, state: &str, notes: &str) -> Value {
    json!({
        "id": task["id"], "version": task["version"],
        "title_override": task["title_override"], "next_action": "Call Sam",
        "notes": notes, "state": state, "today": false,
        "waiting_for": null, "review_on": null, "links": [],
        "project_id": null, "tags": ["person:sam"],
    })
}

#[tokio::test]
async fn reads_answer_an_agent_directly() {
    let dir = tempfile::tempdir().unwrap();
    let mut plugin = Plugin::start(dir.path(), None).await;
    let task = seed(&plugin, "Draft the plan").await;
    let board = plugin.read("board", json!({})).await;
    assert_eq!(board["tasks"][0]["id"], task["id"]);
    let one = plugin.read("task", json!({ "id": task["id"] })).await;
    assert_eq!(one["title"], "Draft the plan");
    assert_eq!(plugin.read("projects", json!({})).await, json!([]));
    let filtered = plugin.read("board", json!({ "state": "next" })).await;
    assert_eq!(filtered["tasks"], json!([]));
    let inbox = plugin
        .read("board", json!({ "state": "inbox", "project": "none" }))
        .await;
    assert_eq!(inbox["tasks"].as_array().unwrap().len(), 1);
    // A read's preview says it changes nothing.
    let preview = plugin
        .tool_with("tracon/preview", "board", json!({}), None, None)
        .await;
    assert_eq!(preview["result"]["previewable"], false, "{preview}");
}

#[tokio::test]
async fn a_write_without_an_approval_is_refused_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut plugin = Plugin::start(dir.path(), None).await;
    let answer = plugin
        .tool_with(
            "tools/call",
            "task_create",
            json!({ "title": "Sneaky" }),
            None,
            None,
        )
        .await;
    assert_eq!(answer["error"]["code"], -32001, "{answer}");
    assert_eq!(answer["error"]["data"]["check"], 7);
    assert_eq!(plugin.task_count().await, 0);

    // A context signed for other arguments is refused too.
    let context = workboard::contract::sign(
        &support::unsigned_call("task_create", &json!({ "title": "A" }), Some("ap-1"), None),
        &support::key(),
    );
    let (_, answer) = plugin
        .rpc(
            "tools/call",
            json!({ "name": "task_create", "arguments": { "title": "B" },
                    "_meta": { "tracon": context } }),
        )
        .await;
    assert_eq!(answer["error"]["data"]["check"], 3, "{answer}");
    assert_eq!(plugin.task_count().await, 0);
}

#[tokio::test]
async fn an_approved_update_applies_against_its_preview() {
    let dir = tempfile::tempdir().unwrap();
    let mut plugin = Plugin::start(dir.path(), None).await;
    let task = seed(&plugin, "Draft the plan").await;
    let args = update_args(&task, "next", "first line\nsecond line");

    let preview = plugin
        .tool_with("tracon/preview", "task_update", args.clone(), None, None)
        .await;
    let shown = &preview["result"];
    assert_eq!(shown["previewable"], true, "{preview}");
    assert_eq!(shown["summary"], "update task “Draft the plan”");
    let body = shown["body"].as_str().unwrap();
    assert!(body.contains("| State | inbox | next |"), "{body}");
    assert!(body.contains("| Tags | (none) | person:sam |"), "{body}");
    assert!(body.contains("+second line"), "{body}");
    assert!(body.contains("+Call Sam"), "{body}");
    let token = shown["token"].as_str().unwrap().to_string();
    // A preview writes nothing.
    let unchanged = plugin.read("task", json!({ "id": task["id"] })).await;
    assert_eq!(unchanged["version"], task["version"]);

    let answer = plugin
        .tool_with(
            "tools/call",
            "task_update",
            args,
            Some("ap-1"),
            Some(&token),
        )
        .await;
    let updated = result(&answer);
    assert_eq!(updated["state"], "next");
    assert_eq!(updated["version"], 2);
    assert_eq!(answer["result"]["structuredContent"]["state"], "next");
}

#[tokio::test]
async fn an_approved_write_on_a_moved_board_is_stale_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut plugin = Plugin::start(dir.path(), None).await;
    let task = seed(&plugin, "Draft the plan").await;
    let args = update_args(&task, "later", "agent notes");
    let preview = plugin
        .tool_with("tracon/preview", "task_update", args.clone(), None, None)
        .await;
    let token = preview["result"]["token"].as_str().unwrap().to_string();

    // The operator edits the task on the page after the card was shown.
    let mut mine = update_args(&task, "waiting", "operator notes");
    mine.as_object_mut().unwrap().remove("id");
    let id = task["id"].as_str().unwrap();
    let (status, _) = plugin
        .api("PUT", &format!("/api/tasks/{id}"), &mine.to_string())
        .await;
    assert_eq!(status, 200);

    let answer = plugin
        .tool_with(
            "tools/call",
            "task_update",
            args.clone(),
            Some("ap-1"),
            Some(&token),
        )
        .await;
    assert_eq!(answer["error"]["code"], -32002, "{answer}");
    assert_eq!(answer["error"]["data"], json!({ "stale": true }));
    // Without a preview token, the stale version refuses it the same way.
    let answer = plugin
        .tool_with(
            "tools/call",
            "task_update",
            args.clone(),
            Some("ap-2"),
            None,
        )
        .await;
    assert_eq!(
        answer["error"]["data"],
        json!({ "stale": true }),
        "{answer}"
    );
    let now = plugin.read("task", json!({ "id": id })).await;
    assert_eq!(now["notes"], "operator notes");
    assert_eq!(now["state"], "waiting");

    // And a fresh card for the stale call cannot be shown.
    let preview = plugin
        .tool_with("tracon/preview", "task_update", args, None, None)
        .await;
    assert_eq!(preview["error"]["code"], -32003, "{preview}");
    assert!(
        preview["error"]["message"]
            .as_str()
            .unwrap()
            .contains("version 2"),
        "{preview}"
    );
}

#[tokio::test]
async fn every_write_previews_and_applies() {
    let dir = tempfile::tempdir().unwrap();
    let mut plugin = Plugin::start(dir.path(), None).await;
    let mut approval = 0;
    let mut apply = async |plugin: &mut Plugin, tool: &str, args: Value| -> (Value, Value) {
        let preview = plugin
            .tool_with("tracon/preview", tool, args.clone(), None, None)
            .await;
        assert_eq!(preview["result"]["previewable"], true, "{tool}: {preview}");
        let token = preview["result"]["token"].as_str().map(str::to_string);
        approval += 1;
        let answer = plugin
            .tool_with(
                "tools/call",
                tool,
                args,
                Some(&format!("ap-{approval}")),
                token.as_deref(),
            )
            .await;
        (preview["result"].clone(), result(&answer))
    };

    let (shown, task) = apply(
        &mut plugin,
        "task_create",
        json!({ "title": "New", "notes": "Some **notes**" }),
    )
    .await;
    assert!(shown["body"].as_str().unwrap().contains("Some **notes**"));
    assert_eq!(shown["token"], Value::Null);
    let (_, alpha) = apply(
        &mut plugin,
        "project_create",
        json!({ "name": "Alpha", "jira_keys": ["app"] }),
    )
    .await;
    let (_, beta) = apply(&mut plugin, "project_create", json!({ "name": "Beta" })).await;
    let (shown, assigned) = apply(
        &mut plugin,
        "task_assign_project",
        json!({ "items": [{ "id": task["id"], "version": task["version"] }],
                "project_id": alpha["id"] }),
    )
    .await;
    assert_eq!(shown["summary"], "assign 1 task to project “Alpha”");
    let (shown, triaged) = apply(
        &mut plugin,
        "task_triage",
        json!({ "items": [{ "id": task["id"], "version": assigned[0]["version"] }],
                "state": "later", "review_on": "2030-01-02" }),
    )
    .await;
    assert!(shown["body"].as_str().unwrap().contains("inbox → later"));
    assert_eq!(triaged[0]["review_on"], "2030-01-02");
    let (shown, renamed) = apply(
        &mut plugin,
        "project_update",
        json!({ "id": alpha["id"], "version": alpha["version"], "name": "Alpha 2",
                "target_on": null, "jira_keys": ["APP"] }),
    )
    .await;
    assert!(
        shown["body"]
            .as_str()
            .unwrap()
            .contains("| Name | Alpha | Alpha 2 |")
    );
    assert_eq!(renamed["name"], "Alpha 2");
    let (shown, order) = apply(
        &mut plugin,
        "project_order",
        json!({ "ids": [beta["id"], alpha["id"]] }),
    )
    .await;
    assert!(
        shown["body"]
            .as_str()
            .unwrap()
            .contains("| 1 | Alpha 2 | Beta |")
    );
    assert_eq!(order[0]["name"], "Beta");
    let (shown, deleted) = apply(
        &mut plugin,
        "project_delete",
        json!({ "id": alpha["id"], "version": renamed["version"] }),
    )
    .await;
    assert!(shown["body"].as_str().unwrap().contains("“New”"), "{shown}");
    assert_eq!(deleted["deleted"], alpha["id"]);
    let task = plugin.read("task", json!({ "id": task["id"] })).await;
    assert_eq!(task["project_id"], Value::Null);
}

#[tokio::test]
async fn a_reorder_previewed_before_the_projects_changed_is_stale() {
    let dir = tempfile::tempdir().unwrap();
    let mut plugin = Plugin::start(dir.path(), None).await;
    let mut ids = Vec::new();
    for name in ["A", "B"] {
        let (status, p) = plugin
            .api(
                "POST",
                "/api/projects",
                &json!({ "name": name }).to_string(),
            )
            .await;
        assert_eq!(status, 201);
        ids.push(p["id"].clone());
    }
    let args = json!({ "ids": [ids[1], ids[0]] });
    let preview = plugin
        .tool_with("tracon/preview", "project_order", args.clone(), None, None)
        .await;
    let token = preview["result"]["token"].as_str().unwrap().to_string();
    let (status, _) = plugin
        .api(
            "POST",
            "/api/projects/order",
            &json!({ "ids": [ids[1], ids[0]] }).to_string(),
        )
        .await;
    assert_eq!(status, 200);
    let answer = plugin
        .tool_with(
            "tools/call",
            "project_order",
            args,
            Some("ap-1"),
            Some(&token),
        )
        .await;
    assert_eq!(
        answer["error"]["data"],
        json!({ "stale": true }),
        "{answer}"
    );
}

#[tokio::test]
async fn a_write_the_board_refuses_is_a_tool_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut plugin = Plugin::start(dir.path(), None).await;
    let preview = plugin
        .tool_with(
            "tracon/preview",
            "task_create",
            json!({ "title": " " }),
            None,
            None,
        )
        .await;
    assert_eq!(preview["error"]["code"], -32003, "{preview}");
    let answer = plugin
        .tool_with(
            "tools/call",
            "task_create",
            json!({ "title": " " }),
            Some("ap-1"),
            None,
        )
        .await;
    assert_eq!(answer["result"]["isError"], true, "{answer}");
    let body: Value =
        serde_json::from_str(answer["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(plugin.task_count().await, 0);
}
