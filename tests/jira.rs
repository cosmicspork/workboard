//! A refresh's two ways to jira-review's `search`: the node's `tools/call`
//! on the plugin-to-node socket, and a `jira-review mcp` child.

mod support;

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};
use support::{
    NodeReply, SITE, fake_jira_review, fake_node, issue, logged, page, page_answer, two_pages,
};
use url::Url;
use workboard::jira::{NODE_TOOL, Source, fetch, parse_page, search_arguments};

const JQL: &str = "assignee = currentUser() ORDER BY updated DESC";

fn site() -> Url {
    Url::parse(SITE).unwrap()
}

fn node(socket: &Path, profile: Option<&str>) -> Source {
    Source::Node {
        socket: Some(socket.to_path_buf()),
        token: "node-token".into(),
        profile: profile.map(str::to_owned),
    }
}

fn reply(f: impl Fn(&Value) -> (u16, Value) + Send + Sync + 'static) -> NodeReply {
    Arc::new(f)
}

#[test]
fn search_arguments_name_profile_only_when_set() {
    assert_eq!(
        search_arguments(None, None),
        json!({ "jql": JQL, "page": true })
    );
    assert_eq!(
        search_arguments(Some("c"), Some("work")),
        json!({ "jql": JQL, "page": true, "cursor": "c", "profile": "work" })
    );
}

#[tokio::test]
async fn node_calls_follow_cursors_with_the_token_and_no_profile() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("node.sock");
    let seen = fake_node(&socket, two_pages()).await;
    let (identity, issues) = fetch(&node(&socket, None), &site(), None).await.unwrap();
    assert_eq!(identity.site, "example.atlassian.net");
    assert_eq!(identity.account_email, "user@example.com");
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].labels, vec!["phase-3"]);
    let parent = issues[0].parent.as_ref().unwrap();
    assert_eq!(parent.key, "ABC-1");
    assert_eq!(parent.issue_type.as_deref(), Some("Epic"));
    assert_eq!(issues[1].status_category, "done");

    let requests = seen.requests();
    assert_eq!(requests.len(), 2);
    for (bearer, request) in &requests {
        assert_eq!(bearer.as_deref(), Some("node-token"));
        assert_eq!(request["jsonrpc"], "2.0");
        assert_eq!(request["method"], "tools/call");
        assert_eq!(request["params"]["name"], NODE_TOOL);
        // Only the keys the node's tools/call takes.
        let mut keys: Vec<&String> = request["params"].as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["arguments", "name"]);
    }
    assert_ne!(requests[0].1["id"], requests[1].1["id"]);
    assert_eq!(
        requests[0].1["params"]["arguments"],
        json!({ "jql": JQL, "page": true })
    );
    assert_eq!(
        requests[1].1["params"]["arguments"],
        json!({ "jql": JQL, "page": true, "cursor": "next-page" })
    );
}

#[tokio::test]
async fn node_calls_pass_the_profile_and_read_structured_content() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("node.sock");
    let seen = fake_node(
        &socket,
        reply(|request| {
            let body = page(vec![issue("1", "ABC-2", "new")], None);
            (
                200,
                json!({ "jsonrpc": "2.0", "id": request["id"],
                        "result": { "content": [{ "type": "text", "text": "a page" }],
                                    "structuredContent": body } }),
            )
        }),
    )
    .await;
    let (_, issues) = fetch(&node(&socket, Some("client-x")), &site(), None)
        .await
        .unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(
        seen.requests()[0].1["params"]["arguments"],
        json!({ "jql": JQL, "page": true, "profile": "client-x" })
    );
}

#[tokio::test]
async fn node_refusals_and_failures_are_reported() {
    let cases: Vec<(Value, u16, &str, &str)> = vec![
        (
            json!({ "code": -32003, "data": { "decision": "asked" },
                    "message": "jira-review__search would be put to the operator, and a call from plugin workboard is never held: add allow = [\"search\"]" }),
            200,
            "jira_refused",
            "allow = [\"search\"]",
        ),
        (
            json!({ "code": -32003, "message": "plugin jira-review is not offered on channel work" }),
            200,
            "jira_refused",
            "not offered",
        ),
        (
            json!({ "code": -32602, "message": "plugin jira-review is not installed on this node" }),
            200,
            "jira_review_missing",
            "tracon plugin add cosmicspork/jira-review",
        ),
        (
            json!({ "code": -32601, "message": "tools/call is not in workboard's node_methods" }),
            200,
            "tracon_unsupported",
            "node_methods",
        ),
        (
            json!({ "code": -32000, "data": { "tool_error": true },
                    "message": "plugin jira-review reports an error from search: credential jira was not lent to this call" }),
            200,
            "jira_unavailable",
            "was not lent",
        ),
        (
            json!({ "code": -32001, "message": "wrong token" }),
            401,
            "tracon_forbidden",
            "token",
        ),
    ];
    for (rpc_error, status, code, says) in cases {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("node.sock");
        let answer = rpc_error.clone();
        fake_node(
            &socket,
            reply(move |request| {
                (
                    status,
                    json!({ "jsonrpc": "2.0", "id": request["id"], "error": answer }),
                )
            }),
        )
        .await;
        let err = fetch(&node(&socket, None), &site(), None)
            .await
            .unwrap_err();
        assert_eq!(err.code, code, "{rpc_error}");
        assert!(err.message.contains(says), "{rpc_error}: {}", err.message);
    }

    // jira-review's own error result.
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("node.sock");
    fake_node(
        &socket,
        reply(|request| {
            (
                200,
                json!({ "jsonrpc": "2.0", "id": request["id"],
                        "result": { "content": [{ "type": "text", "text": "Jira said 401" }],
                                    "isError": true } }),
            )
        }),
    )
    .await;
    let err = fetch(&node(&socket, None), &site(), None)
        .await
        .unwrap_err();
    assert_eq!(err.code, "jira_unavailable");
    assert!(err.message.contains("Jira said 401"), "{}", err.message);

    // No socket answering, and no socket given.
    let gone = dir.path().join("gone.sock");
    let err = fetch(&node(&gone, None), &site(), None).await.unwrap_err();
    assert_eq!(err.code, "tracon_unavailable");
    let none = Source::Node {
        socket: None,
        token: "t".into(),
        profile: None,
    };
    let err = fetch(&none, &site(), None).await.unwrap_err();
    assert_eq!(err.code, "tracon_unsupported");
}

#[tokio::test]
async fn bound_identity_change_stops_after_the_first_page() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("node.sock");
    let seen = fake_node(
        &socket,
        reply(|request| {
            let body = page(vec![issue("1", "ABC-2", "new")], Some("more"));
            (200, page_answer(&request["id"], &body))
        }),
    )
    .await;
    let err = fetch(
        &node(&socket, None),
        &site(),
        Some((SITE, "other@example.com")),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "jira_identity_changed");
    assert_eq!(seen.requests().len(), 1);
    let other_site = Url::parse("https://other.atlassian.net").unwrap();
    let err = fetch(&node(&socket, None), &other_site, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, "jira_identity_changed");
}

#[tokio::test]
async fn repeating_cursor_and_unreadable_pages_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("node.sock");
    fake_node(
        &socket,
        reply(|request| {
            let body = page(vec![issue("1", "ABC-2", "new")], Some("same"));
            (200, page_answer(&request["id"], &body))
        }),
    )
    .await;
    let err = fetch(&node(&socket, None), &site(), None)
        .await
        .unwrap_err();
    assert_eq!(err.code, "jira_invalid_response");

    let socket = dir.path().join("text.sock");
    fake_node(
        &socket,
        reply(|request| {
            (
                200,
                json!({ "jsonrpc": "2.0", "id": request["id"],
                        "result": { "content": [{ "type": "text", "text": "not a page" }] } }),
            )
        }),
    )
    .await;
    let err = fetch(&node(&socket, None), &site(), None)
        .await
        .unwrap_err();
    assert_eq!(err.code, "jira_invalid_response");
}

#[tokio::test]
async fn a_child_is_started_as_mcp_and_paged_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let (program, log) = fake_jira_review(dir.path(), "pages");
    let source = Source::Child {
        program: program.clone(),
        profile: None,
    };
    let (identity, issues) = fetch(&source, &site(), None).await.unwrap();
    assert_eq!(identity.account_email, "user@example.com");
    assert_eq!(issues.len(), 2);
    let log = logged(&log);
    assert_eq!(log[0]["argv"], json!(["mcp"]));
    assert_eq!(log[1]["method"], "initialize");
    assert_eq!(log[1]["params"]["protocolVersion"], "2025-06-18");
    assert_eq!(log[2]["method"], "notifications/initialized");
    assert!(log[2].get("id").is_none());
    let calls: Vec<&Value> = log.iter().filter(|m| m["method"] == "tools/call").collect();
    assert_eq!(calls.len(), 2);
    for call in &calls {
        assert_eq!(call["params"]["name"], "search");
    }
    // No profile argument: standalone, the profile is a flag.
    assert_eq!(
        calls[0]["params"]["arguments"],
        json!({ "jql": JQL, "page": true })
    );
    assert_eq!(
        calls[1]["params"]["arguments"],
        json!({ "jql": JQL, "page": true, "cursor": "next-page" })
    );

    let dir = tempfile::tempdir().unwrap();
    let (program, log) = fake_jira_review(dir.path(), "pages");
    let source = Source::Child {
        program,
        profile: Some("client-x".into()),
    };
    fetch(&source, &site(), None).await.unwrap();
    assert_eq!(
        logged(&log)[0]["argv"],
        json!(["--profile", "client-x", "mcp"])
    );
}

#[tokio::test]
async fn a_child_that_fails_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let (program, _) = fake_jira_review(dir.path(), "tool_error");
    let source = Source::Child {
        program,
        profile: None,
    };
    let err = fetch(&source, &site(), None).await.unwrap_err();
    assert_eq!(err.code, "jira_unavailable");
    assert!(
        err.message.contains("no Jira credential"),
        "{}",
        err.message
    );

    let (program, _) = fake_jira_review(dir.path(), "exit");
    let source = Source::Child {
        program,
        profile: None,
    };
    let err = fetch(&source, &site(), None).await.unwrap_err();
    assert_eq!(err.code, "jira_unavailable");
    assert!(err.message.contains("--bogus"), "{}", err.message);

    let source = Source::Child {
        program: dir.path().join("no-such-jira-review"),
        profile: None,
    };
    let err = fetch(&source, &site(), None).await.unwrap_err();
    assert_eq!(err.code, "jira_review_missing");
    assert!(err.message.contains("--jira-review"), "{}", err.message);
}

#[test]
fn page_parsing_rejects_incomplete_or_unsafe_data() {
    let parsed = parse_page(
        page(vec![issue("1", "ABC-2", "new")], None)
            .to_string()
            .as_bytes(),
    )
    .unwrap();
    assert_eq!(parsed.issues[0].priority.as_deref(), Some("Planned"));
    assert_eq!(parsed.next_cursor, None);
    let mut no_parent = issue("1", "ABC-2", "new");
    no_parent["parent"] = Value::Null;
    no_parent.as_object_mut().unwrap().remove("labels");
    let parsed = parse_page(page(vec![no_parent], None).to_string().as_bytes()).unwrap();
    assert!(parsed.issues[0].parent.is_none());
    assert!(parsed.issues[0].labels.is_empty());

    let mut bad_category = issue("1", "ABC-2", "closed");
    bad_category["status_category"] = json!("closed");
    let mut missing_key = issue("1", "ABC-2", "new");
    missing_key.as_object_mut().unwrap().remove("key");
    let mut bad_labels = issue("1", "ABC-2", "new");
    bad_labels["labels"] = json!("phase-3");
    let mut keyless_parent = issue("1", "ABC-2", "new");
    keyless_parent["parent"] = json!({"summary": "no key"});
    let mut plain_http = page(vec![], None);
    plain_http["site"] = json!("http://example.atlassian.net");
    let mut no_account = page(vec![], None);
    no_account["account_email"] = json!("");
    for bad in [
        page(vec![bad_category], None),
        page(vec![missing_key], None),
        page(vec![bad_labels], None),
        page(vec![keyless_parent], None),
        plain_http,
        no_account,
        json!({"issues": "nope"}),
    ] {
        assert_eq!(
            parse_page(bad.to_string().as_bytes()).unwrap_err().code,
            "jira_invalid_response",
            "{bad}"
        );
    }
    assert_eq!(
        parse_page(b"{bad json").unwrap_err().code,
        "jira_invalid_response"
    );
}
