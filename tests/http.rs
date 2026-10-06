use std::sync::{Arc, atomic::AtomicBool};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use url::Url;
use workboard::{
    http::{self, App},
    store::Store,
};

async fn request(
    port: u16,
    method: &str,
    host: &str,
    origin: Option<&str>,
    path: &str,
    body: &str,
) -> (u16, serde_json::Value) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let origin_header = origin
        .map(|value| format!("Origin: {value}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{origin_header}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let response = std::str::from_utf8(&response).unwrap();
    let (headers, data) = response.split_once("\r\n\r\n").unwrap();
    let status = headers
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (status, serde_json::from_str(data).unwrap())
}

#[tokio::test]
async fn localhost_and_ipv4_are_independent_same_origin_hosts() {
    let dir = tempfile::tempdir().unwrap();
    let site = Url::parse("https://example.atlassian.net").unwrap();
    let store = Arc::new(Store::init(dir.path(), &site).unwrap());
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = App {
        store,
        site,
        tracon: Url::parse("http://127.0.0.1:9").unwrap(),
        jira_channel: "work".into(),
        origin: format!("http://127.0.0.1:{port}"),
        refresh_in_progress: Arc::new(AtomicBool::new(false)),
    };
    let server = tokio::spawn(async move {
        axum::serve(listener, http::router(app)).await.unwrap();
    });
    let localhost = format!("localhost:{port}");
    let ipv4 = format!("127.0.0.1:{port}");
    let local_origin = format!("http://{localhost}");
    let ipv4_origin = format!("http://{ipv4}");
    let (status, board) = request(port, "GET", &localhost, None, "/api/board", "").await;
    assert_eq!(status, 200);
    assert_eq!(board["tasks"].as_array().unwrap().len(), 0);

    let (status, task) = request(
        port,
        "POST",
        &localhost,
        Some(&local_origin),
        "/api/tasks",
        r#"{"title":"Local task"}"#,
    )
    .await;
    assert_eq!(status, 201);
    assert_eq!(task["title"], "Local task");
    let (status, task) = request(
        port,
        "POST",
        &ipv4,
        Some(&ipv4_origin),
        "/api/tasks",
        r#"{"title":"IPv4 task"}"#,
    )
    .await;
    assert_eq!(status, 201);
    assert_eq!(task["title"], "IPv4 task");

    for (host, origin) in [
        (localhost.as_str(), ipv4_origin.as_str()),
        (ipv4.as_str(), local_origin.as_str()),
        ("evil.example:4510", local_origin.as_str()),
    ] {
        let (status, error) = request(
            port,
            "POST",
            host,
            Some(origin),
            "/api/tasks",
            r#"{"title":"Denied"}"#,
        )
        .await;
        assert_eq!(status, 403);
        assert_eq!(error["error"], "forbidden");
    }
    let (status, error) = request(
        port,
        "GET",
        &localhost,
        Some("https://foreign.example"),
        "/api/board",
        "",
    )
    .await;
    assert_eq!(status, 403);
    assert_eq!(error["error"], "forbidden");
    let (status, board) = request(port, "GET", &localhost, None, "/api/board", "").await;
    assert_eq!(status, 200);
    assert_eq!(board["tasks"].as_array().unwrap().len(), 2);

    let (status, project) = request(
        port,
        "POST",
        &localhost,
        Some(&local_origin),
        "/api/projects",
        r#"{"name":"Kubernetes","jira_keys":["app-264"]}"#,
    )
    .await;
    assert_eq!(status, 201);
    assert_eq!(project["jira_keys"][0], "APP-264");
    let project_id = project["id"].as_str().unwrap();
    let (status, _) = request(
        port,
        "POST",
        &localhost,
        None,
        "/api/projects",
        r#"{"name":"No origin"}"#,
    )
    .await;
    assert_eq!(status, 403);
    let items = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| serde_json::json!({"id": t["id"], "version": t["version"]}))
        .collect::<Vec<_>>();
    let (status, assigned) = request(
        port,
        "POST",
        &localhost,
        Some(&local_origin),
        "/api/tasks/project",
        &serde_json::json!({"items": items, "project_id": project_id}).to_string(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(assigned[0]["project_source"], "manual");
    let task = &assigned[0];
    let (status, tagged) = request(
        port,
        "PUT",
        &localhost,
        Some(&local_origin),
        &format!("/api/tasks/{}", task["id"].as_str().unwrap()),
        &serde_json::json!({
            "version": task["version"], "title_override": task["title_override"],
            "next_action": "", "notes": "", "state": "next", "today": false,
            "waiting_for": null, "review_on": null, "links": [],
            "project_id": project_id, "tags": ["person:sam"]
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(tagged["tags"][0], "person:sam");
    let (status, error) = request(
        port,
        "DELETE",
        &localhost,
        Some(&local_origin),
        &format!("/api/projects/{project_id}"),
        r#"{"version":99}"#,
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(error["error"], "task_conflict");
    let (_, board) = request(port, "GET", &localhost, None, "/api/board", "").await;
    assert_eq!(board["projects"][0]["name"], "Kubernetes");
    server.abort();
}
