use crate::model::{Identity, ParentIssue, SourceIssue};
use serde_json::Value;
use std::{collections::HashSet, time::Duration};
use tokio::time::Instant;
use url::Url;

const DEADLINE: Duration = Duration::from_secs(120);
const PAGE_LIMIT: usize = 8 * 1024 * 1024;
const MAX_PAGES: usize = 100;
const JQL: &str = "assignee = currentUser() ORDER BY updated DESC";

#[derive(Debug, Clone)]
pub struct JiraError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for JiraError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for JiraError {}

fn error(code: &'static str, message: impl Into<String>) -> JiraError {
    JiraError {
        code,
        message: message.into(),
    }
}

fn invalid(message: &str) -> JiraError {
    error("jira_invalid_response", message)
}

#[derive(Debug)]
pub struct Page {
    pub identity: Identity,
    pub issues: Vec<SourceIssue>,
    pub next_cursor: Option<String>,
}

pub async fn fetch(
    tracon: &Url,
    channel: &str,
    site: &Url,
    bound: Option<(&str, &str)>,
) -> Result<(Identity, Vec<SourceIssue>), JiraError> {
    let Some(configured_host) = site
        .host_str()
        .filter(|_| site.scheme() == "https")
        .map(|h| h.trim_end_matches('.').to_ascii_lowercase())
    else {
        return Err(invalid("Configured Jira site is invalid."));
    };
    let endpoint = tracon
        .join("api/jira/search")
        .map_err(|_| error("tracon_unavailable", "Configured Tracon URL is invalid."))?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| error("tracon_unavailable", "Could not prepare the Tracon client."))?;
    let deadline = Instant::now() + DEADLINE;
    let mut identity: Option<Identity> = None;
    let mut issues = Vec::new();
    let mut cursors = HashSet::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut query = vec![("channel", channel), ("jql", JQL)];
        if let Some(cursor) = cursor.as_deref() {
            query.push(("cursor", cursor));
        }
        let request = client.get(endpoint.clone()).query(&query).send();
        let bytes = tokio::time::timeout_at(deadline, async {
            let response = request.await.map_err(|_| {
                error(
                    "tracon_unavailable",
                    format!("Tracon is not reachable at {tracon}. Start the Tracon node, then refresh again."),
                )
            })?;
            read_page(response).await
        })
        .await
        .map_err(|_| error("jira_timeout", "Jira refresh timed out."))??;
        let page = parse_page(&bytes)?;
        match &identity {
            None => {
                check_identity(&page.identity, &configured_host, bound)?;
                identity = Some(page.identity);
            }
            Some(first) if *first != page.identity => {
                return Err(error(
                    "jira_identity_changed",
                    "The Jira account changed during the refresh.",
                ));
            }
            Some(_) => {}
        }
        issues.extend(page.issues);
        match page.next_cursor {
            None => {
                check_unique(&issues)?;
                return Ok((identity.unwrap(), issues));
            }
            Some(next) => {
                if !cursors.insert(next.clone()) {
                    return Err(invalid("Jira returned a repeating page cursor."));
                }
                cursor = Some(next);
            }
        }
    }
    Err(invalid("Jira returned more pages than Workboard accepts."))
}

async fn read_page(mut response: reqwest::Response) -> Result<Vec<u8>, JiraError> {
    let status = response.status();
    let json = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().starts_with("application/json"));
    // Tracon answers unknown routes with its web interface, so a non-JSON reply means an older node.
    if !json || status == reqwest::StatusCode::NOT_FOUND {
        return Err(error(
            "tracon_unsupported",
            "This Tracon node does not offer Jira search yet. Update Tracon, then refresh again.",
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| error("tracon_unavailable", "Tracon closed the connection."))?
    {
        if body.len() + chunk.len() > PAGE_LIMIT {
            return Err(invalid("Jira response exceeded its size limit."));
        }
        body.extend_from_slice(&chunk);
    }
    if status.is_success() {
        return Ok(body);
    }
    let detail = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or_default();
    let detail: String = detail.chars().take(300).collect();
    Err(match status.as_u16() {
        401 | 403 => error(
            "tracon_forbidden",
            "Tracon refused the request. Run Workboard on the same machine as the Tracon node.",
        ),
        422 => error(
            "jira_unavailable",
            format!("Tracon could not use its Jira credential: {detail}"),
        ),
        _ => error(
            "jira_unavailable",
            format!("Jira search through Tracon failed: {detail}"),
        ),
    })
}

fn check_identity(
    identity: &Identity,
    configured_host: &str,
    bound: Option<(&str, &str)>,
) -> Result<(), JiraError> {
    if identity.site != configured_host {
        return Err(error(
            "jira_identity_changed",
            "Tracon's Jira site does not match the configured site.",
        ));
    }
    if let Some((bound_site, bound_email)) = bound {
        let bound_host = Url::parse(bound_site)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_else(|| bound_site.to_owned());
        if !bound_host
            .trim_end_matches('.')
            .eq_ignore_ascii_case(configured_host)
            || !bound_email.eq_ignore_ascii_case(&identity.account_email)
        {
            return Err(error(
                "jira_identity_changed",
                "Tracon's Jira identity differs from the identity this board is bound to.",
            ));
        }
    }
    Ok(())
}

fn check_unique(issues: &[SourceIssue]) -> Result<(), JiraError> {
    let mut ids = HashSet::with_capacity(issues.len());
    let mut keys = HashSet::with_capacity(issues.len());
    if issues
        .iter()
        .all(|i| ids.insert(i.id.as_str()) && keys.insert(i.key.as_str()))
    {
        Ok(())
    } else {
        Err(invalid("Jira returned invalid or duplicate issue data."))
    }
}

pub fn parse_page(bytes: &[u8]) -> Result<Page, JiraError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| invalid("Jira returned an invalid response."))?;
    let site = field(value.get("site"))
        .and_then(|s| Url::parse(s).ok())
        .filter(|u| {
            u.scheme() == "https"
                && u.username().is_empty()
                && u.password().is_none()
                && u.port().is_none()
        })
        .and_then(|u| {
            u.host_str()
                .map(|h| h.trim_end_matches('.').to_ascii_lowercase())
        })
        .ok_or_else(|| invalid("Tracon returned an invalid Jira site."))?;
    let account_email = field(value.get("account_email"))
        .ok_or_else(|| invalid("Tracon returned no Jira account."))?
        .to_owned();
    let next_cursor = match value.get("next_cursor") {
        None | Some(Value::Null) => None,
        Some(cursor) => Some(
            field(Some(cursor))
                .ok_or_else(|| invalid("Jira returned an invalid page cursor."))?
                .to_owned(),
        ),
    };
    let rows = value
        .get("issues")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("Jira returned an invalid response."))?;
    let issues = rows
        .iter()
        .map(parse_issue)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Page {
        identity: Identity {
            site,
            account_email,
        },
        issues,
        next_cursor,
    })
}

fn parse_issue(value: &Value) -> Result<SourceIssue, JiraError> {
    let incomplete = || invalid("Jira returned incomplete issue data.");
    let text = |name: &str| field(value.get(name)).map(str::to_owned);
    let (Some(id), Some(key), Some(summary), Some(status_name), Some(status_category)) = (
        text("id"),
        text("key"),
        text("summary"),
        text("status"),
        text("status_category"),
    ) else {
        return Err(incomplete());
    };
    if !["new", "indeterminate", "done"].contains(&status_category.as_str()) {
        return Err(invalid("Jira returned an unsupported status category."));
    }
    let labels = match value.get("labels") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(labels)) => labels
            .iter()
            .map(|l| field(Some(l)).map(str::to_owned).ok_or_else(incomplete))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err(incomplete()),
    };
    let parent = match value.get("parent") {
        None | Some(Value::Null) => None,
        Some(parent) => Some(ParentIssue {
            key: field(parent.get("key")).ok_or_else(incomplete)?.to_owned(),
            summary: field(parent.get("summary")).map(str::to_owned),
            issue_type: field(parent.get("issue_type")).map(str::to_owned),
        }),
    };
    Ok(SourceIssue {
        id,
        key,
        summary,
        status_name,
        status_category,
        priority: text("priority"),
        issue_type: text("issue_type"),
        labels,
        parent,
    })
}

fn field(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}
