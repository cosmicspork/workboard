use std::sync::Arc;
use tempfile::TempDir;
use url::Url;
use workboard::model::{
    CreateTask, Identity, Link, SourceIssue, TriageInput, UpdateTask, VersionedId,
};
use workboard::store::{Store, StoreError};

fn setup() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(
        dir.path(),
        &Url::parse("https://example.atlassian.net").unwrap(),
    )
    .unwrap();
    (dir, store)
}
fn issue(id: &str, key: &str, category: &str, summary: &str) -> SourceIssue {
    SourceIssue {
        id: id.into(),
        key: key.into(),
        summary: summary.into(),
        status_name: "Oddly Named Status".into(),
        status_category: category.into(),
        priority: Some("Planned".into()),
        issue_type: Some("Task".into()),
    }
}
fn identity() -> Identity {
    Identity {
        site: "example.atlassian.net".into(),
        account_email: "user@example.test".into(),
    }
}
fn refresh(store: &Store, issues: &[SourceIssue]) {
    store
        .apply_refresh(&identity(), issues, "2026-09-29T08:00:00Z")
        .unwrap();
}
fn create(store: &Store, title: &str) -> workboard::model::Task {
    store
        .create(CreateTask {
            title: title.into(),
            next_action: String::new(),
            notes: String::new(),
            links: vec![],
        })
        .unwrap()
}
fn update(task: &workboard::model::Task, state: &str, today: bool) -> UpdateTask {
    UpdateTask {
        version: task.version,
        title_override: task.title_override.clone(),
        next_action: task.next_action.clone(),
        notes: task.notes.clone(),
        state: state.into(),
        today,
        waiting_for: task.waiting_for.clone(),
        review_on: task.review_on.clone(),
        links: task.links.clone(),
    }
}

#[test]
fn refresh_imports_open_categories_once_and_uses_status_category() {
    let (_dir, store) = setup();
    let rows = vec![
        issue("1", "A-1", "new", "One"),
        issue("2", "A-2", "indeterminate", "Two"),
        issue("3", "A-3", "done", "Three"),
    ];
    refresh(&store, &rows);
    let board = store.board("2026-09-29".into(), false).unwrap();
    assert_eq!(
        (
            board.sync.issue_count,
            board.sync.open_count,
            board.sync.done_count
        ),
        (3, 2, 1)
    );
    assert_eq!(board.tasks.len(), 2);
    let titles = board
        .tasks
        .iter()
        .map(|t| t.title.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(titles, std::collections::HashSet::from(["One", "Two"]));
    assert_eq!(
        board
            .tasks
            .iter()
            .find(|t| t.jira.as_ref().unwrap().id == "1")
            .unwrap()
            .jira
            .as_ref()
            .unwrap()
            .url,
        "https://example.atlassian.net/browse/A-1"
    );
    refresh(&store, &rows);
    assert_eq!(
        store.board("2026-09-29".into(), false).unwrap().tasks.len(),
        2
    );
}

#[test]
fn refresh_preserves_personal_data_updates_unoverridden_title_and_marks_absent() {
    let (_dir, store) = setup();
    refresh(
        &store,
        &[
            issue("stable", "A-1", "new", "Original"),
            issue("gone", "A-2", "new", "Gone"),
        ],
    );
    let board = store.board("2026-09-29".into(), false).unwrap();
    let tracked = board
        .tasks
        .iter()
        .find(|t| t.jira.as_ref().unwrap().id == "stable")
        .unwrap();
    let mut edited = update(tracked, "next", true);
    edited.title_override = Some("Personal title".into());
    edited.notes = "keep note".into();
    edited.next_action = "follow up".into();
    let edited = store.update(&tracked.id, edited).unwrap();
    let untouched = board
        .tasks
        .iter()
        .find(|t| t.jira.as_ref().unwrap().id == "gone")
        .unwrap()
        .clone();
    refresh(&store, &[issue("stable", "B-1", "done", "Changed source")]);
    let board = store.board("2026-09-29".into(), false).unwrap();
    let current = board.tasks.iter().find(|t| t.id == edited.id).unwrap();
    assert_eq!(current.title, "Personal title");
    assert_eq!(current.notes, "keep note");
    assert_eq!(current.next_action, "follow up");
    assert_eq!(current.state, "next");
    assert!(current.today);
    assert_eq!(current.version, edited.version);
    assert_eq!(current.jira.as_ref().unwrap().key, "B-1");
    assert_eq!(current.jira.as_ref().unwrap().status_category, "done");
    let absent = board.tasks.iter().find(|t| t.id == untouched.id).unwrap();
    assert!(!absent.jira.as_ref().unwrap().present_in_latest);
    assert_eq!(absent.state, "inbox");
    let cleared = store
        .update(
            &current.id,
            UpdateTask {
                version: current.version,
                title_override: None,
                next_action: current.next_action.clone(),
                notes: current.notes.clone(),
                state: "next".into(),
                today: true,
                waiting_for: None,
                review_on: None,
                links: vec![],
            },
        )
        .unwrap();
    assert_eq!(cleared.title, "Changed source");
}

#[test]
fn empty_refresh_preserves_commitments_and_opening_completed_source_creates_task() {
    let (_dir, store) = setup();
    refresh(&store, &[issue("done", "A-1", "done", "Finished")]);
    assert!(
        store
            .board("2026-09-29".into(), false)
            .unwrap()
            .tasks
            .is_empty()
    );
    refresh(&store, &[issue("done", "A-1", "new", "Reopened source")]);
    let task = store
        .board("2026-09-29".into(), false)
        .unwrap()
        .tasks
        .remove(0);
    let done = store
        .update(&task.id, update(&task, "done", false))
        .unwrap();
    refresh(&store, &[issue("done", "A-1", "new", "Another summary")]);
    let board = store.board("2026-09-29".into(), false).unwrap();
    assert_eq!(board.tasks[0].state, "done");
    refresh(&store, &[]);
    let board = store.board("2026-09-29".into(), false).unwrap();
    assert_eq!(board.tasks.len(), 1);
    assert!(!board.tasks[0].jira.as_ref().unwrap().present_in_latest);
    assert_eq!(board.tasks[0].version, done.version);
    let reopened = store
        .update(&board.tasks[0].id, update(&board.tasks[0], "next", false))
        .unwrap();
    assert_eq!(reopened.state, "next");
}

#[test]
fn invalid_snapshot_and_identity_change_leave_previous_snapshot_untouched() {
    let (_dir, store) = setup();
    refresh(&store, &[issue("1", "A-1", "new", "Original")]);
    let duplicate_id = issue("1", "A-2", "new", "Duplicate ID");
    assert!(matches!(
        store.apply_refresh(
            &identity(),
            &[issue("1", "A-1", "new", "Partial"), duplicate_id],
            "now"
        ),
        Err(StoreError::Invalid(_))
    ));
    let duplicate_key = issue("2", "A-1", "new", "Duplicate key");
    assert!(matches!(
        store.apply_refresh(
            &identity(),
            &[issue("1", "A-1", "new", "Partial"), duplicate_key],
            "now"
        ),
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        store.apply_refresh(
            &identity(),
            &[issue("2", "A-2", "unsupported", "Bad")],
            "now"
        ),
        Err(StoreError::Invalid(_))
    ));
    let wrong = Identity {
        account_email: "other@example.test".into(),
        ..identity()
    };
    assert!(matches!(
        store.apply_refresh(&wrong, &[], "now"),
        Err(StoreError::IdentityChanged)
    ));
    let board = store.board("2026-09-29".into(), false).unwrap();
    assert_eq!(board.tasks.len(), 1);
    assert_eq!(board.tasks[0].title, "Original");
    assert!(board.tasks[0].jira.as_ref().unwrap().present_in_latest);
}

#[test]
fn failed_write_rolls_back_source_presence_and_local_task_creation() {
    let (dir, store) = setup();
    refresh(&store, &[issue("existing", "A-1", "new", "Original")]);
    let connection = rusqlite::Connection::open(dir.path().join("workboard.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_issue BEFORE INSERT ON jira_issues WHEN NEW.id='fail' BEGIN SELECT RAISE(ABORT, 'synthetic write failure'); END;").unwrap();
    drop(connection);
    let outcome = store.apply_refresh(
        &identity(),
        &[
            issue("existing", "A-1", "done", "Changed"),
            issue("fail", "A-2", "new", "Not committed"),
        ],
        "2026-09-30T08:00:00Z",
    );
    assert!(matches!(outcome, Err(StoreError::Internal(_))));
    let board = store.board("2026-09-30".into(), false).unwrap();
    assert_eq!(board.sync.issue_count, 1);
    assert_eq!(board.tasks.len(), 1);
    assert_eq!(board.tasks[0].title, "Original");
    assert_eq!(board.tasks[0].jira.as_ref().unwrap().status_category, "new");
    assert!(board.tasks[0].jira.as_ref().unwrap().present_in_latest);
}

#[test]
fn reassigned_key_keeps_historical_issue_and_tracks_new_id_separately() {
    let (_dir, store) = setup();
    refresh(&store, &[issue("old-id", "A-1", "new", "Historical")]);
    refresh(&store, &[issue("new-id", "A-1", "new", "Current")]);
    let board = store.board("2026-09-29".into(), false).unwrap();
    assert_eq!(board.tasks.len(), 2);
    let historical = board
        .tasks
        .iter()
        .find(|t| t.jira.as_ref().unwrap().id == "old-id")
        .unwrap();
    assert_eq!(historical.jira.as_ref().unwrap().key, "A-1");
    assert!(!historical.jira.as_ref().unwrap().present_in_latest);
    let current = board
        .tasks
        .iter()
        .find(|t| t.jira.as_ref().unwrap().id == "new-id")
        .unwrap();
    assert_eq!(current.jira.as_ref().unwrap().key, "A-1");
    assert!(current.jira.as_ref().unwrap().present_in_latest);
}

#[test]
fn optimistic_write_and_bulk_triage_are_atomic_and_clear_today() {
    let (_dir, store) = setup();
    let a = create(&store, "A");
    let b = create(&store, "B");
    let next = store.update(&a.id, update(&a, "next", true)).unwrap();
    let waiting = store
        .update(&next.id, update(&next, "waiting", false))
        .unwrap();
    assert!(!waiting.today);
    let stale_b = VersionedId {
        id: b.id.clone(),
        version: b.version + 1,
    };
    let input = TriageInput {
        items: vec![
            VersionedId {
                id: waiting.id.clone(),
                version: waiting.version,
            },
            stale_b,
        ],
        state: "later".into(),
        review_on: Some("2026-09-30".into()),
    };
    assert!(matches!(store.triage(input), Err(StoreError::Conflict)));
    let current = store.board("2026-09-29".into(), false).unwrap();
    assert_eq!(
        current
            .tasks
            .iter()
            .find(|t| t.id == waiting.id)
            .unwrap()
            .state,
        "waiting"
    );
    let valid = TriageInput {
        items: vec![VersionedId {
            id: waiting.id.clone(),
            version: waiting.version,
        }],
        state: "later".into(),
        review_on: Some("2026-09-30".into()),
    };
    let result = store.triage(valid).unwrap();
    assert_eq!(result[0].review_on.as_deref(), Some("2026-09-30"));
    assert!(matches!(
        store.update(&a.id, update(&a, "next", true)),
        Err(StoreError::Conflict)
    ));
}

#[test]
fn invalid_dates_and_links_are_rejected_and_text_remains_plain_data() {
    let (_dir, store) = setup();
    let mut task = create(&store, "A");
    let mut input = update(&task, "waiting", false);
    input.review_on = Some("2026-02-30".into());
    assert!(matches!(
        store.update(&task.id, input),
        Err(StoreError::Invalid(_))
    ));
    let mut input = update(&task, "next", false);
    input.links = vec![Link {
        label: "bad".into(),
        url: "javascript:alert(1)".into(),
    }];
    assert!(matches!(
        store.update(&task.id, input),
        Err(StoreError::Invalid(_))
    ));
    task.next_action = "safe text <script> remains plain data".into();
    let saved = store
        .update(&task.id, update(&task, "inbox", false))
        .unwrap();
    assert_eq!(saved.next_action, "safe text <script> remains plain data");
}

#[test]
fn refresh_failure_changes_only_attempt_metadata() {
    let (_dir, store) = setup();
    refresh(&store, &[issue("1", "A-1", "new", "One")]);
    let prior = store.sync().unwrap();
    let failed = store
        .record_failure("2026-09-29T09:00:00Z", "jira_unavailable")
        .unwrap();
    assert_eq!(failed.last_success_at, prior.last_success_at);
    assert_eq!(failed.issue_count, prior.issue_count);
    assert_eq!(failed.error_code.as_deref(), Some("jira_unavailable"));
}

#[test]
fn concurrent_versioned_writes_allow_only_one_winner() {
    let (_dir, store) = setup();
    let task = create(&store, "Concurrent");
    let store = Arc::new(store);
    let mut handles = vec![];
    for _ in 0..2 {
        let store = store.clone();
        let stale = task.clone();
        handles.push(std::thread::spawn(move || {
            store.update(&stale.id, update(&stale, "next", false))
        }));
    }
    let results = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(StoreError::Conflict)))
            .count(),
        1
    );
}
