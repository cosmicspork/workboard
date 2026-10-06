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
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub parent: Option<ParentIssue>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParentIssue {
    pub key: String,
    pub summary: Option<String>,
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
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
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
    pub project_id: Option<String>,
    pub tags: Vec<String>,
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
pub struct AssignProject {
    pub items: Vec<VersionedId>,
    pub project_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub position: i64,
    pub target_on: Option<String>,
    pub jira_keys: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub version: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateProject {
    pub name: String,
    #[serde(default)]
    pub target_on: Option<String>,
    #[serde(default)]
    pub jira_keys: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateProject {
    pub version: i64,
    pub name: String,
    pub target_on: Option<String>,
    pub jira_keys: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectOrder {
    pub ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeleteProject {
    pub version: i64,
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
    pub labels: Vec<String>,
    pub parent: Option<ParentIssue>,
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
    pub project_id: Option<String>,
    pub effective_project_id: Option<String>,
    pub project_source: Option<String>,
    pub tags: Vec<String>,
    pub jira_tags: Vec<String>,
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
    pub projects: Vec<Project>,
    pub tasks: Vec<Task>,
    pub sync: Sync,
}
