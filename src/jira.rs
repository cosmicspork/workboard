//! A Jira refresh: every issue assigned to the credential's user, paged
//! through jira-review's `search` tool. Workboard never holds the Jira
//! credential; jira-review does, or the Tracon node lends it.
//!
//! - In plugin mode the search is `jira-review__search`, called through the
//!   node's `tools/call` method on the plugin-to-node socket (`--node-socket`)
//!   as this plugin. The node decides it by policy and jira-review's
//!   `[[plugin]]` entry without holding it, and lends it the channel's `jira`
//!   binding, or `jira-<profile>` when `jira_profile` is set.
//! - Standalone it is `search` on a `jira-review mcp` child started for the
//!   refresh, which finds its own credential (environment, `.env`, keyring).
//!
//! Either way a page answers `site`, `account_email`, `issues` and
//! `next_cursor`, as JSON text in `content[0].text` or as
//! `structuredContent`.

use crate::model::{Identity, ParentIssue, SourceIssue};
use serde_json::{Value, json};
use std::{collections::HashSet, path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
    time::Instant,
};
use url::Url;

const DEADLINE: Duration = Duration::from_secs(120);
const PAGE_LIMIT: usize = 8 * 1024 * 1024;
const MAX_PAGES: usize = 100;
const JQL: &str = "assignee = currentUser() ORDER BY updated DESC";
/// jira-review's search as the node offers it.
pub const NODE_TOOL: &str = "jira-review__search";
/// The MCP protocol version spoken to a `jira-review mcp` child.
const PROTOCOL_VERSION: &str = "2025-06-18";
/// How much of a child's stderr an error quotes.
const STDERR_TAIL: usize = 2048;
/// How long a finished child is given to exit before it is killed.
const EXIT_GRACE: Duration = Duration::from_secs(2);

/// Where a refresh asks jira-review's `search`.
#[derive(Clone)]
pub enum Source {
    /// Plugin mode: the node's `tools/call`.
    Node {
        /// `--node-socket`, absent when the node did not pass one.
        socket: Option<PathBuf>,
        /// `TRACON_PLUGIN_TOKEN`, the socket's bearer token.
        token: String,
        /// Passed as `profile`, so the node lends `jira-<profile>`.
        profile: Option<String>,
    },
    /// Standalone: `<program> [--profile P] mcp`, started per refresh.
    Child {
        program: PathBuf,
        profile: Option<String>,
    },
}

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

/// At most 300 characters of a message from elsewhere.
fn detail(text: Option<&str>) -> String {
    text.unwrap_or_default().trim().chars().take(300).collect()
}

#[derive(Debug)]
pub struct Page {
    pub identity: Identity,
    pub issues: Vec<SourceIssue>,
    pub next_cursor: Option<String>,
}

/// The arguments of one `search` page. `profile` is left out when unset, so
/// the node lends the bare `jira` binding.
pub fn search_arguments(cursor: Option<&str>, profile: Option<&str>) -> Value {
    let mut arguments = json!({ "jql": JQL, "page": true });
    if let Some(cursor) = cursor {
        arguments["cursor"] = json!(cursor);
    }
    if let Some(profile) = profile {
        arguments["profile"] = json!(profile);
    }
    arguments
}

pub async fn fetch(
    source: &Source,
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
    let deadline = Instant::now() + DEADLINE;
    let timed_out = |_| error("jira_timeout", "Jira refresh timed out.");
    let mut session = tokio::time::timeout_at(deadline, Session::open(source))
        .await
        .map_err(timed_out)??;
    let fetched = async {
        let mut identity: Option<Identity> = None;
        let mut issues = Vec::new();
        let mut cursors = HashSet::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let result = tokio::time::timeout_at(deadline, session.search(cursor.as_deref()))
                .await
                .map_err(timed_out)??;
            let page = page_of(&result)?;
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
    .await;
    session.close().await;
    fetched
}

/// One refresh's way to jira-review.
enum Session {
    Node {
        client: reqwest::Client,
        token: String,
        profile: Option<String>,
        next: u64,
    },
    Child(Box<ChildSession>),
}

impl Session {
    async fn open(source: &Source) -> Result<Self, JiraError> {
        match source {
            Source::Node {
                socket,
                token,
                profile,
            } => {
                let socket = socket.clone().ok_or_else(|| {
                    error(
                        "tracon_unsupported",
                        "Tracon started Workboard without a node socket, so it cannot call \
                         jira-review. Update Tracon, then restart Workboard in Tracon.",
                    )
                })?;
                let client = reqwest::Client::builder()
                    .unix_socket(socket)
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .map_err(|_| {
                        error("tracon_unavailable", "Could not prepare the Tracon client.")
                    })?;
                Ok(Self::Node {
                    client,
                    token: token.clone(),
                    profile: profile.clone(),
                    next: 0,
                })
            }
            Source::Child { program, profile } => ChildSession::start(program, profile.as_deref())
                .await
                .map(|child| Self::Child(Box::new(child))),
        }
    }

    /// One page of `search`: the tool's MCP result.
    async fn search(&mut self, cursor: Option<&str>) -> Result<Value, JiraError> {
        match self {
            Self::Node {
                client,
                token,
                profile,
                next,
            } => {
                *next += 1;
                let arguments = search_arguments(cursor, profile.as_deref());
                node_call(client, token, *next, arguments).await
            }
            // The child's profile is its `--profile`, not an argument.
            Self::Child(child) => {
                let arguments = search_arguments(cursor, None);
                child
                    .request(
                        "tools/call",
                        json!({ "name": "search", "arguments": arguments }),
                    )
                    .await
            }
        }
    }

    async fn close(self) {
        if let Self::Child(child) = self {
            child.close().await;
        }
    }
}

async fn node_call(
    client: &reqwest::Client,
    token: &str,
    id: u64,
    arguments: Value,
) -> Result<Value, JiraError> {
    let request = json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": NODE_TOOL, "arguments": arguments },
    });
    let unreachable = || {
        error(
            "tracon_unavailable",
            "Tracon's node socket did not answer. Restart Workboard in Tracon, then refresh again.",
        )
    };
    // The host is not looked up: the request goes to the socket.
    let mut response = client
        .post("http://tracon/")
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(request.to_string())
        .send()
        .await
        .map_err(|_| unreachable())?;
    let status = response.status();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unreachable())? {
        if body.len() + chunk.len() > PAGE_LIMIT {
            return Err(invalid("Jira response exceeded its size limit."));
        }
        body.extend_from_slice(&chunk);
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(forbidden());
    }
    let reply: Value = serde_json::from_slice(&body)
        .map_err(|_| invalid("Tracon returned an invalid response."))?;
    if let Some(e) = reply.get("error").filter(|e| !e.is_null()) {
        return Err(node_error(e));
    }
    reply
        .get("result")
        .filter(|r| r.is_object())
        .cloned()
        .ok_or_else(|| invalid("Tracon returned an invalid response."))
}

fn forbidden() -> JiraError {
    error(
        "tracon_forbidden",
        "Tracon refused Workboard's token on its node socket. Restart Workboard in Tracon, then refresh again.",
    )
}

/// A JSON-RPC error from the node's `tools/call`.
fn node_error(e: &Value) -> JiraError {
    let message = detail(e["message"].as_str());
    match e["code"].as_i64() {
        // Refused: by policy, by jira-review's `[[plugin]]` entry, because the
        // call would be asked (`data.decision`), or because the channel binds
        // no Jira credential.
        Some(-32003) => error(
            "jira_refused",
            format!("Tracon did not let Workboard run jira-review's search: {message}"),
        ),
        Some(-32601) => error(
            "tracon_unsupported",
            format!("This Tracon node does not answer Workboard's tools/call: {message}"),
        ),
        Some(-32602) if message.contains("not installed") => error(
            "jira_review_missing",
            format!("{message}. Install it with: tracon plugin add cosmicspork/jira-review"),
        ),
        Some(-32001) => forbidden(),
        _ => error(
            "jira_unavailable",
            format!("jira-review's search through Tracon failed: {message}"),
        ),
    }
}

/// A `jira-review mcp` child, spoken to one JSON-RPC message per line.
struct ChildSession {
    program: String,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: Option<JoinHandle<Vec<u8>>>,
    next: u64,
}

impl ChildSession {
    async fn start(program: &std::path::Path, profile: Option<&str>) -> Result<Self, JiraError> {
        let shown = program.display().to_string();
        let mut command = Command::new(program);
        if let Some(profile) = profile {
            command.arg("--profile").arg(profile);
        }
        command
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|e| {
            error(
                "jira_review_missing",
                if e.kind() == std::io::ErrorKind::NotFound {
                    format!(
                        "jira-review was not found ({shown}). Install it from \
                         https://github.com/cosmicspork/jira-review, or name it with --jira-review."
                    )
                } else {
                    format!("Could not start jira-review ({shown}): {e}")
                },
            )
        })?;
        let (Some(stdin), Some(stdout), Some(mut stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(error("jira_unavailable", "Could not talk to jira-review."));
        };
        // Kept so an early exit can say why; only the tail is quoted.
        let stderr = tokio::spawn(async move {
            let mut tail = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = stderr.read(&mut buf).await {
                tail.extend_from_slice(&buf[..n]);
                if tail.len() > STDERR_TAIL {
                    tail.drain(..tail.len() - STDERR_TAIL);
                }
            }
            tail
        });
        let mut session = Self {
            program: shown,
            child,
            stdin,
            stdout: BufReader::new(stdout),
            stderr: Some(stderr),
            next: 0,
        };
        session
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": "workboard", "version": env!("CARGO_PKG_VERSION") },
                }),
            )
            .await?;
        session
            .send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await?;
        Ok(session)
    }

    async fn send(&mut self, message: &Value) -> Result<(), JiraError> {
        let mut line = message.to_string();
        line.push('\n');
        let written = async {
            self.stdin.write_all(line.as_bytes()).await?;
            self.stdin.flush().await
        }
        .await;
        match written {
            Ok(()) => Ok(()),
            Err(_) => Err(self.exited().await),
        }
    }

    /// Send a request and wait for the answer with its id: its `result`, or
    /// its error.
    async fn request(&mut self, method: &str, params: Value) -> Result<Value, JiraError> {
        self.next += 1;
        let id = self.next;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await?;
        loop {
            let mut line = Vec::new();
            let read = (&mut self.stdout)
                .take(PAGE_LIMIT as u64 + 1)
                .read_until(b'\n', &mut line)
                .await;
            match read {
                Ok(0) | Err(_) => return Err(self.exited().await),
                Ok(_) if line.len() > PAGE_LIMIT => {
                    return Err(invalid("Jira response exceeded its size limit."));
                }
                Ok(_) => {}
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let message: Value = serde_json::from_slice(&line)
                .map_err(|_| invalid("jira-review returned an invalid response."))?;
            if message.get("id") != Some(&json!(id)) {
                // A notification, or an answer to nothing we are waiting on.
                continue;
            }
            if let Some(e) = message.get("error").filter(|e| !e.is_null()) {
                return Err(error(
                    "jira_unavailable",
                    format!(
                        "jira-review refused {method}: {}",
                        detail(e["message"].as_str())
                    ),
                ));
            }
            return message
                .get("result")
                .filter(|r| r.is_object())
                .cloned()
                .ok_or_else(|| invalid("jira-review returned an invalid response."));
        }
    }

    /// Why the child stopped answering: its exit and the tail of its stderr.
    async fn exited(&mut self) -> JiraError {
        let status = match tokio::time::timeout(EXIT_GRACE, self.child.wait()).await {
            Ok(Ok(status)) => status.to_string(),
            _ => {
                let _ = self.child.start_kill();
                "it stopped answering".to_string()
            }
        };
        let stderr = match self.stderr.take() {
            Some(tail) => match tokio::time::timeout(EXIT_GRACE, tail).await {
                Ok(Ok(tail)) => String::from_utf8_lossy(&tail).trim().to_string(),
                _ => String::new(),
            },
            None => String::new(),
        };
        let mut message = format!("jira-review ({}) stopped: {status}", self.program);
        if !stderr.is_empty() {
            message.push_str(": ");
            message.push_str(&detail(stderr.lines().last()));
        }
        error("jira_unavailable", message)
    }

    /// Close stdin, which ends `jira-review mcp`; kill it if it lingers.
    async fn close(self) {
        let Self {
            mut child, stdin, ..
        } = self;
        drop(stdin);
        if tokio::time::timeout(EXIT_GRACE, child.wait())
            .await
            .is_err()
        {
            let _ = child.kill().await;
        }
    }
}

/// The page a `search` result carries, or the error it reports.
fn page_of(result: &Value) -> Result<Page, JiraError> {
    let text = result["content"][0]["text"].as_str();
    if result["isError"] == true {
        return Err(error(
            "jira_unavailable",
            format!("jira-review could not search Jira: {}", detail(text)),
        ));
    }
    let page = result
        .get("structuredContent")
        .filter(|v| v.is_object())
        .cloned()
        .or_else(|| text.and_then(|t| serde_json::from_str::<Value>(t).ok()))
        .ok_or_else(|| invalid("jira-review answered no page."))?;
    parse_value(&page)
}

fn check_identity(
    identity: &Identity,
    configured_host: &str,
    bound: Option<(&str, &str)>,
) -> Result<(), JiraError> {
    if identity.site != configured_host {
        return Err(error(
            "jira_identity_changed",
            "jira-review's Jira site does not match the configured site.",
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
                "jira-review's Jira identity differs from the identity this board is bound to.",
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
    parse_value(&value)
}

fn parse_value(value: &Value) -> Result<Page, JiraError> {
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
        .ok_or_else(|| invalid("jira-review returned an invalid Jira site."))?;
    let account_email = field(value.get("account_email"))
        .ok_or_else(|| invalid("jira-review returned no Jira account."))?
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
