use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceIssue {
    pub id: String,
    pub key: String,
    pub summary: String,
    pub status_name: String,
    pub status_category: String,
    pub priority: Option<String>,
    pub issue_type: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identity {
    pub site: String,
    pub account_email: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Link {
    pub label: String,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateTask {
    pub title: String,
    #[serde(default)]
    pub next_action: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub links: Vec<Link>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateTask {
    pub version: i64,
    pub title_override: Option<String>,
    pub next_action: String,
    pub notes: String,
    pub state: String,
    pub today: bool,
    pub waiting_for: Option<String>,
    pub review_on: Option<String>,
    pub links: Vec<Link>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct VersionedId {
    pub id: String,
    pub version: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TriageInput {
    pub items: Vec<VersionedId>,
    pub state: String,
    pub review_on: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct JiraFact {
    pub id: String,
    pub key: String,
    pub summary: String,
    pub status_name: String,
    pub status_category: String,
    pub priority: Option<String>,
    pub issue_type: Option<String>,
    pub url: String,
    pub last_seen_at: String,
    pub present_in_latest: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub jira_issue_id: Option<String>,
    pub title_override: Option<String>,
    pub title: String,
    pub next_action: String,
    pub notes: String,
    pub state: String,
    pub today: bool,
    pub waiting_for: Option<String>,
    pub review_on: Option<String>,
    pub links: Vec<Link>,
    pub created_at: String,
    pub updated_at: String,
    pub version: i64,
    pub jira: Option<JiraFact>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Sync {
    pub site: Option<String>,
    pub account_email: Option<String>,
    pub last_attempt_at: Option<String>,
    pub last_success_at: Option<String>,
    pub error_code: Option<String>,
    pub issue_count: i64,
    pub open_count: i64,
    pub done_count: i64,
    pub refresh_in_progress: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Board {
    pub local_date: String,
    pub tasks: Vec<Task>,
    pub sync: Sync,
}
