use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

/// A field that must be given, though it may be null: in an input that
/// replaces every field, leaving one out must not clear it.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct Link {
    pub label: String,
    /// An `http://` or `https://` URL without credentials.
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct CreateTask {
    /// The task's title; a new task starts in Inbox.
    pub title: String,
    /// The next physical step, in Markdown.
    #[serde(default)]
    #[schemars(extend("x-tracon-format" = "markdown"))]
    pub next_action: String,
    /// Notes, in Markdown.
    #[serde(default)]
    #[schemars(extend("x-tracon-format" = "markdown"))]
    pub notes: String,
    #[serde(default)]
    pub links: Vec<Link>,
    /// A project id from `projects`, or null for none.
    #[serde(default)]
    pub project_id: Option<String>,
    /// `name` or `key:value` tags, such as `person:sam`.
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct UpdateTask {
    /// The task's `version` as last read; a task changed since is refused.
    pub version: i64,
    /// A personal title; null follows the Jira summary (a task without a
    /// Jira issue needs one).
    #[serde(deserialize_with = "present")]
    #[schemars(required)]
    pub title_override: Option<String>,
    /// The next physical step, in Markdown.
    #[schemars(extend("x-tracon-format" = "markdown"))]
    pub next_action: String,
    /// Notes, in Markdown.
    #[schemars(extend("x-tracon-format" = "markdown"))]
    pub notes: String,
    /// One of inbox, next, waiting, later, done.
    pub state: String,
    /// Picked for Today; only a Next task can be.
    pub today: bool,
    /// Who or what the task waits on.
    #[serde(deserialize_with = "present")]
    #[schemars(required)]
    pub waiting_for: Option<String>,
    /// The date (YYYY-MM-DD) to look at it again.
    #[serde(deserialize_with = "present")]
    #[schemars(required)]
    pub review_on: Option<String>,
    pub links: Vec<Link>,
    /// The project chosen for the task, overriding a Jira match; null for none.
    #[serde(deserialize_with = "present")]
    #[schemars(required)]
    pub project_id: Option<String>,
    /// The task's own tags (not its read-only `jira_tags`).
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct VersionedId {
    pub id: String,
    pub version: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct TriageInput {
    /// The tasks, each with its `version` as last read.
    pub items: Vec<VersionedId>,
    /// One of next, later, done.
    pub state: String,
    /// The review date (YYYY-MM-DD) to set, or null to clear it.
    #[serde(deserialize_with = "present")]
    #[schemars(required)]
    pub review_on: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct AssignProject {
    /// The tasks, each with its `version` as last read.
    pub items: Vec<VersionedId>,
    /// A project id from `projects`, or null to clear the choice.
    #[serde(deserialize_with = "present")]
    #[schemars(required)]
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct CreateProject {
    pub name: String,
    /// A target date, YYYY-MM-DD.
    #[serde(default)]
    pub target_on: Option<String>,
    /// Jira issue, epic or project keys whose issues belong to it.
    #[serde(default)]
    pub jira_keys: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct UpdateProject {
    /// The project's `version` as last read.
    pub version: i64,
    pub name: String,
    #[serde(deserialize_with = "present")]
    #[schemars(required)]
    pub target_on: Option<String>,
    pub jira_keys: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ProjectOrder {
    /// Every project id, once each, in priority order.
    pub ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct DeleteProject {
    /// The project's `version` as last read.
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
    /// The browse link; absent when no Jira site is configured.
    pub url: Option<String>,
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

/// `board`: every filter is optional, and all given must match.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardQuery {
    /// A project id: tasks whose project (chosen, or matched by Jira key) is
    /// it. `none` for tasks with no project.
    #[serde(default)]
    pub project: Option<String>,
    /// A tag the task carries, its own or a Jira tag such as `jira:app`.
    #[serde(default)]
    pub tag: Option<String>,
    /// One of inbox, next, waiting, later, done.
    #[serde(default)]
    pub state: Option<String>,
}

/// An operation that takes no arguments.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoArgs {}

/// A task by its Workboard id.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct TaskRef {
    /// The task's Workboard id (not its Jira key).
    pub id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct TaskUpdate {
    /// The task's Workboard id (not its Jira key).
    pub id: String,
    /// Every field is replaced: send them all, as `task` returns them, with
    /// your changes.
    #[serde(flatten)]
    pub task: UpdateTask,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ProjectUpdate {
    /// The project's id.
    pub id: String,
    #[serde(flatten)]
    pub project: UpdateProject,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct ProjectDelete {
    /// The project's id.
    pub id: String,
    #[serde(flatten)]
    pub project: DeleteProject,
}
