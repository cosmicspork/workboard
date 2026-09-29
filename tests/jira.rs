use std::{fs, os::unix::fs::PermissionsExt};
use url::Url;
use workboard::jira::{fetch, parse_identity, parse_issues};

const SITE: &str = "https://example.atlassian.net";
fn site() -> Url {
    Url::parse(SITE).unwrap()
}
fn issue(id: &str, key: &str, category: &str, status: &str) -> String {
    serde_json::json!({"id":id,"key":key,"self":format!("{SITE}/rest/api/3/issue/{id}"),"fields":{"summary":"A summary","status":{"name":status,"statusCategory":{"key":category}},"priority":{"name":"Planned"},"issuetype":{"name":"Task"}}}).to_string()
}

#[test]
fn parses_anchored_auth_and_rejects_missing_identity() {
    let identity = parse_identity(b"\xe2\x9c\x93 Authenticated\n  Site: example.atlassian.net\n  Email: user@example.com\n  Authentication Type: api_token\n").unwrap();
    assert_eq!(identity.site, "example.atlassian.net");
    assert_eq!(identity.account_email, "user@example.com");
    assert_eq!(
        parse_identity(b"Site: https://example.atlassian.net/\nEmail: user@example.com\n").unwrap(),
        identity
    );
    assert_eq!(
        parse_identity(b"Not Site: https://example.atlassian.net\nEmail: user@example.com")
            .unwrap_err()
            .code,
        "jira_auth_required"
    );
}

#[test]
fn validates_full_issue_array_categories_self_and_uniqueness() {
    let rows = format!(
        "[{},{}]",
        issue("1", "ABC-1", "done", "Cancelled"),
        issue("2", "ABC-2", "indeterminate", "In Progress")
    );
    let parsed = parse_issues(rows.as_bytes(), &site()).unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].status_category, "done");
    assert_eq!(parsed[1].status_name, "In Progress");
    let backend = issue("3", "ABC-3", "new", "Open")
        .replace(SITE, "https://jira-prod-us-28-2.prod.atl-paas.net");
    assert_eq!(
        parse_issues(format!("[{backend}]").as_bytes(), &site())
            .unwrap()
            .len(),
        1
    );
    for bad in [
        format!(
            "[{},{}]",
            issue("1", "ABC-1", "new", "Open"),
            issue("1", "ABC-2", "new", "Open")
        ),
        format!(
            "[{},{}]",
            issue("1", "ABC-1", "new", "Open"),
            issue("2", "ABC-1", "new", "Open")
        ),
        issue("1", "ABC-1", "closed", "Closed"),
        issue("1", "ABC-1", "new", "Open").replace(SITE, "https://evil.example"),
        issue("1", "ABC-1", "new", "Open")
            .replace(SITE, "https://fake.prod.atl-paas.net.evil.example"),
        issue("1", "ABC-1", "new", "Open")
            .replace(SITE, "http://jira-prod-us-28-2.prod.atl-paas.net"),
        "{bad json}".to_owned(),
    ] {
        assert_eq!(
            parse_issues(bad.as_bytes(), &site()).unwrap_err().code,
            "jira_invalid_response"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn fake_cli_fetches_auth_then_search_and_blocks_bound_identity_change() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("fake-acli");
    let marker = dir.path().join("search-called");
    let contents = format!(
        "#!/bin/sh\nif [ \"$3\" = status ]; then printf 'Site: {SITE}\\nEmail: user@example.com\\n'; exit 0; fi\ntouch '{}'\nprintf '%s' '[{}]'\n",
        marker.display(),
        issue("7", "ABC-7", "new", "Open")
    );
    fs::write(&script, contents).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let (identity, rows) = fetch(script.to_str().unwrap(), &site(), None)
        .await
        .unwrap();
    assert_eq!(identity.account_email, "user@example.com");
    assert_eq!(rows.len(), 1);
    assert!(marker.exists());
    fs::remove_file(&marker).unwrap();
    let err = fetch(
        script.to_str().unwrap(),
        &site(),
        Some((SITE, "other@example.com")),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "jira_identity_changed");
    assert!(!marker.exists());
}
