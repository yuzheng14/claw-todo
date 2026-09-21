use claw_todo::{AppError, CreateTask, EditTask, Store};
use serde_json::json;
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

#[tokio::test]
async fn edits_record_full_snapshots_and_distinguish_omission_clearing_and_noop() {
    let (_dir, store, _conn) = setup().await;
    let before = store.create(request("original")).await.unwrap().task;
    let patch = EditTask {
        title: Some(" 新标题 ".into()),
        category: Some("work".into()),
        description: Some(Some(" \n详细描述\n ".into())),
        project: Some(Some(" project ".into())),
        sources: Some(vec![" notes.md ".into(), "https://example.com".into()]),
    };
    let result = store.edit(&before.id, patch.clone()).await.unwrap();
    assert!(result.changed);
    let mut expected = before.clone();
    expected.title = patch.title.clone().unwrap();
    expected.category = patch.category.clone().unwrap();
    expected.description = patch.description.clone().unwrap();
    expected.project = patch.project.clone().unwrap();
    expected.sources.0 = patch.sources.clone().unwrap();
    expected.updated_at = result.task.updated_at.clone();
    assert!(chrono::DateTime::parse_from_rfc3339(&expected.updated_at).is_ok());
    assert_eq!(result.task, expected);
    assert_eq!(store.get(&before.id).await.unwrap(), expected);
    let history = store.history(&before.id).await.unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].kind, "edited");
    assert_eq!(history[1].at, expected.updated_at);
    assert_eq!(
        history[1].changes.0,
        json!({"before": before, "after": expected})
    );

    for unchanged in [EditTask::default(), patch] {
        let result = store.edit(&before.id, unchanged).await.unwrap();
        assert!(!result.changed);
        assert_eq!(result.task, expected);
        assert_eq!(store.history(&before.id).await.unwrap().len(), 2);
    }
    let cleared = store
        .edit(
            &before.id,
            EditTask {
                description: Some(None),
                project: Some(None),
                sources: Some(vec![]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(cleared.changed);
    assert_eq!(cleared.task.description, None);
    assert_eq!(cleared.task.project, None);
    assert!(cleared.task.sources.0.is_empty());
    assert_eq!(cleared.task.title, expected.title);
    assert_eq!(cleared.task.category, expected.category);
    // 空字符串及全空白描述都是有效内容，不等同于清空为 NULL。
    for description in ["", " \n "] {
        let edited = store
            .edit(
                &before.id,
                EditTask {
                    description: Some(Some(description.into())),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(edited.changed);
        assert_eq!(edited.task.description.as_deref(), Some(description));
    }
}

#[tokio::test]
async fn closed_tasks_remain_editable_without_changing_identity_or_creation_retry() {
    let (_dir, store, mut conn) = setup().await;
    let parent = store.create(request("parent")).await.unwrap().task;
    let mut input = request("original");
    input.parent_id = Some(parent.id);
    input.creation_token = Some("creation-key".into());
    let task = store.create(input.clone()).await.unwrap().task;
    // 后续批次才实现关闭操作；这里模拟父子都已关闭。
    sqlx::query(
        "UPDATE tasks SET status = 'cancelled', cancel_reason = '已取消', closed_at = updated_at",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    let before = store.get(&task.id).await.unwrap();
    let result = store
        .edit(
            &task.id,
            EditTask {
                title: Some("after closing".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut expected = before;
    expected.title = "after closing".into();
    expected.updated_at = result.task.updated_at.clone();
    assert!(result.changed);
    assert_eq!(result.task, expected);
    let retry = store.create(input).await.unwrap();
    assert!(retry.deduplicated);
    assert_eq!(retry.task, expected);
    assert_eq!(store.history(&task.id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn invalid_edits_and_notes_and_missing_tasks_leave_no_writes() {
    let (_dir, store, _conn) = setup().await;
    let before = store.create(request("valid")).await.unwrap().task;
    for (field, value) in [
        ("title", ""),
        ("title", " \n\u{3000}"),
        ("category", ""),
        ("category", "unknown"),
        ("project", ""),
        ("project", " \n\u{3000}"),
        ("source", ""),
        ("source", " \n\u{3000}"),
    ] {
        let mut patch = EditTask::default();
        match field {
            "title" => patch.title = Some(value.into()),
            "category" => patch.category = Some(value.into()),
            "project" => patch.project = Some(Some(value.into())),
            _ => patch.sources = Some(vec!["valid.md".into(), value.into()]),
        }
        assert!(matches!(store.edit(&before.id, patch).await.unwrap_err(),
            AppError::InvalidInput { field: actual, .. } if actual == field));
    }
    for body in ["", " \n\u{3000}"] {
        assert!(matches!(
            store.note(&before.id, body).await.unwrap_err(),
            AppError::InvalidInput { .. }
        ));
    }
    assert_eq!(store.get(&before.id).await.unwrap(), before);
    assert_eq!(store.history(&before.id).await.unwrap().len(), 1);
    assert!(
        matches!(store.edit("missing", EditTask::default()).await.unwrap_err(),
        AppError::NotFound { entity: "task", id } if id == "missing")
    );
    assert!(matches!(store.note("missing", "body").await.unwrap_err(),
        AppError::NotFound { entity: "task", id } if id == "missing"));
}

#[tokio::test]
async fn work_category_rejects_wechat_reminders_of_every_status_atomically() {
    let (_dir, store, mut conn) = setup().await;
    let before = store.create(request("personal")).await.unwrap().task;
    let other = store.create(request("other")).await.unwrap().task;
    // 只使用 SQL fixture 建提醒，不提前引入下一批的提醒 API。
    for (id, task_id, channel, status) in [
        ("d", &before.id, "wechat", "scheduled"),
        ("c", &before.id, "wechat", "fired"),
        ("b", &before.id, "wechat", "cancelled"),
        ("a", &before.id, "wechat", "unknown"),
        ("email", &before.id, "email", "scheduled"),
        ("other", &other.id, "wechat", "scheduled"),
    ] {
        sqlx::query("INSERT INTO reminders (id, task_id, external_id, scheduled_at, timezone, channel, status, created_at, updated_at) VALUES (?, ?, ?, ?, 'UTC', ?, ?, ?, ?)")
            .bind(id).bind(task_id).bind(id).bind(&before.created_at)
            .bind(channel).bind(status).bind(&before.created_at).bind(&before.created_at)
            .execute(&mut conn).await.unwrap();
    }
    let patch = EditTask {
        title: Some("must not leak".into()),
        category: Some("work".into()),
        ..Default::default()
    };
    assert!(
        matches!(store.edit(&before.id, patch.clone()).await.unwrap_err(),
        AppError::ChannelForbidden { task_id, channel: "wechat", reminder_ids }
            if task_id == before.id && reminder_ids == ["a", "b", "c", "d"])
    );
    assert_eq!(store.get(&before.id).await.unwrap(), before);
    assert_eq!(store.history(&before.id).await.unwrap().len(), 1);
    sqlx::query("DELETE FROM reminders WHERE task_id = ? AND channel = 'wechat'")
        .bind(&before.id)
        .execute(&mut conn)
        .await
        .unwrap();
    let edited = store.edit(&before.id, patch).await.unwrap();
    assert!(edited.changed);
    assert_eq!(edited.task.category, "work");
    assert_eq!(edited.task.title, "must not leak");
}

#[tokio::test]
async fn history_failures_roll_back_both_edits_and_notes() {
    let (_dir, store, mut conn) = setup().await;
    let before = store.create(request("atomic")).await.unwrap().task;
    sqlx::query("CREATE TRIGGER fail_history BEFORE INSERT ON history WHEN NEW.kind IN ('edited', 'note') BEGIN SELECT RAISE(ABORT, 'injected history failure'); END")
        .execute(&mut conn).await.unwrap();
    let patch = EditTask {
        title: Some("changed".into()),
        ..Default::default()
    };
    assert!(matches!(
        store.edit(&before.id, patch.clone()).await.unwrap_err(),
        AppError::Database(_)
    ));
    assert_eq!(store.get(&before.id).await.unwrap(), before);
    assert!(matches!(
        store.note(&before.id, "not committed").await.unwrap_err(),
        AppError::Database(_)
    ));
    assert_eq!(store.get(&before.id).await.unwrap(), before);
    assert_eq!(store.history(&before.id).await.unwrap().len(), 1);
    sqlx::query("DROP TRIGGER fail_history")
        .execute(&mut conn)
        .await
        .unwrap();
    assert!(store.edit(&before.id, patch).await.unwrap().changed);
    assert!(store.note(&before.id, "committed").await.unwrap().changed);
    assert_eq!(store.history(&before.id).await.unwrap().len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_edits_from_independent_stores_preserve_fields_and_history_chain() {
    let (dir, store, _conn) = setup().await;
    let independent = Store::open(dir.path().join("todos.db")).await.unwrap();
    let before = store.create(request("original")).await.unwrap().task;
    let title = EditTask {
        title: Some("new title".into()),
        ..Default::default()
    };
    let project = EditTask {
        project: Some(Some("new project".into())),
        ..Default::default()
    };
    let (left, right) = tokio::join!(
        store.edit(&before.id, title),
        independent.edit(&before.id, project)
    );
    assert!(left.unwrap().changed);
    assert!(right.unwrap().changed);
    let current = store.get(&before.id).await.unwrap();
    assert_eq!(current.title, "new title");
    assert_eq!(current.project.as_deref(), Some("new project"));
    let history = store.history(&before.id).await.unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[1].kind, "edited");
    assert_eq!(history[2].kind, "edited");
    assert!(history[1].id < history[2].id);
    assert_eq!(history[1].changes.0["before"], json!(before));
    assert_eq!(
        history[1].changes.0["after"],
        history[2].changes.0["before"]
    );
    assert_eq!(history[2].changes.0["after"], json!(current));
    let (left, right) = tokio::join!(
        store.note(&before.id, "left note"),
        independent.note(&before.id, "right note")
    );
    assert!(left.unwrap().changed);
    assert!(right.unwrap().changed);
    let history = store.history(&before.id).await.unwrap();
    assert_eq!(history.len(), 5);
    assert_eq!(history[3].kind, "note");
    assert_eq!(history[4].kind, "note");
    let mut bodies = [
        history[3].changes.0["body"].as_str().unwrap(),
        history[4].changes.0["body"].as_str().unwrap(),
    ];
    bodies.sort();
    assert_eq!(bodies, ["left note", "right note"]);
    assert_eq!(history[3].changes.0["before"], json!(current));
    assert_eq!(
        history[3].changes.0["after"],
        history[4].changes.0["before"]
    );
    assert_eq!(
        history[4].changes.0["after"],
        json!(store.get(&before.id).await.unwrap())
    );
    independent.close().await;
}

#[tokio::test]
async fn notes_preserve_body_and_append_again_on_retries_even_for_closed_tasks() {
    let (_dir, store, mut conn) = setup().await;
    let task = store.create(request("notes")).await.unwrap().task;
    sqlx::query("UPDATE tasks SET status = 'done', closed_at = updated_at WHERE id = ?")
        .bind(&task.id)
        .execute(&mut conn)
        .await
        .unwrap();
    let mut before = store.get(&task.id).await.unwrap();
    let body = "  进度备注\n保留换行和空白  ";
    for history_len in [2, 3] {
        let result = store.note(&task.id, body).await.unwrap();
        assert!(result.changed);
        let mut expected = before.clone();
        expected.updated_at = result.task.updated_at.clone();
        assert!(chrono::DateTime::parse_from_rfc3339(&expected.updated_at).is_ok());
        assert_eq!(result.task, expected);
        assert_eq!(store.get(&task.id).await.unwrap(), expected);
        let history = store.history(&task.id).await.unwrap();
        assert_eq!(history.len(), history_len);
        let event = history.last().unwrap();
        assert_eq!(event.kind, "note");
        assert_eq!(event.at, expected.updated_at);
        assert_eq!(
            event.changes.0,
            json!({"body": body, "before": before, "after": expected})
        );
        before = expected;
    }
}
