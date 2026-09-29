use crate::model::{Identity, SourceIssue};
use serde_json::Value;
use std::{collections::HashSet, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    time::Instant,
};
use url::Url;

const DEADLINE: Duration = Duration::from_secs(120);
const STDOUT_LIMIT: usize = 32 * 1024 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;

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

fn error(code: &'static str, message: &'static str) -> JiraError {
    JiraError {
        code,
        message: message.into(),
    }
}

pub async fn fetch(
    acli: &str,
    site: &Url,
    bound: Option<(&str, &str)>,
) -> Result<(Identity, Vec<SourceIssue>), JiraError> {
    if site.scheme() != "https" || site.host_str().is_none() {
        return Err(error(
            "jira_invalid_response",
            "Configured Jira site is invalid.",
        ));
    }
    let deadline = Instant::now() + DEADLINE;
    let auth = run(acli, &["jira", "auth", "status"], false, deadline).await?;
    let identity = parse_identity(&auth)?;
    let configured_host = site
        .host_str()
        .unwrap()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let auth_host = identity.site.trim_end_matches('.').to_ascii_lowercase();
    if auth_host != configured_host {
        return Err(error(
            "jira_identity_changed",
            "The authenticated Jira site does not match the configured site; select the intended CLI account.",
        ));
    }
    if let Some((bound_site, bound_email)) = bound {
        let bound_host = Url::parse(bound_site)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_else(|| bound_site.to_owned());
        if !bound_host
            .trim_end_matches('.')
            .eq_ignore_ascii_case(&configured_host)
            || !bound_email.eq_ignore_ascii_case(&identity.account_email)
        {
            return Err(error(
                "jira_identity_changed",
                "The authenticated Jira identity differs from the saved identity; select the intended CLI account.",
            ));
        }
    }
    let args = [
        "jira",
        "workitem",
        "search",
        "--jql",
        "assignee = currentUser() ORDER BY updated DESC",
        "--fields",
        "key,summary,status,priority,issuetype",
        "--paginate",
        "--json",
    ];
    let response = run(acli, &args, true, deadline).await?;
    let issues = parse_issues(&response, site)?;
    Ok((identity, issues))
}

async fn run(
    acli: &str,
    args: &[&str],
    search: bool,
    deadline: Instant,
) -> Result<Vec<u8>, JiraError> {
    let mut child = Command::new(acli)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| error("jira_unavailable", "Could not start the Jira CLI."))?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let mut out_task = tokio::spawn(read_bounded(stdout, STDOUT_LIMIT));
    let mut err_task = tokio::spawn(read_bounded(stderr, STDERR_LIMIT));
    let mut out = None;
    let mut err = None;
    let mut status = None;
    while status.is_none() || out.is_none() || err.is_none() {
        tokio::select! {
            result = &mut out_task, if out.is_none() => match result {
                Ok(Ok(bytes)) => out = Some(bytes),
                _ => {
                    out_task.abort(); err_task.abort();
                    let _ = child.kill().await; let _ = child.wait().await;
                    return Err(error("jira_unavailable", "Jira CLI output exceeded its size limit."));
                }
            },
            result = &mut err_task, if err.is_none() => match result {
                Ok(Ok(bytes)) => err = Some(bytes),
                _ => {
                    out_task.abort(); err_task.abort();
                    let _ = child.kill().await; let _ = child.wait().await;
                    return Err(error("jira_unavailable", "Jira CLI output exceeded its size limit."));
                }
            },
            _ = tokio::time::sleep_until(deadline) => {
                out_task.abort(); err_task.abort();
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(error("jira_timeout", "Jira refresh timed out."));
            },
            result = child.wait(), if status.is_none() => {
                status = Some(result.map_err(|_| error("jira_unavailable", "Jira CLI process failed."))?);
            },
        }
    }
    let status = status.unwrap();
    if !status.success() {
        return Err(if search {
            error(
                "jira_unavailable",
                "Jira search failed; verify CLI access and try again.",
            )
        } else {
            error(
                "jira_auth_required",
                "Jira CLI authentication is unavailable; run `acli jira auth login` manually.",
            )
        });
    }
    Ok(out.unwrap())
}

async fn read_bounded<R: AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
) -> std::io::Result<Vec<u8>> {
    let mut result = Vec::with_capacity(limit.min(16 * 1024));
    let mut buffer = [0u8; 8192];
    loop {
        let n = reader.read(&mut buffer).await?;
        if n == 0 {
            return Ok(result);
        }
        if result.len().saturating_add(n) > limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "output limit",
            ));
        }
        result.extend_from_slice(&buffer[..n]);
    }
}

pub fn parse_identity(bytes: &[u8]) -> Result<Identity, JiraError> {
    let text = std::str::from_utf8(bytes).map_err(|_| error("jira_auth_required", "Jira CLI authentication response was not recognized; run `acli jira auth login` manually."))?;
    let mut site = None;
    let mut email = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Site:") {
            if site.is_none() {
                site = nonempty(value);
            }
        }
        if let Some(value) = line.strip_prefix("Email:") {
            if email.is_none() {
                email = nonempty(value);
            }
        }
    }
    let site = site.ok_or_else(|| error("jira_auth_required", "Jira CLI authentication response was not recognized; run `acli jira auth login` manually."))?;
    let account_email = email.ok_or_else(|| error("jira_auth_required", "Jira CLI authentication response was not recognized; run `acli jira auth login` manually."))?;
    let site_url = if site.contains("://") {
        site.to_owned()
    } else {
        format!("https://{site}")
    };
    let parsed = Url::parse(&site_url).map_err(|_| error("jira_auth_required", "Jira CLI authentication response was not recognized; run `acli jira auth login` manually."))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || parsed.path() != "/"
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(error(
            "jira_auth_required",
            "Jira CLI authentication response was not recognized; run `acli jira auth login` manually.",
        ));
    }
    Ok(Identity {
        site: parsed.host_str().unwrap().to_owned(),
        account_email: account_email.to_owned(),
    })
}
fn nonempty(s: &str) -> Option<&str> {
    let s = s.trim();
    (!s.is_empty()).then_some(s)
}

pub fn parse_issues(bytes: &[u8], site: &Url) -> Result<Vec<SourceIssue>, JiraError> {
    let values: Vec<Value> = serde_json::from_slice(bytes).map_err(|_| {
        error(
            "jira_invalid_response",
            "Jira returned an invalid response.",
        )
    })?;
    let mut ids = HashSet::with_capacity(values.len());
    let mut keys = HashSet::with_capacity(values.len());
    let mut issues = Vec::with_capacity(values.len());
    for value in values {
        let id = field(value.get("id"));
        let key = field(value.get("key"));
        let self_url = field(value.get("self"));
        let fields = value.get("fields");
        let summary = field(fields.and_then(|v| v.get("summary")));
        let status = fields.and_then(|v| v.get("status"));
        let status_name = field(status.and_then(|v| v.get("name")));
        let status_category = field(
            status
                .and_then(|v| v.get("statusCategory"))
                .and_then(|v| v.get("key")),
        );
        let (
            Some(id),
            Some(key),
            Some(self_url),
            Some(summary),
            Some(status_name),
            Some(status_category),
        ) = (id, key, self_url, summary, status_name, status_category)
        else {
            return Err(error(
                "jira_invalid_response",
                "Jira returned incomplete issue data.",
            ));
        };
        if !["new", "indeterminate", "done"].contains(&status_category)
            || !ids.insert(id.to_owned())
            || !keys.insert(key.to_owned())
            || !same_origin(self_url, site)
        {
            return Err(error(
                "jira_invalid_response",
                "Jira returned invalid or duplicate issue data.",
            ));
        }
        let priority = optional_name(fields.and_then(|v| v.get("priority")));
        let issue_type = optional_name(fields.and_then(|v| v.get("issuetype")));
        issues.push(SourceIssue {
            id: id.into(),
            key: key.into(),
            summary: summary.into(),
            status_name: status_name.into(),
            status_category: status_category.into(),
            priority,
            issue_type,
        });
    }
    Ok(issues)
}
fn field(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}
fn optional_name(value: Option<&Value>) -> Option<String> {
    value
        .and_then(|v| if v.is_null() { None } else { v.get("name") })
        .and_then(Value::as_str)
        .map(str::to_owned)
}
fn same_origin(raw: &str, site: &Url) -> bool {
    let Ok(url) = Url::parse(raw) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    // Atlassian's CLI can emit internal service URLs; they are validated but never used as links.
    let allowed_host = host.eq_ignore_ascii_case(site.host_str().unwrap_or(""))
        || host.to_ascii_lowercase().ends_with(".prod.atl-paas.net");
    url.scheme() == "https"
        && allowed_host
        && url.port_or_known_default() == site.port_or_known_default()
        && url.username().is_empty()
        && url.password().is_none()
}
