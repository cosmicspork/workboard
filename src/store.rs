use crate::model::{
    AssignProject, Board, CreateProject, CreateTask, Identity, JiraFact, Link, ParentIssue,
    Project, SourceIssue, Sync, Task, TriageInput, UpdateProject, UpdateTask,
};
use anyhow::{Context, anyhow};
use chrono::NaiveDate;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use url::Url;
use uuid::Uuid;

pub struct Store {
    connection: Mutex<Connection>,
    site: Url,
}

#[derive(Debug)]
pub enum StoreError {
    Invalid(String),
    IdentityChanged,
    NotFound,
    Conflict,
    Internal(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "{message}"),
            Self::IdentityChanged => {
                f.write_str("Jira account differs from the account bound to this workboard")
            }
            Self::NotFound => f.write_str("not found"),
            Self::Conflict => f.write_str("record was changed by another request"),
            Self::Internal(message) => write!(f, "{message}"),
        }
    }
}
impl std::error::Error for StoreError {}

const MAX_TAGS: usize = 20;
const MAX_PROJECT_KEYS: usize = 50;

const TASK_SELECT: &str = "SELECT t.id,t.jira_issue_id,t.title_override,t.next_action,t.notes,t.state,t.today,t.waiting_for,t.review_on,t.links_json,t.created_at,t.updated_at,t.version,j.id,j.key,j.summary,j.status_name,j.status_category,j.priority,j.issue_type,j.last_seen_at,j.present_in_latest,t.project_id,j.labels_json,j.parent_key,j.parent_summary,j.parent_type FROM tasks t LEFT JOIN jira_issues j ON j.id=t.jira_issue_id";

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE jira_issues(id TEXT PRIMARY KEY,key TEXT NOT NULL,summary TEXT NOT NULL,status_name TEXT NOT NULL,status_category TEXT NOT NULL CHECK(status_category IN ('new','indeterminate','done')),priority TEXT,issue_type TEXT,last_seen_at TEXT NOT NULL,present_in_latest INTEGER NOT NULL CHECK(present_in_latest IN (0,1))); CREATE TABLE tasks(id TEXT PRIMARY KEY,jira_issue_id TEXT UNIQUE REFERENCES jira_issues(id),title_override TEXT,next_action TEXT NOT NULL DEFAULT '',notes TEXT NOT NULL DEFAULT '',state TEXT NOT NULL CHECK(state IN ('inbox','next','waiting','later','done')),today INTEGER NOT NULL DEFAULT 0 CHECK(today IN (0,1)),waiting_for TEXT,review_on TEXT,links_json TEXT NOT NULL DEFAULT '[]',created_at TEXT NOT NULL,updated_at TEXT NOT NULL,version INTEGER NOT NULL DEFAULT 1 CHECK(version>0),CHECK(today=0 OR state='next'),CHECK(jira_issue_id IS NOT NULL OR (title_override IS NOT NULL AND length(trim(title_override))>0))); CREATE TABLE jira_sync(singleton INTEGER PRIMARY KEY CHECK(singleton=1),site TEXT,account_email TEXT,last_attempt_at TEXT,last_success_at TEXT,error_code TEXT,issue_count INTEGER NOT NULL DEFAULT 0,open_count INTEGER NOT NULL DEFAULT 0,done_count INTEGER NOT NULL DEFAULT 0); INSERT INTO jira_sync(singleton) VALUES(1);",
    "ALTER TABLE jira_issues ADD COLUMN labels_json TEXT NOT NULL DEFAULT '[]'; ALTER TABLE jira_issues ADD COLUMN parent_key TEXT; ALTER TABLE jira_issues ADD COLUMN parent_summary TEXT; ALTER TABLE jira_issues ADD COLUMN parent_type TEXT; CREATE TABLE projects(id TEXT PRIMARY KEY,name TEXT NOT NULL UNIQUE COLLATE NOCASE,position INTEGER NOT NULL UNIQUE,target_on TEXT,jira_keys_json TEXT NOT NULL DEFAULT '[]',created_at TEXT NOT NULL,updated_at TEXT NOT NULL,version INTEGER NOT NULL DEFAULT 1 CHECK(version>0)); ALTER TABLE tasks ADD COLUMN project_id TEXT REFERENCES projects(id); CREATE TABLE task_tags(task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,tag TEXT NOT NULL,PRIMARY KEY(task_id,tag)); CREATE INDEX task_tags_tag ON task_tags(tag);",
];

impl Store {
    pub fn init(dir: &Path, site: &Url) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating data directory {}", dir.display()))?;
        set_mode(dir, 0o700)?;
        let path = dir.join("workboard.sqlite3");
        let mut connection = Connection::open(&path).context("opening workboard database")?;
        set_mode(&path, 0o600)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        migrate(&mut connection)?;
        // SQLite creates sidecars lazily. Restrict existing sidecars whenever the store opens.
        for suffix in ["-wal", "-shm"] {
            let sidecar = PathBuf::from(format!("{}{}", path.display(), suffix));
            if sidecar.exists() {
                set_mode(&sidecar, 0o600)?;
            }
        }
        Ok(Self {
            connection: Mutex::new(connection),
            site: site.clone(),
        })
    }

    pub fn board(&self, local_date: String, refresh_in_progress: bool) -> anyhow::Result<Board> {
        validate_date(&local_date).map_err(|e| anyhow!(e))?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow!("database lock poisoned"))?;
        let mut statement =
            connection.prepare(&format!("{TASK_SELECT} ORDER BY t.created_at,t.id"))?;
        let mut tasks = statement
            .query_map([], |row| task_from_row(row, &self.site))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let projects = read_projects(&connection)?;
        hydrate(&connection, &mut tasks, &projects)?;
        let sync = read_sync(&connection, refresh_in_progress)?;
        Ok(Board {
            local_date,
            projects,
            tasks,
            sync,
        })
    }

    pub fn sync(&self) -> anyhow::Result<Sync> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow!("database lock poisoned"))?;
        read_sync(&connection, false)
    }

    pub fn create(&self, input: CreateTask) -> Result<Task, StoreError> {
        validate_create(&input)?;
        let tags = normalize_tags(&input.tags)?;
        let id = Uuid::new_v4().to_string();
        let now = now_utc();
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(internal)?;
        require_project(&transaction, input.project_id.as_deref())?;
        let links =
            serde_json::to_string(&input.links).map_err(|e| StoreError::Internal(e.to_string()))?;
        transaction.execute("INSERT INTO tasks(id,title_override,next_action,notes,state,today,links_json,created_at,updated_at,project_id) VALUES(?1,?2,?3,?4,'inbox',0,?5,?6,?6,?7)", params![id,input.title,input.next_action,input.notes,links,now,input.project_id]).map_err(internal)?;
        write_tags(&transaction, &id, &tags)?;
        let task = load_task(&transaction, &id, &self.site)?.ok_or(StoreError::NotFound)?;
        transaction.commit().map_err(internal)?;
        Ok(task)
    }

    pub fn task(&self, id: &str) -> Result<Task, StoreError> {
        let connection = self.lock()?;
        load_task(&connection, id, &self.site)?.ok_or(StoreError::NotFound)
    }

    pub fn update(&self, id: &str, input: UpdateTask) -> Result<Task, StoreError> {
        validate_update(&input)?;
        let tags = normalize_tags(&input.tags)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(internal)?;
        let existing = task_local(&transaction, id)
            .map_err(internal)?
            .ok_or(StoreError::NotFound)?;
        if existing.0 != input.version {
            return Err(StoreError::Conflict);
        }
        if existing.1.is_none()
            && input
                .title_override
                .as_deref()
                .is_none_or(|s| s.trim().is_empty())
        {
            return Err(StoreError::Invalid("manual task title is required".into()));
        }
        require_project(&transaction, input.project_id.as_deref())?;
        let now = now_utc();
        let links =
            serde_json::to_string(&input.links).map_err(|e| StoreError::Internal(e.to_string()))?;
        let changed = transaction.execute("UPDATE tasks SET title_override=?1,next_action=?2,notes=?3,state=?4,today=?5,waiting_for=?6,review_on=?7,links_json=?8,updated_at=?9,project_id=?10,version=version+1 WHERE id=?11 AND version=?12", params![input.title_override,input.next_action,input.notes,input.state,input.today as i64,input.waiting_for,input.review_on,links,now,input.project_id,id,input.version]).map_err(internal)?;
        if changed != 1 {
            return Err(StoreError::Conflict);
        }
        write_tags(&transaction, id, &tags)?;
        let task = load_task(&transaction, id, &self.site)?.ok_or(StoreError::NotFound)?;
        transaction.commit().map_err(internal)?;
        Ok(task)
    }

    pub fn triage(&self, input: TriageInput) -> Result<Vec<Task>, StoreError> {
        if !["next", "later", "done"].contains(&input.state.as_str()) {
            return Err(StoreError::Invalid(
                "triage state must be next, later, or done".into(),
            ));
        }
        validate_optional_date(input.review_on.as_deref())?;
        validate_selection(&input.items)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(internal)?;
        check_versions(&transaction, &input.items)?;
        let now = now_utc();
        for item in &input.items {
            transaction.execute("UPDATE tasks SET state=?1,today=CASE WHEN ?1='next' THEN today ELSE 0 END,review_on=?2,updated_at=?3,version=version+1 WHERE id=?4 AND version=?5", params![input.state,input.review_on,now,item.id,item.version]).map_err(internal)?;
        }
        let tasks = load_tasks(&transaction, &input.items, &self.site)?;
        transaction.commit().map_err(internal)?;
        Ok(tasks)
    }

    pub fn assign_project(&self, input: AssignProject) -> Result<Vec<Task>, StoreError> {
        validate_selection(&input.items)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(internal)?;
        require_project(&transaction, input.project_id.as_deref())?;
        check_versions(&transaction, &input.items)?;
        let now = now_utc();
        for item in &input.items {
            transaction.execute("UPDATE tasks SET project_id=?1,updated_at=?2,version=version+1 WHERE id=?3 AND version=?4", params![input.project_id,now,item.id,item.version]).map_err(internal)?;
        }
        let tasks = load_tasks(&transaction, &input.items, &self.site)?;
        transaction.commit().map_err(internal)?;
        Ok(tasks)
    }

    pub fn projects(&self) -> Result<Vec<Project>, StoreError> {
        let connection = self.lock()?;
        read_projects(&connection).map_err(internal)
    }

    pub fn create_project(&self, input: CreateProject) -> Result<Project, StoreError> {
        let name = validate_project_name(&input.name)?;
        validate_optional_date(input.target_on.as_deref())?;
        let keys = normalize_jira_keys(&input.jira_keys)?;
        let id = Uuid::new_v4().to_string();
        let now = now_utc();
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(internal)?;
        check_project_conflicts(&transaction, None, &name, &keys)?;
        let position: i64 = transaction
            .query_row(
                "SELECT COALESCE(MAX(position),0)+1 FROM projects",
                [],
                |r| r.get(0),
            )
            .map_err(internal)?;
        let keys_json = to_json(&keys)?;
        transaction.execute("INSERT INTO projects(id,name,position,target_on,jira_keys_json,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?6)", params![id,name,position,input.target_on,keys_json,now]).map_err(internal)?;
        let project = load_project(&transaction, &id)?.ok_or(StoreError::NotFound)?;
        transaction.commit().map_err(internal)?;
        Ok(project)
    }

    pub fn update_project(&self, id: &str, input: UpdateProject) -> Result<Project, StoreError> {
        let name = validate_project_name(&input.name)?;
        validate_optional_date(input.target_on.as_deref())?;
        let keys = normalize_jira_keys(&input.jira_keys)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(internal)?;
        let current = load_project(&transaction, id)?.ok_or(StoreError::NotFound)?;
        if current.version != input.version {
            return Err(StoreError::Conflict);
        }
        check_project_conflicts(&transaction, Some(id), &name, &keys)?;
        let keys_json = to_json(&keys)?;
        transaction.execute("UPDATE projects SET name=?1,target_on=?2,jira_keys_json=?3,updated_at=?4,version=version+1 WHERE id=?5 AND version=?6", params![name,input.target_on,keys_json,now_utc(),id,input.version]).map_err(internal)?;
        let project = load_project(&transaction, id)?.ok_or(StoreError::NotFound)?;
        transaction.commit().map_err(internal)?;
        Ok(project)
    }

    pub fn order_projects(&self, ids: &[String]) -> Result<Vec<Project>, StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(internal)?;
        let existing: HashSet<String> = read_projects(&transaction)
            .map_err(internal)?
            .into_iter()
            .map(|p| p.id)
            .collect();
        let requested: HashSet<&String> = ids.iter().collect();
        if requested.len() != ids.len()
            || ids.len() != existing.len()
            || ids.iter().any(|id| !existing.contains(id))
        {
            return Err(StoreError::Invalid(
                "project order must list every project exactly once".into(),
            ));
        }
        // Positions are unique, so move them out of the way before assigning the new order.
        transaction
            .execute("UPDATE projects SET position=-position", [])
            .map_err(internal)?;
        let now = now_utc();
        for (index, id) in ids.iter().enumerate() {
            transaction
                .execute(
                    "UPDATE projects SET position=?1,updated_at=?2 WHERE id=?3",
                    params![index as i64 + 1, now, id],
                )
                .map_err(internal)?;
        }
        let projects = read_projects(&transaction).map_err(internal)?;
        transaction.commit().map_err(internal)?;
        Ok(projects)
    }

    pub fn delete_project(&self, id: &str, version: i64) -> Result<(), StoreError> {
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(internal)?;
        let current = load_project(&transaction, id)?.ok_or(StoreError::NotFound)?;
        if current.version != version {
            return Err(StoreError::Conflict);
        }
        transaction
            .execute(
                "UPDATE tasks SET project_id=NULL,updated_at=?1,version=version+1 WHERE project_id=?2",
                params![now_utc(), id],
            )
            .map_err(internal)?;
        transaction
            .execute("DELETE FROM projects WHERE id=?1", [id])
            .map_err(internal)?;
        transaction.commit().map_err(internal)?;
        Ok(())
    }

    pub fn apply_refresh(
        &self,
        identity: &Identity,
        issues: &[SourceIssue],
        now: &str,
    ) -> Result<Sync, StoreError> {
        validate_nonempty(now, "refresh timestamp")?;
        validate_nonempty(&identity.site, "Jira site identity")?;
        validate_nonempty(&identity.account_email, "Jira account email")?;
        let configured_host = self.site.host_str().unwrap_or_default();
        if !identity.site.eq_ignore_ascii_case(configured_host) {
            return Err(StoreError::IdentityChanged);
        }
        validate_sources(issues)?;
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(internal)?;
        let bound: (Option<String>, Option<String>) = transaction
            .query_row(
                "SELECT site,account_email FROM jira_sync WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(internal)?;
        if bound
            .0
            .as_deref()
            .is_some_and(|site| !site.eq_ignore_ascii_case(&identity.site))
            || bound
                .1
                .as_deref()
                .is_some_and(|email| !email.eq_ignore_ascii_case(&identity.account_email))
        {
            return Err(StoreError::IdentityChanged);
        }
        transaction
            .execute("UPDATE jira_issues SET present_in_latest=0", [])
            .map_err(internal)?;
        for issue in issues {
            let labels = to_json(&issue.labels)?;
            let parent = issue.parent.as_ref();
            transaction.execute("INSERT INTO jira_issues(id,key,summary,status_name,status_category,priority,issue_type,last_seen_at,present_in_latest,labels_json,parent_key,parent_summary,parent_type) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,1,?9,?10,?11,?12) ON CONFLICT(id) DO UPDATE SET key=excluded.key,summary=excluded.summary,status_name=excluded.status_name,status_category=excluded.status_category,priority=excluded.priority,issue_type=excluded.issue_type,last_seen_at=excluded.last_seen_at,present_in_latest=1,labels_json=excluded.labels_json,parent_key=excluded.parent_key,parent_summary=excluded.parent_summary,parent_type=excluded.parent_type", params![issue.id,issue.key,issue.summary,issue.status_name,issue.status_category,issue.priority,issue.issue_type,now,labels,parent.map(|p| &p.key),parent.and_then(|p| p.summary.as_ref()),parent.and_then(|p| p.issue_type.as_ref())]).map_err(internal)?;
            if issue.status_category != "done" {
                transaction.execute("INSERT INTO tasks(id,jira_issue_id,next_action,notes,state,today,links_json,created_at,updated_at) SELECT ?1,?2,'','','inbox',0,'[]',?3,?3 WHERE NOT EXISTS(SELECT 1 FROM tasks WHERE jira_issue_id=?2)", params![Uuid::new_v4().to_string(),issue.id,now]).map_err(internal)?;
            }
        }
        let open_count = issues
            .iter()
            .filter(|i| i.status_category != "done")
            .count() as i64;
        let done_count = issues.len() as i64 - open_count;
        transaction.execute("UPDATE jira_sync SET site=?1,account_email=?2,last_attempt_at=?3,last_success_at=?3,error_code=NULL,issue_count=?4,open_count=?5,done_count=?6 WHERE singleton=1", params![identity.site,identity.account_email,now,issues.len() as i64,open_count,done_count]).map_err(internal)?;
        let sync =
            read_sync(&transaction, false).map_err(|e| StoreError::Internal(e.to_string()))?;
        transaction.commit().map_err(internal)?;
        Ok(sync)
    }

    pub fn record_failure(&self, now: &str, error_code: &str) -> anyhow::Result<Sync> {
        if now.trim().is_empty() || error_code.trim().is_empty() {
            return Err(anyhow!("timestamp and error code are required"));
        }
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow!("database lock poisoned"))?;
        connection.execute(
            "UPDATE jira_sync SET last_attempt_at=?1,error_code=?2 WHERE singleton=1",
            params![now, error_code],
        )?;
        read_sync(&connection, false)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StoreError> {
        self.connection
            .lock()
            .map_err(|_| StoreError::Internal("database lock poisoned".into()))
    }
}

fn set_mode(path: &Path, mode: u32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}
fn migrate(connection: &mut Connection) -> anyhow::Result<()> {
    let supported = MIGRATIONS.len() as i64;
    let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > supported {
        return Err(anyhow!(
            "database schema version {version} is newer than supported version {supported}"
        ));
    }
    for (index, step) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let tx = connection.transaction()?;
        tx.execute_batch(step)
            .with_context(|| format!("applying schema version {}", index + 1))?;
        tx.pragma_update(None, "user_version", index as i64 + 1)?;
        tx.commit()?;
    }
    Ok(())
}
fn validate_nonempty(s: &str, name: &str) -> Result<(), StoreError> {
    if s.trim().is_empty() {
        Err(StoreError::Invalid(format!("{name} must not be empty")))
    } else {
        Ok(())
    }
}
fn validate_date(date: &str) -> Result<(), StoreError> {
    if date.len() != 10 || NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
        return Err(StoreError::Invalid(
            "date must be a valid YYYY-MM-DD calendar date".into(),
        ));
    }
    Ok(())
}
fn validate_optional_date(date: Option<&str>) -> Result<(), StoreError> {
    if let Some(date) = date {
        validate_date(date)?;
    }
    Ok(())
}
fn validate_text(s: &str, field: &str, max: usize) -> Result<(), StoreError> {
    if s.chars().count() > max {
        return Err(StoreError::Invalid(format!(
            "{field} exceeds {max} characters"
        )));
    }
    Ok(())
}
fn validate_links(links: &[Link]) -> Result<(), StoreError> {
    if links.len() > 20 {
        return Err(StoreError::Invalid("at most 20 links are allowed".into()));
    }
    for link in links {
        if link.label.trim().is_empty() {
            return Err(StoreError::Invalid("link label must not be empty".into()));
        }
        validate_text(&link.url, "link URL", 2048)?;
        let url = Url::parse(&link.url)
            .map_err(|_| StoreError::Invalid("link URL must be absolute HTTP or HTTPS".into()))?;
        if !["http", "https"].contains(&url.scheme())
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(StoreError::Invalid(
                "link URL must be HTTP or HTTPS without credentials".into(),
            ));
        }
    }
    Ok(())
}
fn validate_create(input: &CreateTask) -> Result<(), StoreError> {
    validate_nonempty(&input.title, "title")?;
    validate_text(&input.title, "title", 500)?;
    validate_text(&input.next_action, "next action", 16 * 1024)?;
    validate_text(&input.notes, "notes", 16 * 1024)?;
    validate_links(&input.links)
}
fn validate_update(input: &UpdateTask) -> Result<(), StoreError> {
    if input.version < 1 {
        return Err(StoreError::Invalid("version must be positive".into()));
    }
    if !["inbox", "next", "waiting", "later", "done"].contains(&input.state.as_str()) {
        return Err(StoreError::Invalid("invalid task state".into()));
    }
    if input.today && input.state != "next" {
        return Err(StoreError::Invalid(
            "Today is only valid for Next tasks".into(),
        ));
    }
    if let Some(title) = input.title_override.as_deref() {
        validate_nonempty(title, "title")?;
        validate_text(title, "title", 500)?;
    }
    validate_text(&input.next_action, "next action", 16 * 1024)?;
    validate_text(&input.notes, "notes", 16 * 1024)?;
    if let Some(waiting) = input.waiting_for.as_deref() {
        validate_text(waiting, "waiting for", 500)?;
    }
    validate_optional_date(input.review_on.as_deref())?;
    validate_links(&input.links)
}
fn validate_selection(items: &[crate::model::VersionedId]) -> Result<(), StoreError> {
    if items.len() > 200 {
        return Err(StoreError::Invalid(
            "at most 200 tasks may be changed at once".into(),
        ));
    }
    let mut ids = HashSet::new();
    if items.iter().any(|item| !ids.insert(&item.id)) {
        return Err(StoreError::Invalid("duplicate task in selection".into()));
    }
    Ok(())
}
fn check_versions(
    connection: &Connection,
    items: &[crate::model::VersionedId],
) -> Result<(), StoreError> {
    for item in items {
        let version: Option<i64> = connection
            .query_row("SELECT version FROM tasks WHERE id=?1", [&item.id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(internal)?;
        match version {
            None => return Err(StoreError::NotFound),
            Some(version) if version != item.version => return Err(StoreError::Conflict),
            _ => {}
        }
    }
    Ok(())
}
fn validate_sources(issues: &[SourceIssue]) -> Result<(), StoreError> {
    let mut ids = HashSet::new();
    let mut keys = HashSet::new();
    for issue in issues {
        for (field, value) in [
            ("id", &issue.id),
            ("key", &issue.key),
            ("summary", &issue.summary),
            ("status name", &issue.status_name),
            ("status category", &issue.status_category),
        ] {
            validate_nonempty(value, field)?;
        }
        if !["new", "indeterminate", "done"].contains(&issue.status_category.as_str()) {
            return Err(StoreError::Invalid(
                "unsupported Jira status category".into(),
            ));
        }
        if !ids.insert(&issue.id) || !keys.insert(&issue.key) {
            return Err(StoreError::Invalid("duplicate Jira issue ID or key".into()));
        }
        if issue.labels.len() > 100 || issue.labels.iter().any(|l| l.trim().is_empty()) {
            return Err(StoreError::Invalid("invalid Jira labels".into()));
        }
        if let Some(parent) = &issue.parent {
            validate_nonempty(&parent.key, "parent key")?;
        }
    }
    Ok(())
}
fn validate_project_name(name: &str) -> Result<String, StoreError> {
    let name = name.trim();
    validate_nonempty(name, "project name")?;
    validate_text(name, "project name", 200)?;
    Ok(name.to_owned())
}
fn normalize_jira_keys(keys: &[String]) -> Result<Vec<String>, StoreError> {
    if keys.len() > MAX_PROJECT_KEYS {
        return Err(StoreError::Invalid(format!(
            "at most {MAX_PROJECT_KEYS} Jira keys are allowed per project"
        )));
    }
    let mut normalized: Vec<String> = Vec::with_capacity(keys.len());
    for key in keys {
        let key = key.trim().to_ascii_uppercase();
        if key.is_empty() {
            continue;
        }
        let (project, number) = match key.split_once('-') {
            Some((project, number)) => (project, Some(number)),
            None => (key.as_str(), None),
        };
        let valid_project = project.len() <= 20
            && project.starts_with(|c: char| c.is_ascii_uppercase())
            && project
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        let valid_number = number.is_none_or(|n| {
            !n.is_empty() && n.len() <= 12 && n.chars().all(|c| c.is_ascii_digit())
        });
        if !valid_project || !valid_number {
            return Err(StoreError::Invalid(format!(
                "{key} is not a Jira project key (APP) or issue key (APP-264)"
            )));
        }
        if !normalized.contains(&key) {
            normalized.push(key);
        }
    }
    Ok(normalized)
}
fn check_project_conflicts(
    connection: &Connection,
    id: Option<&str>,
    name: &str,
    keys: &[String],
) -> Result<(), StoreError> {
    for project in read_projects(connection).map_err(internal)? {
        if Some(project.id.as_str()) == id {
            continue;
        }
        if project.name.eq_ignore_ascii_case(name) {
            return Err(StoreError::Invalid(format!(
                "a project named {} already exists",
                project.name
            )));
        }
        if let Some(key) = keys.iter().find(|key| project.jira_keys.contains(key)) {
            return Err(StoreError::Invalid(format!(
                "{key} already belongs to project {}",
                project.name
            )));
        }
    }
    Ok(())
}
fn normalize_tags(tags: &[String]) -> Result<Vec<String>, StoreError> {
    let mut normalized: Vec<String> = Vec::with_capacity(tags.len());
    for tag in tags {
        let tag = tag.trim().to_lowercase();
        if tag.is_empty() {
            continue;
        }
        let part_valid = |part: &str| {
            part.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
                && part.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
                })
        };
        let valid = tag.len() <= 64
            && match tag.split_once(':') {
                Some((key, value)) => part_valid(key) && part_valid(value),
                None => part_valid(&tag),
            };
        if !valid {
            return Err(StoreError::Invalid(format!(
                "tag {tag:?} must be a name or key:value using letters, digits, '.', '_' or '-'"
            )));
        }
        if !normalized.contains(&tag) {
            normalized.push(tag);
        }
    }
    if normalized.len() > MAX_TAGS {
        return Err(StoreError::Invalid(format!(
            "at most {MAX_TAGS} tags are allowed"
        )));
    }
    Ok(normalized)
}
fn write_tags(connection: &Connection, task_id: &str, tags: &[String]) -> Result<(), StoreError> {
    connection
        .execute("DELETE FROM task_tags WHERE task_id=?1", [task_id])
        .map_err(internal)?;
    for tag in tags {
        connection
            .execute(
                "INSERT INTO task_tags(task_id,tag) VALUES(?1,?2)",
                params![task_id, tag],
            )
            .map_err(internal)?;
    }
    Ok(())
}
fn require_project(connection: &Connection, id: Option<&str>) -> Result<(), StoreError> {
    let Some(id) = id else { return Ok(()) };
    let exists = connection
        .query_row("SELECT 1 FROM projects WHERE id=?1", [id], |_| Ok(()))
        .optional()
        .map_err(internal)?
        .is_some();
    if exists {
        Ok(())
    } else {
        Err(StoreError::Invalid("project does not exist".into()))
    }
}
fn to_json<T: serde::Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|e| StoreError::Internal(e.to_string()))
}
fn now_utc() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn internal(error: rusqlite::Error) -> StoreError {
    StoreError::Internal(error.to_string())
}
fn read_sync(connection: &Connection, refresh_in_progress: bool) -> anyhow::Result<Sync> {
    connection.query_row("SELECT site,account_email,last_attempt_at,last_success_at,error_code,issue_count,open_count,done_count FROM jira_sync WHERE singleton=1", [], |r| Ok(Sync { site:r.get(0)?,account_email:r.get(1)?,last_attempt_at:r.get(2)?,last_success_at:r.get(3)?,error_code:r.get(4)?,issue_count:r.get(5)?,open_count:r.get(6)?,done_count:r.get(7)?,refresh_in_progress })).map_err(Into::into)
}
const PROJECT_SELECT: &str =
    "SELECT id,name,position,target_on,jira_keys_json,created_at,updated_at,version FROM projects";
fn project_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    let keys: String = row.get(4)?;
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        position: row.get(2)?,
        target_on: row.get(3)?,
        jira_keys: parse_json(&keys, 4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        version: row.get(7)?,
    })
}
fn read_projects(connection: &Connection) -> rusqlite::Result<Vec<Project>> {
    connection
        .prepare(&format!("{PROJECT_SELECT} ORDER BY position"))?
        .query_map([], project_from_row)?
        .collect()
}
fn load_project(connection: &Connection, id: &str) -> Result<Option<Project>, StoreError> {
    connection
        .query_row(
            &format!("{PROJECT_SELECT} WHERE id=?1"),
            [id],
            project_from_row,
        )
        .optional()
        .map_err(internal)
}
fn task_local(
    connection: &Connection,
    id: &str,
) -> rusqlite::Result<Option<(i64, Option<String>)>> {
    connection
        .query_row(
            "SELECT version,jira_issue_id FROM tasks WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
}
fn load_task(connection: &Connection, id: &str, site: &Url) -> Result<Option<Task>, StoreError> {
    let Some(task) = connection
        .query_row(&format!("{TASK_SELECT} WHERE t.id=?1"), [id], |r| {
            task_from_row(r, site)
        })
        .optional()
        .map_err(internal)?
    else {
        return Ok(None);
    };
    let mut tasks = [task];
    let projects = read_projects(connection).map_err(internal)?;
    hydrate(connection, &mut tasks, &projects).map_err(internal)?;
    let [task] = tasks;
    Ok(Some(task))
}
fn load_tasks(
    connection: &Connection,
    items: &[crate::model::VersionedId],
    site: &Url,
) -> Result<Vec<Task>, StoreError> {
    items
        .iter()
        .map(|item| load_task(connection, &item.id, site)?.ok_or(StoreError::NotFound))
        .collect()
}
fn hydrate(
    connection: &Connection,
    tasks: &mut [Task],
    projects: &[Project],
) -> rusqlite::Result<()> {
    let mut tags: HashMap<String, Vec<String>> = HashMap::new();
    if tasks.len() == 1 {
        let mut statement =
            connection.prepare("SELECT tag FROM task_tags WHERE task_id=?1 ORDER BY rowid")?;
        let rows = statement
            .query_map([&tasks[0].id], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        tags.insert(tasks[0].id.clone(), rows);
    } else {
        let mut statement =
            connection.prepare("SELECT task_id,tag FROM task_tags ORDER BY task_id,rowid")?;
        let rows = statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))?;
        for row in rows {
            let (task_id, tag) = row?;
            tags.entry(task_id).or_default().push(tag);
        }
    }
    let by_key = |key: &str| {
        projects
            .iter()
            .find(|p| p.jira_keys.iter().any(|k| k.eq_ignore_ascii_case(key)))
    };
    for task in tasks.iter_mut() {
        task.tags = tags.remove(&task.id).unwrap_or_default();
        let Some(jira) = &task.jira else {
            task.effective_project_id = task.project_id.clone();
            task.project_source = task.project_id.as_ref().map(|_| "manual".into());
            continue;
        };
        let prefix = jira.key.split('-').next().unwrap_or_default();
        let mut derived = vec![format!("jira:{}", prefix.to_lowercase())];
        if let Some(parent) = &jira.parent {
            let kind = if parent.issue_type.as_deref() == Some("Epic") {
                "epic"
            } else {
                "parent"
            };
            derived.push(format!("{kind}:{}", parent.key.to_lowercase()));
        }
        derived.extend(
            jira.labels
                .iter()
                .map(|l| format!("label:{}", l.to_lowercase())),
        );
        task.jira_tags = derived;
        if task.project_id.is_some() {
            task.effective_project_id = task.project_id.clone();
            task.project_source = Some("manual".into());
        } else if let Some(project) = by_key(&jira.key)
            .or_else(|| jira.parent.as_ref().and_then(|p| by_key(&p.key)))
            .or_else(|| by_key(prefix))
        {
            task.effective_project_id = Some(project.id.clone());
            task.project_source = Some("jira".into());
        }
    }
    Ok(())
}
fn parse_json<T: serde::de::DeserializeOwned>(raw: &str, column: usize) -> rusqlite::Result<T> {
    serde_json::from_str(raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}
fn task_from_row(row: &rusqlite::Row<'_>, site: &Url) -> rusqlite::Result<Task> {
    let id: String = row.get(0)?;
    let jira_issue_id: Option<String> = row.get(1)?;
    let title_override: Option<String> = row.get(2)?;
    let links_json: String = row.get(9)?;
    let jira = if let Some(source_id) = row.get::<_, Option<String>>(13)? {
        let key: String = row.get(14)?;
        let mut url = site.clone();
        url.path_segments_mut()
            .expect("configured site must be hierarchical")
            .push("browse")
            .push(&key);
        let labels: String = row.get(23)?;
        let parent = row
            .get::<_, Option<String>>(24)?
            .map(|key| -> rusqlite::Result<ParentIssue> {
                Ok(ParentIssue {
                    key,
                    summary: row.get(25)?,
                    issue_type: row.get(26)?,
                })
            })
            .transpose()?;
        Some(JiraFact {
            id: source_id,
            key,
            summary: row.get(15)?,
            status_name: row.get(16)?,
            status_category: row.get(17)?,
            priority: row.get(18)?,
            issue_type: row.get(19)?,
            url: url.to_string(),
            last_seen_at: row.get(20)?,
            present_in_latest: row.get::<_, i64>(21)? != 0,
            labels: parse_json(&labels, 23)?,
            parent,
        })
    } else {
        None
    };
    let title = title_override
        .clone()
        .or_else(|| jira.as_ref().map(|j| j.summary.clone()))
        .unwrap_or_default();
    Ok(Task {
        id,
        jira_issue_id,
        title_override,
        title,
        next_action: row.get(3)?,
        notes: row.get(4)?,
        state: row.get(5)?,
        today: row.get::<_, i64>(6)? != 0,
        waiting_for: row.get(7)?,
        review_on: row.get(8)?,
        links: parse_json(&links_json, 9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        version: row.get(12)?,
        jira,
        project_id: row.get(22)?,
        effective_project_id: None,
        project_source: None,
        tags: Vec::new(),
        jira_tags: Vec::new(),
    })
}
