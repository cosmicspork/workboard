//! What a write would do, without doing it, and applying it only against
//! what was shown.
//!
//! [`preview`] reads the board and renders the change as Markdown: the
//! current values next to the proposed ones. Its token is the state the
//! preview was taken against (the versions of the records it touches, or the
//! project order), as canonical JSON. [`apply`] recomputes that token from
//! the board as it is now and refuses as stale when it differs, so a write
//! lands only on what the reviewer saw. Nothing here writes; no write is run
//! to preview it.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::contract::canonical;
use crate::model::{
    AssignProject, CreateProject, CreateTask, Project, ProjectDelete, ProjectOrder, ProjectUpdate,
    Task, TaskUpdate, TriageInput, VersionedId,
};
use crate::ops::{self, Ctx, OpError, input};
use crate::store::{self, Store, StoreError};

pub struct Preview {
    /// One line, for the card's heading.
    pub summary: String,
    /// Markdown.
    pub body: String,
    /// The state it was taken against, or none when nothing it reads can
    /// move under it (a create).
    pub token: Option<String>,
}

#[derive(Debug)]
pub enum ApplyError {
    /// What the call names moved since it was read or previewed. Nothing was
    /// written.
    Stale(String),
    Op(OpError),
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale(message) => f.write_str(message),
            Self::Op(err) => err.fmt(f),
        }
    }
}

const STALE: &str = "This changed since it was read or previewed; nothing was written. Read it again and propose the change anew.";

/// Run write `name` only if the board is still as `expected` describes it
/// (a preview's token), and refuse a stale `version` the same way.
pub fn apply(
    ctx: &Ctx,
    name: &str,
    args: &Value,
    expected: Option<&str>,
) -> Result<Value, ApplyError> {
    if let Some(expected) = expected {
        match token(ctx.store, name, args) {
            Ok(now) if now.as_deref() == Some(expected) => {}
            Ok(_) | Err(OpError::Store(StoreError::NotFound | StoreError::Conflict)) => {
                return Err(ApplyError::Stale(STALE.into()));
            }
            Err(err) => return Err(ApplyError::Op(err)),
        }
    }
    ops::run(ctx, name, args).map_err(|err| match err {
        OpError::Store(StoreError::Conflict) => ApplyError::Stale(STALE.into()),
        err => ApplyError::Op(err),
    })
}

#[derive(Default)]
struct Versions {
    tasks: BTreeMap<String, i64>,
    projects: BTreeMap<String, i64>,
    order: Option<Vec<String>>,
}

impl Versions {
    fn token(&self) -> String {
        let mut v = json!({});
        if !self.tasks.is_empty() {
            v["tasks"] = json!(self.tasks);
        }
        if !self.projects.is_empty() {
            v["projects"] = json!(self.projects);
        }
        if let Some(order) = &self.order {
            v["order"] = json!(order);
        }
        canonical(&v)
    }
}

/// The token of write `name` against the board as it is now.
fn token(store: &Store, name: &str, args: &Value) -> Result<Option<String>, OpError> {
    let mut v = Versions::default();
    match name {
        "task_create" | "project_create" => return Ok(None),
        "task_update" => {
            let a: TaskUpdate = input(args)?;
            let task = store.task(&a.id)?;
            v.tasks.insert(task.id, task.version);
        }
        "task_triage" | "task_assign_project" => {
            for item in items(name, args)? {
                let task = store.task(&item.id)?;
                v.tasks.insert(task.id, task.version);
            }
        }
        "project_update" => {
            let a: ProjectUpdate = input(args)?;
            let p = project(store, &a.id)?;
            v.projects.insert(p.id, p.version);
        }
        "project_delete" => {
            let a: ProjectDelete = input(args)?;
            let p = project(store, &a.id)?;
            v.projects.insert(p.id.clone(), p.version);
            for task in chosen(store, &p.id)? {
                v.tasks.insert(task.id, task.version);
            }
        }
        "project_order" => {
            v.order = Some(store.projects()?.into_iter().map(|p| p.id).collect());
        }
        other => return Err(OpError::Unknown(other.into())),
    }
    Ok(Some(v.token()))
}

fn items(name: &str, args: &Value) -> Result<Vec<VersionedId>, OpError> {
    Ok(if name == "task_triage" {
        input::<TriageInput>(args)?.items
    } else {
        input::<AssignProject>(args)?.items
    })
}

fn project(store: &Store, id: &str) -> Result<Project, OpError> {
    store
        .projects()?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or(OpError::Store(StoreError::NotFound))
}

fn all_tasks(store: &Store) -> Result<Vec<Task>, OpError> {
    let date = chrono::Local::now().date_naive().to_string();
    store
        .board(date, false)
        .map(|b| b.tasks)
        .map_err(|e| OpError::Store(StoreError::Internal(e.to_string())))
}

/// The tasks that chose project `id` themselves.
fn chosen(store: &Store, id: &str) -> Result<Vec<Task>, OpError> {
    Ok(all_tasks(store)?
        .into_iter()
        .filter(|t| t.project_id.as_deref() == Some(id))
        .collect())
}

/// A task named for a person: its Jira key, if any, and its title.
fn label(task: &Task) -> String {
    let title = one_line(&task.title, 80);
    match &task.jira {
        Some(jira) => format!("{} “{title}”", jira.key),
        None => format!("“{title}”"),
    }
}

fn one_line(s: &str, max: usize) -> String {
    let line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > max {
        let cut: String = line.chars().take(max - 1).collect();
        format!("{cut}…")
    } else {
        line
    }
}

/// A table cell: one line, pipes escaped, `(none)` for nothing.
fn cell(s: &str) -> String {
    let s = one_line(s, 200).replace('|', "\\|");
    if s.is_empty() { "(none)".into() } else { s }
}

fn opt(s: &Option<String>) -> String {
    s.clone().unwrap_or_default()
}

fn yes_no(b: bool) -> String {
    if b { "yes".into() } else { "no".into() }
}

/// A fence longer than any run of backticks in `text`.
fn fence(text: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat(longest.max(2) + 1)
}

fn block(title: &str, info: &str, text: &str) -> String {
    let f = fence(text);
    let text = if text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    };
    format!("\n**{title}**\n\n{f}{info}\n{text}{f}\n")
}

fn diff(before: &str, after: &str) -> String {
    // An empty text is no lines, not one blank line.
    let lines = |s: &str| {
        if s.is_empty() {
            String::new()
        } else {
            format!("{s}\n")
        }
    };
    let (before, after) = (lines(before), lines(after));
    similar::TextDiff::from_lines(&before, &after)
        .unified_diff()
        .context_radius(3)
        .header("now", "proposed")
        .to_string()
}

struct Table(Vec<[String; 3]>);

impl Table {
    fn row(&mut self, field: &str, now: String, proposed: String) {
        if now != proposed {
            self.0.push([field.into(), cell(&now), cell(&proposed)]);
        }
    }

    fn render(&self, head: [&str; 3]) -> String {
        let mut out = format!(
            "| {} | {} | {} |\n|---|---|---|\n",
            head[0], head[1], head[2]
        );
        for [a, b, c] in &self.0 {
            out.push_str(&format!("| {a} | {b} | {c} |\n"));
        }
        out
    }
}

fn names(projects: &[Project]) -> impl Fn(&Option<String>) -> String + '_ {
    |id| match id {
        None => String::new(),
        Some(id) => projects
            .iter()
            .find(|p| &p.id == id)
            .map_or_else(|| id.clone(), |p| p.name.clone()),
    }
}

fn links(links: &[crate::model::Link]) -> String {
    links
        .iter()
        .map(|l| format!("{} <{}>", l.label, l.url))
        .collect::<Vec<_>>()
        .join(", ")
}

fn require_project(projects: &[Project], id: &Option<String>) -> Result<(), OpError> {
    match id {
        Some(id) if !projects.iter().any(|p| &p.id == id) => Err(OpError::Store(
            StoreError::Invalid("project does not exist".into()),
        )),
        _ => Ok(()),
    }
}

/// The task's version must be the one the call names, or the call is stale
/// already.
fn same_version(task: &Task, version: i64) -> Result<(), OpError> {
    if task.version == version {
        Ok(())
    } else {
        Err(OpError::BadInput(format!(
            "{} is at version {}, and this call names version {version}: it changed since it was read. Read it again.",
            label(task),
            task.version
        )))
    }
}

/// What write `name` would do with `args`, checked as the store would check
/// it. A call that would be refused is an error with the reason.
pub fn preview(store: &Store, name: &str, args: &Value) -> Result<Preview, OpError> {
    let projects = store.projects()?;
    let project_name = names(&projects);
    let (summary, body) = match name {
        "task_create" => {
            let a: CreateTask = input(args)?;
            store::validate_create(&a)?;
            let tags = store::normalize_tags(&a.tags)?;
            require_project(&projects, &a.project_id)?;
            let mut t = Table(Vec::new());
            t.row("Title", String::new(), a.title.clone());
            t.row("State", String::new(), "inbox".into());
            t.row("Project", String::new(), project_name(&a.project_id));
            t.row("Tags", String::new(), tags.join(", "));
            t.row("Links", String::new(), links(&a.links));
            let mut body = format!("A new task in Inbox.\n\n{}", t.render(["Field", "", "New"]));
            if !a.next_action.is_empty() {
                body.push_str(&block("Next action", "markdown", &a.next_action));
            }
            if !a.notes.is_empty() {
                body.push_str(&block("Notes", "markdown", &a.notes));
            }
            (format!("add task “{}”", one_line(&a.title, 80)), body)
        }
        "task_update" => {
            let TaskUpdate { id, task: a } = input(args)?;
            let now = store.task(&id)?;
            same_version(&now, a.version)?;
            store::validate_update(&a)?;
            if now.jira.is_none()
                && a.title_override
                    .as_deref()
                    .is_none_or(|s| s.trim().is_empty())
            {
                return Err(StoreError::Invalid("manual task title is required".into()).into());
            }
            require_project(&projects, &a.project_id)?;
            let tags = store::normalize_tags(&a.tags)?;
            let mut t = Table(Vec::new());
            t.row(
                "Personal title",
                opt(&now.title_override),
                opt(&a.title_override),
            );
            t.row("State", now.state.clone(), a.state.clone());
            t.row("Today", yes_no(now.today), yes_no(a.today));
            t.row("Waiting for", opt(&now.waiting_for), opt(&a.waiting_for));
            t.row("Review on", opt(&now.review_on), opt(&a.review_on));
            t.row(
                "Project",
                project_name(&now.project_id),
                project_name(&a.project_id),
            );
            t.row("Tags", now.tags.join(", "), tags.join(", "));
            t.row("Links", links(&now.links), links(&a.links));
            let mut body = if t.0.is_empty() {
                String::new()
            } else {
                t.render(["Field", "Now", "Proposed"])
            };
            for (title, before, after) in [
                ("Next action", &now.next_action, &a.next_action),
                ("Notes", &now.notes, &a.notes),
            ] {
                if before != after {
                    body.push_str(&block(title, "diff", &diff(before, after)));
                }
            }
            if body.is_empty() {
                body = "No field changes.\n".into();
            }
            (format!("update task {}", label(&now)), body)
        }
        "task_triage" => {
            let a: TriageInput = input(args)?;
            if !["next", "later", "done"].contains(&a.state.as_str()) {
                return Err(StoreError::Invalid(
                    "triage state must be next, later, or done".into(),
                )
                .into());
            }
            store::validate_optional_date(a.review_on.as_deref())?;
            store::validate_selection(&a.items)?;
            let mut rows =
                String::from("| Task | State | Today | Review on |\n|---|---|---|---|\n");
            for item in &a.items {
                let now = store.task(&item.id)?;
                same_version(&now, item.version)?;
                let today = now.today && a.state == "next";
                let arrow = |b: String, c: String| {
                    if b == c {
                        cell(&b)
                    } else {
                        format!("{} → {}", cell(&b), cell(&c))
                    }
                };
                rows.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    cell(&label(&now)),
                    arrow(now.state.clone(), a.state.clone()),
                    arrow(yes_no(now.today), yes_no(today)),
                    arrow(opt(&now.review_on), opt(&a.review_on)),
                ));
            }
            (
                format!("move {} to {}", count(a.items.len()), a.state),
                rows,
            )
        }
        "task_assign_project" => {
            let a: AssignProject = input(args)?;
            store::validate_selection(&a.items)?;
            require_project(&projects, &a.project_id)?;
            let mut rows = String::from("| Task | Project now | Proposed |\n|---|---|---|\n");
            for item in &a.items {
                let now = store.task(&item.id)?;
                same_version(&now, item.version)?;
                let current = match (now.project_source.as_deref(), &now.effective_project_id) {
                    (Some("jira"), id) => format!("{} (by Jira key)", project_name(id)),
                    (_, id) => project_name(id),
                };
                rows.push_str(&format!(
                    "| {} | {} | {} |\n",
                    cell(&label(&now)),
                    cell(&current),
                    cell(&project_name(&a.project_id)),
                ));
            }
            let summary = match &a.project_id {
                Some(_) => format!(
                    "assign {} to project “{}”",
                    count(a.items.len()),
                    project_name(&a.project_id)
                ),
                None => format!("clear the project of {}", count(a.items.len())),
            };
            (summary, rows)
        }
        "project_create" => {
            let a: CreateProject = input(args)?;
            let name = store::validate_project_name(&a.name)?;
            store::validate_optional_date(a.target_on.as_deref())?;
            let keys = store::normalize_jira_keys(&a.jira_keys)?;
            store.check_project(None, &name, &keys)?;
            let mut t = Table(Vec::new());
            t.row("Name", String::new(), name.clone());
            t.row("Target", String::new(), opt(&a.target_on));
            t.row("Jira keys", String::new(), keys.join(", "));
            t.row(
                "Priority",
                String::new(),
                format!("{} of {}", projects.len() + 1, projects.len() + 1),
            );
            (
                format!("add project “{}”", one_line(&name, 80)),
                t.render(["Field", "", "New"]),
            )
        }
        "project_update" => {
            let ProjectUpdate { id, project: a } = input(args)?;
            let now = project(store, &id)?;
            if now.version != a.version {
                return Err(OpError::BadInput(format!(
                    "project “{}” is at version {}, and this call names version {}: it changed since it was read. Read it again.",
                    now.name, now.version, a.version
                )));
            }
            let name = store::validate_project_name(&a.name)?;
            store::validate_optional_date(a.target_on.as_deref())?;
            let keys = store::normalize_jira_keys(&a.jira_keys)?;
            store.check_project(Some(&id), &name, &keys)?;
            let mut t = Table(Vec::new());
            t.row("Name", now.name.clone(), name);
            t.row("Target", opt(&now.target_on), opt(&a.target_on));
            t.row("Jira keys", now.jira_keys.join(", "), keys.join(", "));
            let body = if t.0.is_empty() {
                "No field changes.\n".into()
            } else {
                t.render(["Field", "Now", "Proposed"])
            };
            (
                format!("update project “{}”", one_line(&now.name, 80)),
                body,
            )
        }
        "project_delete" => {
            let ProjectDelete { id, project: a } = input(args)?;
            let now = project(store, &id)?;
            if now.version != a.version {
                return Err(OpError::BadInput(format!(
                    "project “{}” is at version {}, and this call names version {}: it changed since it was read. Read it again.",
                    now.name, now.version, a.version
                )));
            }
            let tasks = all_tasks(store)?;
            let chose: Vec<&Task> = tasks
                .iter()
                .filter(|t| t.project_id.as_deref() == Some(id.as_str()))
                .collect();
            let matched = tasks
                .iter()
                .filter(|t| {
                    t.project_id.is_none() && t.effective_project_id.as_deref() == Some(&id)
                })
                .count();
            let mut body = format!(
                "Deletes project “{}” (priority {} of {}; Jira keys: {}).\n",
                now.name,
                now.position,
                projects.len(),
                cell(&now.jira_keys.join(", ")),
            );
            if chose.is_empty() {
                body.push_str("\nNo task chose it.\n");
            } else {
                body.push_str(&format!("\n{} lose the choice:\n\n", count(chose.len())));
                for t in &chose {
                    body.push_str(&format!("- {}\n", label(t)));
                }
            }
            if matched > 0 {
                body.push_str(&format!(
                    "\n{} matched by its Jira keys become unassigned.\n",
                    count(matched)
                ));
            }
            (
                format!("delete project “{}”", one_line(&now.name, 80)),
                body,
            )
        }
        "project_order" => {
            let a: ProjectOrder = input(args)?;
            let known: std::collections::HashSet<&str> =
                projects.iter().map(|p| p.id.as_str()).collect();
            let asked: std::collections::HashSet<&str> = a.ids.iter().map(String::as_str).collect();
            if asked.len() != a.ids.len() || asked != known {
                return Err(StoreError::Invalid(
                    "project order must list every project exactly once".into(),
                )
                .into());
            }
            let mut rows = String::from("| # | Now | Proposed |\n|---|---|---|\n");
            for (i, (now, next)) in projects.iter().zip(&a.ids).enumerate() {
                rows.push_str(&format!(
                    "| {} | {} | {} |\n",
                    i + 1,
                    cell(&now.name),
                    cell(&project_name(&Some(next.clone()))),
                ));
            }
            ("reorder projects".into(), rows)
        }
        other if ops::find(other).is_some_and(|op| !op.write) => {
            return Err(OpError::BadInput(format!("{other} only reads")));
        }
        other => return Err(OpError::Unknown(other.into())),
    };
    Ok(Preview {
        summary,
        body,
        token: token(store, name, args)?,
    })
}

fn count(n: usize) -> String {
    if n == 1 {
        "1 task".into()
    } else {
        format!("{n} tasks")
    }
}
