//! Writes proposed through the standalone `mcp` mode, waiting on a person,
//! as files: `pending/<id>.json` until one is decided, then
//! `decided/<id>.json` with its outcome. Claiming a proposal is an atomic
//! rename out of `pending/`, so two reviewers cannot both apply it. They
//! live beside the board, in `<data-dir>/proposals/`, outside its database,
//! so the board's schema does not change for them.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::preview::Preview;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Approved,
    Rejected,
    /// The board moved after the proposal was previewed; nothing was written.
    Stale,
    /// The board refused it.
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub status: Status,
    pub decided_at: String,
    pub result: Option<Value>,
    pub reason: Option<String>,
}

impl Outcome {
    pub fn new(status: Status, result: Option<Value>, reason: Option<String>) -> Self {
        Self {
            status,
            decided_at: chrono::Utc::now().to_rfc3339(),
            result,
            reason,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub tool: String,
    pub arguments: Value,
    pub summary: String,
    pub body: String,
    /// The preview's token: the board state the proposal applies against.
    pub token: Option<String>,
    pub created_at: String,
    pub outcome: Option<Outcome>,
}

impl Proposal {
    pub fn new(tool: &str, arguments: Value, shown: Preview) -> Self {
        Self {
            // v7 ids sort in the order they were made.
            id: uuid::Uuid::now_v7().to_string(),
            tool: tool.to_string(),
            arguments,
            summary: shown.summary,
            body: shown.body,
            token: shown.token,
            created_at: chrono::Utc::now().to_rfc3339(),
            outcome: None,
        }
    }

    /// What `proposal_status` answers.
    pub fn status(&self) -> Value {
        match &self.outcome {
            None => json!({ "id": self.id, "status": "pending" }),
            Some(o) => json!({
                "id": self.id,
                "status": o.status,
                "decided_at": o.decided_at,
                "result": o.result,
                "reason": o.reason,
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Proposals {
    root: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Pending,
    Decided,
}

impl Proposals {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, String> {
        let root = root.into();
        for dir in ["pending", "decided"] {
            let dir = root.join(dir);
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700));
            }
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path(&self, place: Place, id: &str) -> Result<PathBuf, String> {
        // An id names a file; it may not name a path.
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!("{id:?} is not a proposal id"));
        }
        let dir = match place {
            Place::Pending => "pending",
            Place::Decided => "decided",
        };
        Ok(self.root.join(dir).join(format!("{id}.json")))
    }

    /// Write `p` whole or not at all: to a temporary file, then renamed.
    fn write(&self, place: Place, p: &Proposal) -> Result<(), String> {
        let path = self.path(place, &p.id)?;
        let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
        let json = serde_json::to_string_pretty(p).map_err(|e| e.to_string())?;
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(json.as_bytes())
            .and_then(|()| f.sync_all())
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn read(path: &Path) -> Result<Proposal, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn put(&self, p: &Proposal) -> Result<(), String> {
        self.write(Place::Pending, p)
    }

    /// The proposals waiting on a reviewer, oldest first.
    pub fn pending(&self) -> Result<Vec<Proposal>, String> {
        let dir = self.root.join("pending");
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        paths.sort();
        Ok(paths.iter().filter_map(|p| Self::read(p).ok()).collect())
    }

    /// A proposal by id, decided or not.
    pub fn get(&self, id: &str) -> Result<Option<Proposal>, String> {
        for place in [Place::Decided, Place::Pending] {
            let path = self.path(place, id)?;
            if path.exists() {
                return Self::read(&path).map(Some);
            }
        }
        Ok(None)
    }

    /// Take a pending proposal for deciding. Only one caller gets it.
    pub fn claim(&self, id: &str) -> Result<Proposal, String> {
        let from = self.path(Place::Pending, id)?;
        let to = self.path(Place::Decided, id)?;
        match std::fs::rename(&from, &to) {
            Ok(()) => Self::read(&to),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(match self.get(id)? {
                Some(_) => format!("proposal {id} was already decided"),
                None => format!("no proposal {id}"),
            }),
            Err(e) => Err(format!("{}: {e}", from.display())),
        }
    }

    /// Write a claimed proposal's outcome.
    pub fn decide(&self, mut p: Proposal, outcome: Outcome) -> Result<Proposal, String> {
        p.outcome = Some(outcome);
        self.write(Place::Decided, &p)?;
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal() -> Proposal {
        Proposal::new(
            "task_create",
            json!({ "title": "x" }),
            Preview {
                summary: "add task “x”".into(),
                body: "…".into(),
                token: None,
            },
        )
    }

    #[test]
    fn a_proposal_is_claimed_once_and_then_reads_as_decided() {
        let dir = tempfile::tempdir().unwrap();
        let store = Proposals::open(dir.path()).unwrap();
        let (a, b) = (proposal(), proposal());
        store.put(&a).unwrap();
        store.put(&b).unwrap();
        let ids: Vec<String> = store.pending().unwrap().into_iter().map(|p| p.id).collect();
        assert_eq!(ids, [a.id.clone(), b.id.clone()]);
        let claimed = store.claim(&a.id).unwrap();
        assert!(store.claim(&a.id).unwrap_err().contains("already decided"));
        store
            .decide(
                claimed,
                Outcome::new(Status::Rejected, None, Some("no".into())),
            )
            .unwrap();
        let decided = store.get(&a.id).unwrap().unwrap();
        assert_eq!(decided.status()["status"], "rejected");
        assert_eq!(store.pending().unwrap().len(), 1);
        assert!(store.claim("zz").unwrap_err().contains("no proposal"));
        assert!(store.get("../x").is_err());
    }
}
