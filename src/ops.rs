//! The operation table: every board operation, named, routed and described
//! once. The HTTP API (`http::api`) registers one route per entry, and every
//! MCP surface (`serve`'s tools, the stdio `mcp` mode, the `op` command)
//! lists one tool per entry, so the two cannot drift apart. Both run an
//! operation through [`run`], over the same store functions.
//!
//! An HTTP request's arguments are its path parameters plus its JSON body
//! (or, for a `GET`, its query string); a tool's are one JSON object with
//! the same names. `PUT /api/tasks/{id}` with body `{"version": 3, ...}` is
//! tool `task_update` with `{"id": "...", "version": 3, ...}`.

use schemars::{JsonSchema, generate::SchemaSettings};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::model::{
    AssignProject, BoardQuery, CreateProject, CreateTask, NoArgs, ProjectDelete, ProjectOrder,
    ProjectUpdate, TaskRef, TaskUpdate, TriageInput,
};
use crate::store::{Store, StoreError};

pub struct Op {
    /// The tool name, unprefixed.
    pub name: &'static str,
    /// The HTTP method of its route.
    pub method: &'static str,
    /// Its route, in axum's syntax; `{name}` segments are arguments.
    pub path: &'static str,
    /// The HTTP status of a success.
    pub status: u16,
    /// Changes the board. Writes are offered for approval, never applied
    /// on an agent's word alone.
    pub write: bool,
    pub description: &'static str,
    schema: fn() -> Value,
}

impl Op {
    /// The JSON schema of its arguments, with nested types inlined.
    pub fn input_schema(&self) -> Value {
        (self.schema)()
    }

    /// The names of its path parameters, in order.
    pub fn path_params(&self) -> Vec<&'static str> {
        self.path
            .split('/')
            .filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
            .collect()
    }
}

fn schema<T: JsonSchema>() -> Value {
    let generator = SchemaSettings::draft2020_12()
        .with(|s| s.inline_subschemas = true)
        .into_generator();
    let mut schema =
        serde_json::to_value(generator.into_root_schema_for::<T>()).unwrap_or_else(|_| json!({}));
    if let Some(o) = schema.as_object_mut() {
        o.remove("$schema");
        o.remove("title");
        o.entry("type").or_insert_with(|| json!("object"));
        o.entry("properties").or_insert_with(|| json!({}));
    }
    schema
}

macro_rules! op {
    ($name:literal, $method:literal $path:literal => $status:literal, $ty:ty, $write:literal, $description:literal) => {
        Op {
            name: $name,
            method: $method,
            path: $path,
            status: $status,
            write: $write,
            description: $description,
            schema: schema::<$ty>,
        }
    };
}

pub const OPS: &[Op] = &[
    op!(
        "board", "GET" "/api/board" => 200, BoardQuery, false,
        "The board: projects in priority order, tasks (each with its id, version, state, Today flag, next action, notes, tags, review date, project and the Jira facts of its issue) and the last Jira refresh. Filter by project, tag or state."
    ),
    op!(
        "task", "GET" "/api/tasks/{id}" => 200, TaskRef, false,
        "One task by its Workboard id, as `board` lists it."
    ),
    op!(
        "projects", "GET" "/api/projects" => 200, NoArgs, false,
        "The projects in priority order, each with its id, version, target date and Jira keys."
    ),
    op!(
        "task_create", "POST" "/api/tasks" => 201, CreateTask, true,
        "Add a task to Inbox, not tied to Jira."
    ),
    op!(
        "task_update", "PUT" "/api/tasks/{id}" => 200, TaskUpdate, true,
        "Replace a task's own fields: state, Today, next action, notes, tags, waiting for, review date, links, project and personal title. Send every field as `task` returns it, with your changes, and its version. Jira facts are read-only."
    ),
    op!(
        "task_triage", "POST" "/api/tasks/triage" => 200, TriageInput, true,
        "Move tasks to Next, Later or Done and set or clear their review date. Moving off Next clears Today."
    ),
    op!(
        "task_assign_project", "POST" "/api/tasks/project" => 200, AssignProject, true,
        "Choose a project for tasks, overriding a Jira match, or clear the choice."
    ),
    op!(
        "project_create", "POST" "/api/projects" => 201, CreateProject, true,
        "Add a project, last in priority."
    ),
    op!(
        "project_update", "PUT" "/api/projects/{id}" => 200, ProjectUpdate, true,
        "Rename a project or change its target date or Jira keys."
    ),
    op!(
        "project_delete", "DELETE" "/api/projects/{id}" => 204, ProjectDelete, true,
        "Delete a project. Tasks that chose it lose the choice."
    ),
    op!(
        "project_order", "POST" "/api/projects/order" => 200, ProjectOrder, true,
        "Set the projects' priority order. List every project once."
    ),
];

/// HTTP routes that are deliberately not tools, each with why.
pub const HTTP_ONLY: &[(&str, &str, &str)] = &[(
    "POST",
    "/api/jira/refresh",
    "A refresh reads Jira through the node's operator route (`GET /api/jira/search`), so an \
     agent's call would reach Jira as the operator. An agent that wants fresh Jira facts calls \
     jira-review's `search` under its own policy instead.",
)];

/// Tools that are deliberately not HTTP routes, each with why.
pub const TOOL_ONLY: &[(&str, &str)] = &[(
    "proposal_status",
    "Standalone `mcp` only: how an agent hears what the person decided in `workboard review`. \
     Plugin mode needs none, since the node's card is the review.",
)];

pub fn find(name: &str) -> Option<&'static Op> {
    OPS.iter().find(|op| op.name == name)
}

#[derive(Debug)]
pub enum OpError {
    Unknown(String),
    BadInput(String),
    Store(StoreError),
}

impl From<StoreError> for OpError {
    fn from(err: StoreError) -> Self {
        Self::Store(err)
    }
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(name) => write!(f, "no operation {name}"),
            Self::BadInput(message) => f.write_str(message),
            Self::Store(err) => err.fmt(f),
        }
    }
}

impl OpError {
    /// The HTTP status, error code and message the API answers with; a tool
    /// that fails answers the same code and message.
    pub fn parts(&self) -> (u16, &'static str, String) {
        match self {
            Self::Unknown(_) => (404, "not_found", "API route not found.".into()),
            Self::BadInput(message) => (400, "invalid_request", message.clone()),
            Self::Store(StoreError::Invalid(message)) => (400, "invalid_request", message.clone()),
            Self::Store(StoreError::NotFound) => (404, "not_found", "Not found.".into()),
            Self::Store(StoreError::Conflict) => (
                409,
                "task_conflict",
                "This changed elsewhere. Reload it before saving.".into(),
            ),
            Self::Store(StoreError::IdentityChanged) => (
                502,
                "jira_identity_changed",
                "Tracon's Jira account or site changed from the one this board is bound to.".into(),
            ),
            Self::Store(StoreError::Internal(_)) => {
                (500, "internal_error", "Local data is unavailable.".into())
            }
        }
    }

    /// The JSON error body, as the HTTP API answers it.
    pub fn body(&self) -> Value {
        let (_, code, message) = self.parts();
        json!({ "error": code, "message": message })
    }
}

pub fn input<T: DeserializeOwned>(args: &Value) -> Result<T, OpError> {
    let args = if args.is_null() {
        json!({})
    } else {
        args.clone()
    };
    serde_json::from_value(args).map_err(|e| OpError::BadInput(format!("invalid arguments: {e}")))
}

fn out<T: Serialize>(value: T) -> Result<Value, OpError> {
    serde_json::to_value(value).map_err(|e| OpError::Store(StoreError::Internal(e.to_string())))
}

const STATES: [&str; 5] = ["inbox", "next", "waiting", "later", "done"];

/// What an operation runs against.
pub struct Ctx<'a> {
    pub store: &'a Store,
    /// A Jira refresh is running in this process, for `board`'s sync.
    pub refresh_in_progress: bool,
}

fn board(ctx: &Ctx, query: BoardQuery) -> Result<Value, OpError> {
    if let Some(state) = query.state.as_deref()
        && !STATES.contains(&state)
    {
        return Err(OpError::BadInput(
            "state must be inbox, next, waiting, later or done".into(),
        ));
    }
    let local_date = chrono::Local::now().date_naive().to_string();
    let mut board = ctx
        .store
        .board(local_date, ctx.refresh_in_progress)
        .map_err(|e| OpError::Store(StoreError::Internal(e.to_string())))?;
    let tag = query.tag.as_deref().map(|t| t.trim().to_lowercase());
    board.tasks.retain(|task| {
        let project = match query.project.as_deref() {
            None => true,
            Some("none") => task.effective_project_id.is_none(),
            Some(id) => task.effective_project_id.as_deref() == Some(id),
        };
        let tagged = tag
            .as_deref()
            .is_none_or(|tag| task.tags.iter().chain(&task.jira_tags).any(|t| t == tag));
        let state = query.state.as_deref().is_none_or(|s| task.state == s);
        project && tagged && state
    });
    out(board)
}

/// Run operation `name` with `args`, as the caller in front of it: the
/// operator's page, an approved tool call, or a person at the terminal.
/// Callers decide who may; this only does it.
pub fn run(ctx: &Ctx, name: &str, args: &Value) -> Result<Value, OpError> {
    let store = ctx.store;
    match name {
        "board" => board(ctx, input(args)?),
        "task" => out(store.task(&input::<TaskRef>(args)?.id)?),
        "projects" => {
            input::<NoArgs>(args)?;
            out(store.projects()?)
        }
        "task_create" => out(store.create(input::<CreateTask>(args)?)?),
        "task_update" => {
            let TaskUpdate { id, task } = input(args)?;
            out(store.update(&id, task)?)
        }
        "task_triage" => out(store.triage(input::<TriageInput>(args)?)?),
        "task_assign_project" => out(store.assign_project(input::<AssignProject>(args)?)?),
        "project_create" => out(store.create_project(input::<CreateProject>(args)?)?),
        "project_update" => {
            let ProjectUpdate { id, project } = input(args)?;
            out(store.update_project(&id, project)?)
        }
        "project_delete" => {
            let ProjectDelete { id, project } = input(args)?;
            store.delete_project(&id, project.version)?;
            Ok(json!({ "deleted": id }))
        }
        "project_order" => out(store.order_projects(&input::<ProjectOrder>(args)?.ids)?),
        other => Err(OpError::Unknown(other.to_string())),
    }
}

/// The tools, as `tools/list` answers them. `write_note` ends each write's
/// description: how this surface applies it.
pub fn tools(write_note: &str) -> Vec<Value> {
    OPS.iter()
        .map(|op| {
            let description = if op.write {
                format!("{} {write_note}", op.description)
            } else {
                op.description.to_string()
            };
            json!({
                "name": op.name,
                "description": description,
                "inputSchema": op.input_schema(),
                "annotations": {
                    "readOnlyHint": !op.write,
                    "destructiveHint": op.write && op.method != "POST",
                    "idempotentHint": !op.write,
                    "openWorldHint": false,
                },
            })
        })
        .collect()
}
