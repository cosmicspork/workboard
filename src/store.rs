use crate::model::{
    Board, CreateTask, Identity, JiraFact, Link, SourceIssue, Sync, Task, TriageInput, UpdateTask,
};
use anyhow::{Context, anyhow};
use chrono::NaiveDate;
use rusqlite::{Connection, OptionalExtension, params};
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
            Self::NotFound => f.write_str("task not found"),
            Self::Conflict => f.write_str("task was changed by another request"),
            Self::Internal(message) => write!(f, "{message}"),
        }
    }
}
impl std::error::Error for StoreError {}

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
        let mut statement = connection.prepare("SELECT t.id,t.jira_issue_id,t.title_override,t.next_action,t.notes,t.state,t.today,t.waiting_for,t.review_on,t.links_json,t.created_at,t.updated_at,t.version,j.id,j.key,j.summary,j.status_name,j.status_category,j.priority,j.issue_type,j.last_seen_at,j.present_in_latest FROM tasks t LEFT JOIN jira_issues j ON j.id=t.jira_issue_id ORDER BY t.created_at,t.id")?;
        let tasks = statement
            .query_map([], |row| task_from_row(row, &self.site))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let sync = read_sync(&connection, refresh_in_progress)?;
        Ok(Board {
            local_date,
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
        let id = Uuid::new_v4().to_string();
        let now = now_utc();
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(internal)?;
        let links =
            serde_json::to_string(&input.links).map_err(|e| StoreError::Internal(e.to_string()))?;
        transaction.execute("INSERT INTO tasks(id,title_override,next_action,notes,state,today,links_json,created_at,updated_at) VALUES(?1,?2,?3,?4,'inbox',0,?5,?6,?6)", params![id,input.title,input.next_action,input.notes,links,now]).map_err(internal)?;
        let task = load_task(&transaction, &id, &self.site)?.ok_or(StoreError::NotFound)?;
        transaction.commit().map_err(internal)?;
        Ok(task)
    }

    pub fn update(&self, id: &str, input: UpdateTask) -> Result<Task, StoreError> {
        validate_update(&input)?;
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
        let now = now_utc();
        let links =
            serde_json::to_string(&input.links).map_err(|e| StoreError::Internal(e.to_string()))?;
        let changed = transaction.execute("UPDATE tasks SET title_override=?1,next_action=?2,notes=?3,state=?4,today=?5,waiting_for=?6,review_on=?7,links_json=?8,updated_at=?9,version=version+1 WHERE id=?10 AND version=?11", params![input.title_override,input.next_action,input.notes,input.state,input.today as i64,input.waiting_for,input.review_on,links,now,id,input.version]).map_err(internal)?;
        if changed != 1 {
            return Err(StoreError::Conflict);
        }
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
        if input.items.len() > 200 {
            return Err(StoreError::Invalid(
                "at most 200 tasks may be triaged".into(),
            ));
        }
        let mut ids = std::collections::HashSet::new();
        if input.items.iter().any(|item| !ids.insert(&item.id)) {
            return Err(StoreError::Invalid(
                "duplicate task in triage selection".into(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction().map_err(internal)?;
        for item in &input.items {
            let version: Option<i64> = transaction
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
        let now = now_utc();
        for item in &input.items {
            transaction.execute("UPDATE tasks SET state=?1,today=CASE WHEN ?1='next' THEN today ELSE 0 END,review_on=?2,updated_at=?3,version=version+1 WHERE id=?4 AND version=?5", params![input.state,input.review_on,now,item.id,item.version]).map_err(internal)?;
        }
        let mut tasks = Vec::with_capacity(input.items.len());
        for item in &input.items {
            tasks.push(load_task(&transaction, &item.id, &self.site)?.ok_or(StoreError::NotFound)?);
        }
        transaction.commit().map_err(internal)?;
        Ok(tasks)
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
            transaction.execute("INSERT INTO jira_issues(id,key,summary,status_name,status_category,priority,issue_type,last_seen_at,present_in_latest) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,1) ON CONFLICT(id) DO UPDATE SET key=excluded.key,summary=excluded.summary,status_name=excluded.status_name,status_category=excluded.status_category,priority=excluded.priority,issue_type=excluded.issue_type,last_seen_at=excluded.last_seen_at,present_in_latest=1", params![issue.id,issue.key,issue.summary,issue.status_name,issue.status_category,issue.priority,issue.issue_type,now]).map_err(internal)?;
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
    let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > 1 {
        return Err(anyhow!(
            "database schema version {version} is newer than supported version 1"
        ));
    }
    if version == 1 {
        return Ok(());
    }
    let tx = connection.transaction()?;
    tx.execute_batch("CREATE TABLE jira_issues(id TEXT PRIMARY KEY,key TEXT NOT NULL,summary TEXT NOT NULL,status_name TEXT NOT NULL,status_category TEXT NOT NULL CHECK(status_category IN ('new','indeterminate','done')),priority TEXT,issue_type TEXT,last_seen_at TEXT NOT NULL,present_in_latest INTEGER NOT NULL CHECK(present_in_latest IN (0,1))); CREATE TABLE tasks(id TEXT PRIMARY KEY,jira_issue_id TEXT UNIQUE REFERENCES jira_issues(id),title_override TEXT,next_action TEXT NOT NULL DEFAULT '',notes TEXT NOT NULL DEFAULT '',state TEXT NOT NULL CHECK(state IN ('inbox','next','waiting','later','done')),today INTEGER NOT NULL DEFAULT 0 CHECK(today IN (0,1)),waiting_for TEXT,review_on TEXT,links_json TEXT NOT NULL DEFAULT '[]',created_at TEXT NOT NULL,updated_at TEXT NOT NULL,version INTEGER NOT NULL DEFAULT 1 CHECK(version>0),CHECK(today=0 OR state='next'),CHECK(jira_issue_id IS NOT NULL OR (title_override IS NOT NULL AND length(trim(title_override))>0))); CREATE TABLE jira_sync(singleton INTEGER PRIMARY KEY CHECK(singleton=1),site TEXT,account_email TEXT,last_attempt_at TEXT,last_success_at TEXT,error_code TEXT,issue_count INTEGER NOT NULL DEFAULT 0,open_count INTEGER NOT NULL DEFAULT 0,done_count INTEGER NOT NULL DEFAULT 0); INSERT INTO jira_sync(singleton) VALUES(1); PRAGMA user_version=1;")?;
    tx.commit()?;
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
fn validate_sources(issues: &[SourceIssue]) -> Result<(), StoreError> {
    let mut ids = std::collections::HashSet::new();
    let mut keys = std::collections::HashSet::new();
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
    }
    Ok(())
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
    connection.query_row("SELECT t.id,t.jira_issue_id,t.title_override,t.next_action,t.notes,t.state,t.today,t.waiting_for,t.review_on,t.links_json,t.created_at,t.updated_at,t.version,j.id,j.key,j.summary,j.status_name,j.status_category,j.priority,j.issue_type,j.last_seen_at,j.present_in_latest FROM tasks t LEFT JOIN jira_issues j ON j.id=t.jira_issue_id WHERE t.id=?1",[id],|r|task_from_row(r,site)).optional().map_err(internal)
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
        })
    } else {
        None
    };
    let title = title_override
        .clone()
        .or_else(|| jira.as_ref().map(|j| j.summary.clone()))
        .unwrap_or_default();
    let links = serde_json::from_str(&links_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(9, rusqlite::types::Type::Text, Box::new(error))
    })?;
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
        links,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        version: row.get(12)?,
        jira,
    })
}
