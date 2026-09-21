use claw_todo::{AppError, CreateTask, Store, Task, Transition};
use serde_json::{Value, json};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::sync::Arc;
use tokio::sync::Barrier;

fn request(title: &str, parent_id: Option<&str>) -> CreateTask {
    CreateTask {
        title: title.into(),
        category: "personal".into(),
        parent_id: parent_id.map(str::to_owned),
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

async fn create(store: &Store, title: &str, parent: Option<&str>) -> Task {
    store.create(request(title, parent)).await.unwrap().task
}

async fn history(store: &Store, id: &str) -> Value {
    serde_json::to_value(store.history(id).await.unwrap()).unwrap()
}

fn cancel(reason: Option<&str>, cascade: bool) -> Transition {
    Transition::Cancel {
        reason: reason.map(str::to_owned),
        cascade,
    }
}

#[tokio::test]
async fn lifecycle_records_snapshots_and_repeated_actions_are_noops() {
    let (_dir, store, _conn) = setup().await;
    let mut before = create(&store, "lifecycle", None).await;
    let pending = store
        .transition(&before.id, Transition::Reopen)
        .await
        .unwrap();
    assert!(!pending.changed);
    assert!(pending.affected_task_ids.is_empty());
    assert_eq!(pending.task, before);
    assert_eq!(store.history(&before.id).await.unwrap().len(), 1);
    for (action, status, reason, kind) in [
        (Transition::Start, "in_progress", None, "status_changed"),
        (
            Transition::Block {
                reason: " 等待回复 ".into(),
            },
            "blocked",
            Some(" 等待回复 "),
            "status_changed",
        ),
        (
            Transition::Block {
                reason: "等待审批".into(),
            },
            "blocked",
            Some("等待审批"),
            "edited",
        ),
        (Transition::Start, "in_progress", None, "status_changed"),
        (
            Transition::Block {
                reason: "再次阻塞".into(),
            },
            "blocked",
            Some("再次阻塞"),
            "status_changed",
        ),
        (
            Transition::Done { cascade: false },
            "done",
            None,
            "status_changed",
        ),
        (Transition::Reopen, "pending", None, "status_changed"),
        (
            cancel(Some(" 不再需要 "), false),
            "cancelled",
            Some(" 不再需要 "),
            "status_changed",
        ),
        (
            cancel(Some("原因修正"), false),
            "cancelled",
            Some("原因修正"),
            "edited",
        ),
        (cancel(None, false), "cancelled", None, "edited"),
        (Transition::Reopen, "pending", None, "status_changed"),
    ] {
        let result = store.transition(&before.id, action.clone()).await.unwrap();
        assert!(result.changed);
        assert_eq!(result.affected_task_ids, [before.id.clone()]);
        let mut expected = before.clone();
        expected.status = status.into();
        expected.blocked_reason = (status == "blocked").then(|| reason.unwrap().into());
        expected.cancel_reason = (status == "cancelled")
            .then_some(reason)
            .flatten()
            .map(str::to_owned);
        expected.updated_at = result.task.updated_at.clone();
        expected.closed_at = match status {
            "done" | "cancelled" if kind == "edited" => before.closed_at.clone(),
            "done" | "cancelled" => Some(expected.updated_at.clone()),
            _ => None,
        };
        assert_eq!(result.task, expected);
        assert_eq!(store.get(&before.id).await.unwrap(), expected);
        let entries = store.history(&before.id).await.unwrap();
        let event = entries.last().unwrap();
        assert_eq!(event.kind, kind);
        assert_eq!(event.at, expected.updated_at);
        assert_eq!(
            event.changes.0,
            json!({"before": before, "after": expected})
        );
        let saved_history = history(&store, &before.id).await;
        let noop = store.transition(&before.id, action).await.unwrap();
        assert!(!noop.changed);
        assert!(noop.affected_task_ids.is_empty());
        assert_eq!(noop.task, expected);
        assert_eq!(history(&store, &before.id).await, saved_history);
        before = expected;
    }
}

#[tokio::test]
async fn invalid_states_reasons_and_missing_ids_do_not_write() {
    let (_dir, store, _conn) = setup().await;
    for closing in [Transition::Done { cascade: false }, cancel(None, false)] {
        let task = create(&store, "closed", None).await;
        let before = store.transition(&task.id, closing).await.unwrap().task;
        let saved_history = history(&store, &task.id).await;
        let opposite = if before.status == "done" {
            cancel(None, false)
        } else {
            Transition::Done { cascade: false }
        };
        for (action, target) in [
            (Transition::Start, "in_progress"),
            (
                Transition::Block {
                    reason: "reason".into(),
                },
                "blocked",
            ),
            (
                opposite,
                if before.status == "done" {
                    "cancelled"
                } else {
                    "done"
                },
            ),
        ] {
            assert!(
                matches!(store.transition(&task.id, action).await.unwrap_err(),
                AppError::InvalidState { task_id, status, requested_status }
                    if task_id == task.id && status == before.status && requested_status == target)
            );
        }
        assert_eq!(store.get(&task.id).await.unwrap(), before);
        assert_eq!(history(&store, &task.id).await, saved_history);
    }
    for state in [
        Transition::Start,
        Transition::Block {
            reason: "reason".into(),
        },
    ] {
        let task = create(&store, "open", None).await;
        let before = store.transition(&task.id, state).await.unwrap().task;
        let saved_history = history(&store, &task.id).await;
        assert!(matches!(
            store
                .transition(&task.id, Transition::Reopen)
                .await
                .unwrap_err(),
            AppError::InvalidState {
                requested_status: "pending",
                ..
            }
        ));
        for reason in ["", " \n\u{3000}"] {
            for action in [
                Transition::Block {
                    reason: reason.into(),
                },
                cancel(Some(reason), false),
            ] {
                assert!(
                    matches!(store.transition(&task.id, action).await.unwrap_err(),
                    AppError::InvalidInput { field, .. } if field == "reason")
                );
            }
        }
        assert_eq!(store.get(&task.id).await.unwrap(), before);
        assert_eq!(history(&store, &task.id).await, saved_history);
    }
    for action in [
        Transition::Start,
        Transition::Block {
            reason: "reason".into(),
        },
        Transition::Done { cascade: false },
        cancel(None, false),
        Transition::Reopen,
    ] {
        assert!(
            matches!(store.transition("missing", action).await.unwrap_err(),
            AppError::NotFound { entity: "task", id } if id == "missing")
        );
    }
}

#[tokio::test]
async fn closing_checks_all_descendants_and_cascades_only_to_open_tasks() {
    let (_dir, store, mut conn) = setup().await;
    for is_cancel in [false, true] {
        let root = create(&store, "root", None).await;
        let middle = create(&store, "middle", Some(&root.id)).await;
        let middle = store
            .transition(
                &middle.id,
                Transition::Block {
                    reason: "waiting".into(),
                },
            )
            .await
            .unwrap()
            .task;
        let leaf = create(&store, "leaf", Some(&middle.id)).await;
        let closed = create(&store, "closed", Some(&root.id)).await;
        let closed = store
            .transition(&closed.id, cancel(Some("old reason"), false))
            .await
            .unwrap()
            .task;
        let bridge = create(&store, "closed bridge", Some(&root.id)).await;
        let hidden = create(&store, "open under closed bridge", Some(&bridge.id)).await;
        // 非正常树 fixture：遍历不能因为中间节点关闭而遗漏更深的开放任务。
        sqlx::query("UPDATE tasks SET status = 'done', closed_at = updated_at WHERE id = ?")
            .bind(&bridge.id)
            .execute(&mut conn)
            .await
            .unwrap();
        let bridge = store.get(&bridge.id).await.unwrap();
        let closed_history = history(&store, &closed.id).await;
        let bridge_history = history(&store, &bridge.id).await;
        let mut open = vec![middle, leaf, hidden];
        open.sort_by(|left, right| left.id.cmp(&right.id));
        let blocking: Vec<_> = open.iter().map(|task| task.id.clone()).collect();
        let action = if is_cancel {
            cancel(Some("cascade reason"), false)
        } else {
            Transition::Done { cascade: false }
        };
        assert!(
            matches!(store.transition(&root.id, action).await.unwrap_err(),
            AppError::OpenDescendants { task_id, blocking_task_ids }
                if task_id == root.id && blocking_task_ids == blocking)
        );
        for task in std::iter::once(&root).chain(&open) {
            assert_eq!(store.get(&task.id).await.unwrap(), *task);
        }
        let action = if is_cancel {
            cancel(Some("cascade reason"), true)
        } else {
            Transition::Done { cascade: true }
        };
        let result = store.transition(&root.id, action.clone()).await.unwrap();
        let expected_ids: Vec<_> = std::iter::once(root.id.clone()).chain(blocking).collect();
        assert!(result.changed);
        assert_eq!(result.affected_task_ids, expected_ids);
        for before in std::iter::once(&root).chain(&open) {
            let mut expected = before.clone();
            expected.status = if is_cancel { "cancelled" } else { "done" }.into();
            expected.blocked_reason = None;
            expected.cancel_reason = is_cancel.then(|| "cascade reason".into());
            expected.updated_at = result.task.updated_at.clone();
            expected.closed_at = Some(result.task.updated_at.clone());
            assert_eq!(store.get(&before.id).await.unwrap(), expected);
            let entries = store.history(&before.id).await.unwrap();
            let event = entries.last().unwrap();
            assert_eq!(event.kind, "status_changed");
            assert_eq!(event.at, expected.updated_at);
            let mut changes = json!({"before": before, "after": expected});
            if before.id != root.id {
                changes["cascade_from"] = json!(root.id);
            }
            assert_eq!(event.changes.0, changes);
        }
        for (task, entries) in [(&closed, closed_history), (&bridge, bridge_history)] {
            assert_eq!(store.get(&task.id).await.unwrap(), *task);
            assert_eq!(history(&store, &task.id).await, entries);
        }
        let before = history(&store, &root.id).await;
        let repeated = store.transition(&root.id, action).await.unwrap();
        assert!(!repeated.changed);
        assert!(repeated.affected_task_ids.is_empty());
        assert_eq!(repeated.task, result.task);
        assert_eq!(history(&store, &root.id).await, before);
    }
}

#[tokio::test]
async fn reopening_checks_the_full_ancestor_chain_without_automatic_changes() {
    let (_dir, store, _conn) = setup().await;
    let root = create(&store, "root", None).await;
    let middle = create(&store, "middle", Some(&root.id)).await;
    let leaf = create(&store, "leaf", Some(&middle.id)).await;
    store
        .transition(&leaf.id, Transition::Done { cascade: false })
        .await
        .unwrap();
    assert_eq!(store.get(&middle.id).await.unwrap(), middle);
    assert_eq!(store.get(&root.id).await.unwrap(), root);
    store
        .transition(&root.id, cancel(None, true))
        .await
        .unwrap();
    let leaf_before = store.get(&leaf.id).await.unwrap();
    let leaf_history = history(&store, &leaf.id).await;
    assert!(
        matches!(store.transition(&leaf.id, Transition::Reopen).await.unwrap_err(),
        AppError::ClosedAncestor { parent_id, ancestor_ids }
            if parent_id == middle.id && ancestor_ids.len() == 2
                && ancestor_ids.contains(&root.id) && ancestor_ids.contains(&middle.id))
    );
    store
        .transition(&root.id, Transition::Reopen)
        .await
        .unwrap();
    assert_eq!(store.get(&middle.id).await.unwrap().status, "cancelled");
    assert_eq!(store.get(&leaf.id).await.unwrap(), leaf_before);
    assert!(
        matches!(store.transition(&leaf.id, Transition::Reopen).await.unwrap_err(),
        AppError::ClosedAncestor { ancestor_ids, .. } if ancestor_ids == [middle.id.clone()])
    );
    store
        .transition(&middle.id, Transition::Reopen)
        .await
        .unwrap();
    assert_eq!(store.get(&leaf.id).await.unwrap(), leaf_before);
    assert_eq!(history(&store, &leaf.id).await, leaf_history);
    let reopened = store
        .transition(&leaf.id, Transition::Reopen)
        .await
        .unwrap();
    assert_eq!(reopened.task.status, "pending");
    assert_eq!(reopened.task.closed_at, None);
    assert_eq!(reopened.task.cancel_reason, None);
}

#[tokio::test]
async fn last_descendant_history_failure_rolls_back_the_entire_cascade() {
    let (_dir, store, mut conn) = setup().await;
    let root = create(&store, "root", None).await;
    let mut children = vec![
        create(&store, "one", Some(&root.id)).await,
        create(&store, "two", Some(&root.id)).await,
    ];
    children.sort_by(|left, right| left.id.cmp(&right.id));
    // 仅对排序最后的后代注入故障，前面的写入也必须全部回滚。
    sqlx::query("CREATE TRIGGER fail_last_history BEFORE INSERT ON history WHEN NEW.kind = 'status_changed' AND NEW.task_id = (SELECT max(id) FROM tasks WHERE parent_id IS NOT NULL) BEGIN SELECT RAISE(ABORT, 'injected history failure'); END")
        .execute(&mut conn).await.unwrap();
    for action in [
        Transition::Done { cascade: true },
        cancel(Some("reason"), true),
    ] {
        assert!(matches!(
            store.transition(&root.id, action).await.unwrap_err(),
            AppError::Database(_)
        ));
        for task in std::iter::once(&root).chain(&children) {
            assert_eq!(store.get(&task.id).await.unwrap(), *task);
            assert_eq!(store.history(&task.id).await.unwrap().len(), 1);
        }
    }
    sqlx::query("DROP TRIGGER fail_last_history")
        .execute(&mut conn)
        .await
        .unwrap();
    assert!(
        store
            .transition(&root.id, Transition::Done { cascade: true })
            .await
            .unwrap()
            .changed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn independent_stores_serialize_repeated_transitions_and_parent_creation_races() {
    let (dir, store, _conn) = setup().await;
    let task = create(&store, "concurrent", None).await;
    let mut stores = Vec::new();
    for _ in 0..6 {
        stores.push(Store::open(dir.path().join("todos.db")).await.unwrap());
    }
    let barrier = Arc::new(Barrier::new(stores.len()));
    let mut jobs = Vec::new();
    for independent in stores {
        let id = task.id.clone();
        let barrier = barrier.clone();
        jobs.push(tokio::spawn(async move {
            barrier.wait().await;
            let result = independent
                .transition(&id, Transition::Done { cascade: false })
                .await
                .unwrap();
            independent.close().await;
            result
        }));
    }
    let mut changed = 0;
    for job in jobs {
        let result = job.await.unwrap();
        changed += usize::from(result.changed);
        assert_eq!(result.task, store.get(&task.id).await.unwrap());
        assert_eq!(result.affected_task_ids.len(), usize::from(result.changed));
    }
    assert_eq!(changed, 1);
    assert_eq!(store.history(&task.id).await.unwrap().len(), 2);
    let independent = Store::open(dir.path().join("todos.db")).await.unwrap();
    for _ in 0..6 {
        let parent = create(&store, "parent", None).await;
        match tokio::join!(
            store.transition(&parent.id, Transition::Done { cascade: false }),
            independent.create(request("child", Some(&parent.id)))
        ) {
            (Ok(closed), Err(AppError::ClosedAncestor { ancestor_ids, .. })) => {
                assert_eq!(closed.task.status, "done");
                assert_eq!(ancestor_ids.as_slice(), std::slice::from_ref(&parent.id));
                assert_eq!(store.history(&parent.id).await.unwrap().len(), 2);
            }
            (
                Err(AppError::OpenDescendants {
                    blocking_task_ids, ..
                }),
                Ok(child),
            ) => {
                assert_eq!(blocking_task_ids, [child.task.id]);
                assert_eq!(store.get(&parent.id).await.unwrap(), parent);
                assert_eq!(store.history(&parent.id).await.unwrap().len(), 1);
            }
            results => panic!(
                "Closing and creation must not produce an open child of a closed parent: {results:?}"
            ),
        }
    }
    independent.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cascading_close_and_descendant_reopen_cannot_leave_an_open_child() {
    let (dir, store, _conn) = setup().await;
    let independent = Store::open(dir.path().join("todos.db")).await.unwrap();
    for _ in 0..6 {
        let parent = create(&store, "parent", None).await;
        let child = create(&store, "child", Some(&parent.id)).await;
        let before = store
            .transition(&child.id, Transition::Done { cascade: false })
            .await
            .unwrap()
            .task;
        let (closing, reopening) = tokio::join!(
            store.transition(&parent.id, Transition::Done { cascade: true }),
            independent.transition(&child.id, Transition::Reopen)
        );
        let closing = closing.unwrap();
        let current = store.get(&child.id).await.unwrap();
        let entries = store.history(&child.id).await.unwrap();
        assert_eq!(closing.task.status, "done");
        assert_eq!(current.status, "done");
        assert_eq!(store.history(&parent.id).await.unwrap().len(), 2);
        match reopening {
            Ok(reopened) => {
                // 重开先提交，随后父任务的级联必须将它再次关闭。
                assert_eq!(reopened.task.status, "pending");
                assert_eq!(closing.affected_task_ids, [parent.id.clone(), child.id]);
                assert_eq!(entries.len(), 4);
                assert_eq!(
                    entries[2].changes.0,
                    json!({"before": before, "after": reopened.task})
                );
                assert_eq!(
                    entries[3].changes.0,
                    json!({
                        "before": reopened.task,
                        "after": current,
                        "cascade_from": parent.id,
                    })
                );
                assert_eq!(current.closed_at, closing.task.closed_at);
            }
            Err(AppError::ClosedAncestor { ancestor_ids, .. }) => {
                // 父任务先提交，重开必须失败且不修改已关闭的子任务。
                assert_eq!(ancestor_ids.as_slice(), std::slice::from_ref(&parent.id));
                assert_eq!(closing.affected_task_ids, [parent.id]);
                assert_eq!(current, before);
                assert_eq!(entries.len(), 2);
            }
            result => {
                panic!("Expected successful reopen before cascade or ClosedAncestor: {result:?}")
            }
        }
    }
    independent.close().await;
}

#[tokio::test]
async fn transitions_preserve_creation_idempotency_and_all_reminder_fields() {
    let (_dir, store, mut conn) = setup().await;
    let mut input = request("retry", None);
    input.creation_token = Some("stable-key".into());
    let task = store.create(input.clone()).await.unwrap().task;
    for (id, status) in [
        ("one", "scheduled"),
        ("two", "fired"),
        ("three", "cancelled"),
        ("four", "unknown"),
    ] {
        sqlx::query("INSERT INTO reminders (id, task_id, external_id, scheduled_at, timezone, channel, status, created_at, updated_at) VALUES (?, ?, ?, ?, 'UTC', 'wechat', ?, ?, ?)")
            .bind(id).bind(&task.id).bind(id).bind(&task.created_at).bind(status)
            .bind(&task.created_at).bind(&task.created_at).execute(&mut conn).await.unwrap();
    }
    let reminder_query = "SELECT json_array(id, task_id, external_id, scheduled_at, timezone, channel, status, created_at, updated_at) FROM reminders ORDER BY id";
    let before: Vec<String> = sqlx::query_scalar(reminder_query)
        .fetch_all(&mut conn)
        .await
        .unwrap();
    for action in [
        cancel(Some("reason"), false),
        Transition::Reopen,
        Transition::Done { cascade: false },
    ] {
        let result = store.transition(&task.id, action).await.unwrap();
        let saved_history = history(&store, &task.id).await;
        let retry = store.create(input.clone()).await.unwrap();
        assert!(retry.deduplicated);
        assert_eq!(retry.task, result.task);
        assert_eq!(history(&store, &task.id).await, saved_history);
        let after: Vec<String> = sqlx::query_scalar(reminder_query)
            .fetch_all(&mut conn)
            .await
            .unwrap();
        assert_eq!(after, before);
    }
}
