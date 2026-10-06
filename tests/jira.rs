use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Value, json};
use url::Url;
use workboard::jira::{fetch, parse_page};

const SITE: &str = "https://example.atlassian.net";

fn site() -> Url {
    Url::parse(SITE).unwrap()
}

fn issue(id: &str, key: &str, category: &str) -> Value {
    json!({
        "id": id, "key": key, "summary": "A summary", "status": "In Progress",
        "status_category": category, "priority": "Planned", "issue_type": "Task",
        "labels": ["phase-3"],
        "parent": {"key": "ABC-1", "summary": "The epic", "issue_type": "Epic"}
    })
}

fn page(issues: Vec<Value>, next: Option<&str>) -> Value {
    json!({"site": SITE, "account_email": "user@example.com", "issues": issues, "next_cursor": next})
}

#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<(HashMap<String, String>, bool)>>>);

type Reply = fn(Option<&str>) -> Response;

async fn serve(reply: Reply) -> (Url, Seen) {
    let seen = Seen::default();
    let app = Router::new()
        .route(
            "/api/jira/search",
            get(
                |State((seen, reply)): State<(Seen, Reply)>,
                 headers: HeaderMap,
                 Query(query): Query<HashMap<String, String>>| async move {
                    let cursor = query.get("cursor").cloned();
                    seen.0
                        .lock()
                        .unwrap()
                        .push((query, headers.contains_key(header::ORIGIN)));
                    reply(cursor.as_deref())
                },
            ),
        )
        .with_state((seen.clone(), reply));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, seen)
}

#[tokio::test]
async fn follows_cursors_and_maps_labels_and_parent() {
    let (tracon, seen) = serve(|cursor| match cursor {
        None => Json(page(vec![issue("1", "ABC-2", "new")], Some("next-page"))).into_response(),
        Some("next-page") => Json(page(vec![issue("2", "ABC-3", "done")], None)).into_response(),
        Some(_) => StatusCode::BAD_REQUEST.into_response(),
    })
    .await;
    let (identity, issues) = fetch(&tracon, "work", &site(), None).await.unwrap();
    assert_eq!(identity.site, "example.atlassian.net");
    assert_eq!(identity.account_email, "user@example.com");
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].labels, vec!["phase-3"]);
    let parent = issues[0].parent.as_ref().unwrap();
    assert_eq!(parent.key, "ABC-1");
    assert_eq!(parent.issue_type.as_deref(), Some("Epic"));
    assert_eq!(issues[1].status_category, "done");
    let seen = seen.0.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].0["channel"], "work");
    assert_eq!(
        seen[0].0["jql"],
        "assignee = currentUser() ORDER BY updated DESC"
    );
    assert!(!seen[0].0.contains_key("cursor"));
    assert_eq!(seen[1].0["cursor"], "next-page");
    assert!(seen.iter().all(|(_, origin)| !origin));
}

#[tokio::test]
async fn bound_identity_change_stops_after_the_first_page() {
    let (tracon, seen) =
        serve(|_| Json(page(vec![issue("1", "ABC-2", "new")], Some("more"))).into_response()).await;
    let err = fetch(&tracon, "work", &site(), Some((SITE, "other@example.com")))
        .await
        .unwrap_err();
    assert_eq!(err.code, "jira_identity_changed");
    assert_eq!(seen.0.lock().unwrap().len(), 1);
    let other_site = Url::parse("https://other.atlassian.net").unwrap();
    let err = fetch(&tracon, "work", &other_site, None).await.unwrap_err();
    assert_eq!(err.code, "jira_identity_changed");
}

#[tokio::test]
async fn older_tracon_serving_its_web_page_is_unsupported() {
    let (tracon, _) = serve(|_| {
        (
            [(header::CONTENT_TYPE, "text/html")],
            "<!doctype html><div id=app></div>",
        )
            .into_response()
    })
    .await;
    let err = fetch(&tracon, "work", &site(), None).await.unwrap_err();
    assert_eq!(err.code, "tracon_unsupported");
}

#[tokio::test]
async fn tracon_refusals_and_absence_are_reported() {
    let (tracon, _) = serve(|_| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": {"code": 422, "message": "credential jira is not bound to channel work"}})),
        )
            .into_response()
    })
    .await;
    let err = fetch(&tracon, "work", &site(), None).await.unwrap_err();
    assert_eq!(err.code, "jira_unavailable");
    assert!(err.message.contains("not bound to channel work"));

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    drop(listener);
    let err = fetch(&closed, "work", &site(), None).await.unwrap_err();
    assert_eq!(err.code, "tracon_unavailable");
}

#[tokio::test]
async fn repeating_cursor_and_duplicate_issues_are_rejected() {
    let (tracon, _) =
        serve(|_| Json(page(vec![issue("1", "ABC-2", "new")], Some("same"))).into_response()).await;
    let err = fetch(&tracon, "work", &site(), None).await.unwrap_err();
    assert_eq!(err.code, "jira_invalid_response");
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
