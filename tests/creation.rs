use claw_todo::{AppError, CreateTask, Store};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

fn request(title: &str) -> CreateTask {
    CreateTask {
        title: title.into(),
        category: "personal".into(),
        ..Default::default()
    }
}

async fn setup() -> (tempfile::TempDir, Store, SqliteConnection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("todos.db");
    let store = Store::open(&path).await.unwrap();
    let conn = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .foreign_keys(true),
    )
    .await
    .unwrap();
    (dir, store, conn)
}

async fn counts(conn: &mut SqliteConnection) -> (i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM tasks), (SELECT count(*) FROM history)")
        .fetch_one(conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn creation_preserves_fields_and_records_a_snapshot() {
    let (_dir, store, mut conn) = setup().await;
    let parent = store.create(request("parent")).await.unwrap().task;
    let input = CreateTask {
        title: "提交周报".into(),
        category: "work".into(),
        description: Some("本周进展\n下一步计划".into()),
        project: Some("claw-todo".into()),
        parent_id: Some(parent.id),
        sources: vec!["notes/weekly.md".into(), "https://example.com/task".into()],
        creation_token: Some("weekly-001".into()),
    };
    let result = store.create(input.clone()).await.unwrap();
    assert!(!result.deduplicated);
    let task = result.task;
    assert!(!task.id.is_empty());
    assert_eq!(task.title, input.title);
    assert_eq!(task.category, input.category);
    assert_eq!(task.description, input.description);
    assert_eq!(task.project, input.project);
    assert_eq!(task.parent_id, input.parent_id);
    assert_eq!(task.sources.0, input.sources);
    assert_eq!(task.creation_token, input.creation_token);
    assert_eq!(task.status, "pending");
    assert_eq!(task.created_at, task.updated_at);
    assert!(chrono::DateTime::parse_from_rfc3339(&task.created_at).is_ok());
    assert!(task.closed_at.is_none());
    assert!(task.blocked_reason.is_none());
    assert!(task.cancel_reason.is_none());
    assert_eq!(store.get(&task.id).await.unwrap(), task);
    let history = store.history(&task.id).await.unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].task_id, task.id);
    assert_eq!(history[0].kind, "created");
    assert_eq!(history[0].at, task.created_at);
    assert_eq!(history[0].changes.0["before"], serde_json::Value::Null);
    assert_eq!(history[0].changes.0["after"], serde_json::json!(task));
    assert_eq!(counts(&mut conn).await, (2, 2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn independent_stores_deduplicate_concurrent_creates_and_reject_conflicts() {
    let (dir, store, mut conn) = setup().await;
    let mut input = request("concurrent");
    input.creation_token = Some("shared".into());
    let mut stores = Vec::new();
    for _ in 0..8 {
        stores.push(Store::open(dir.path().join("todos.db")).await.unwrap());
    }
    let mut jobs = Vec::new();
    for independent in stores {
        let input = input.clone();
        jobs.push(tokio::spawn(async move {
            let result = independent.create(input).await.unwrap();
            independent.close().await;
            result
        }));
    }
    let mut ids = std::collections::HashSet::new();
    let mut created = 0;
    for job in jobs {
        let result = job.await.unwrap();
        ids.insert(result.task.id);
        created += usize::from(!result.deduplicated);
    }
    assert_eq!(ids.len(), 1);
    assert_eq!(created, 1);
    input.title = "different content".into();
    assert!(matches!(
        store.create(input).await.unwrap_err(),
        AppError::CreationTokenConflict { creation_token, task_id }
            if creation_token == "shared" && ids.contains(&task_id)
    ));
    assert_eq!(counts(&mut conn).await, (1, 1));

    let mut left = request("left");
    left.creation_token = Some("collision".into());
    let mut right = left.clone();
    right.title = "right".into();
    let independent = Store::open(dir.path().join("todos.db")).await.unwrap();
    match tokio::join!(store.create(left), independent.create(right)) {
        (Ok(created), Err(AppError::CreationTokenConflict { task_id, .. }))
        | (Err(AppError::CreationTokenConflict { task_id, .. }), Ok(created)) => {
            assert!(!created.deduplicated);
            assert_eq!(created.task.id, task_id);
        }
        results => panic!("Expected one creation and one conflict: {results:?}"),
    }
    independent.close().await;
    assert_eq!(counts(&mut conn).await, (2, 2));
}

#[tokio::test]
async fn retry_uses_original_request_and_returns_current_closed_task() {
    let (_dir, store, mut conn) = setup().await;
    let parent = store.create(request("parent")).await.unwrap().task;
    let mut input = request("original");
    input.parent_id = Some(parent.id);
    input.creation_token = Some("retry-key".into());
    let task = store.create(input.clone()).await.unwrap().task;
    // Fixtures simulate future edit/close operations, including closing the parent.
    sqlx::query("UPDATE tasks SET status = 'done', closed_at = updated_at")
        .execute(&mut conn)
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET title = 'edited' WHERE id = ?")
        .bind(&task.id)
        .execute(&mut conn)
        .await
        .unwrap();
    let current = store.get(&task.id).await.unwrap();
    let retry = store.create(input.clone()).await.unwrap();
    assert!(retry.deduplicated);
    assert_eq!(retry.task, current);
    assert_eq!(retry.task.title, "edited");
    assert_eq!(retry.task.status, "done");
    input.title = "edited".into();
    assert!(matches!(
        store.create(input).await.unwrap_err(),
        AppError::CreationTokenConflict { task_id, .. } if task_id == task.id
    ));
    assert_eq!(store.get(&task.id).await.unwrap(), current);
    assert_eq!(store.history(&task.id).await.unwrap().len(), 1);
    assert_eq!(counts(&mut conn).await, (2, 2));
}

#[tokio::test]
async fn invalid_fields_and_missing_or_closed_ancestors_leave_no_writes() {
    let (_dir, store, mut conn) = setup().await;
    for (field, value) in [
        ("title", ""),
        ("title", " \n\u{3000}"),
        ("category", ""),
        ("category", "unknown"),
        ("creation_token", ""),
        ("creation_token", " \n"),
    ] {
        let mut input = request("valid");
        match field {
            "title" => input.title = value.into(),
            "category" => input.category = value.into(),
            _ => input.creation_token = Some(value.into()),
        }
        assert!(matches!(store.create(input).await.unwrap_err(),
            AppError::InvalidInput { field: actual, .. } if actual == field));
    }
    let mut missing = request("child");
    missing.parent_id = Some("missing".into());
    assert!(matches!(store.create(missing).await.unwrap_err(),
        AppError::NotFound { id, .. } if id == "missing"));
    assert_eq!(counts(&mut conn).await, (0, 0));
    let root = store.create(request("root")).await.unwrap().task;
    let mut child = request("middle");
    child.parent_id = Some(root.id.clone());
    let middle = store.create(child).await.unwrap().task;
    sqlx::query("UPDATE tasks SET status = 'cancelled', closed_at = updated_at WHERE id = ?")
        .bind(&root.id)
        .execute(&mut conn)
        .await
        .unwrap();
    for parent in [&root.id, &middle.id] {
        let mut input = request("rejected");
        input.parent_id = Some(parent.clone());
        assert!(matches!(store.create(input).await.unwrap_err(),
            AppError::ClosedAncestor { parent_id, ancestor_ids }
                if parent_id == *parent && ancestor_ids.contains(&root.id)));
    }
    assert_eq!(counts(&mut conn).await, (2, 2));
}

#[tokio::test]
async fn history_failure_rolls_back_task_and_releases_creation_token() {
    let (_dir, store, mut conn) = setup().await;
    sqlx::query(
        "CREATE TRIGGER fail_created BEFORE INSERT ON history WHEN NEW.kind = 'created' \
                 BEGIN SELECT RAISE(ABORT, 'injected history failure'); END",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    let mut input = request("atomic");
    input.creation_token = Some("reusable-after-rollback".into());
    assert!(matches!(
        store.create(input.clone()).await.unwrap_err(),
        AppError::Database(_)
    ));
    assert_eq!(counts(&mut conn).await, (0, 0));
    sqlx::query("DROP TRIGGER fail_created")
        .execute(&mut conn)
        .await
        .unwrap();
    let result = store.create(input).await.unwrap();
    assert!(!result.deduplicated);
    assert_eq!(counts(&mut conn).await, (1, 1));
}

#[tokio::test]
async fn without_a_shared_token_identical_titles_are_independent_and_missing_ids_fail() {
    let (_dir, store, mut conn) = setup().await;
    let mut ids = std::collections::HashSet::new();
    for creation_token in [None, None, Some("one"), Some("two")] {
        let mut input = request("same title");
        input.creation_token = creation_token.map(str::to_owned);
        let result = store.create(input).await.unwrap();
        assert!(!result.deduplicated);
        ids.insert(result.task.id);
    }
    assert_eq!(ids.len(), 4);
    assert_eq!(counts(&mut conn).await, (4, 4));
    assert!(matches!(store.get("missing").await.unwrap_err(),
        AppError::NotFound { entity: "task", id } if id == "missing"));
    assert!(matches!(store.history("missing").await.unwrap_err(),
        AppError::NotFound { entity: "task", id } if id == "missing"));
}
