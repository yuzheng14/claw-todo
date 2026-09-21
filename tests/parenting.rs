use claw_todo::{AppError, CreateTask, Store, Task, Transition};
use serde_json::{Value, json};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use tokio::sync::Barrier;

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
    store
        .create(CreateTask {
            title: title.into(),
            category: "personal".into(),
            parent_id: parent.map(str::to_owned),
            ..Default::default()
        })
        .await
        .unwrap()
        .task
}

async fn history(store: &Store, id: &str) -> Value {
    serde_json::to_value(store.history(id).await.unwrap()).unwrap()
}

async fn assert_move(store: &Store, before: &Task, parent: Option<&str>) -> Task {
    let previous = store.history(&before.id).await.unwrap().len();
    let result = store.set_parent(&before.id, parent).await.unwrap();
    assert!(result.changed);
    let mut expected = before.clone();
    expected.parent_id = parent.map(str::to_owned);
    expected.updated_at = result.task.updated_at.clone();
    assert!(chrono::DateTime::parse_from_rfc3339(&expected.updated_at).is_ok());
    assert_eq!(result.task, expected);
    assert_eq!(store.get(&before.id).await.unwrap(), expected);
    let entries = store.history(&before.id).await.unwrap();
    assert_eq!(entries.len(), previous + 1);
    let event = entries.last().unwrap();
    assert_eq!(event.kind, "parent_changed");
    assert_eq!(event.at, expected.updated_at);
    assert_eq!(
        event.changes.0,
        json!({"before": before, "after": expected})
    );
    expected
}

#[tokio::test]
async fn attaching_moving_and_detaching_preserve_subtree_fields_and_creation_retry() {
    let (_dir, store, mut conn) = setup().await;
    let left = create(&store, "left", None).await;
    let right = create(&store, "right", None).await;
    let input = CreateTask {
        title: " moving subtree ".into(),
        category: "work".into(),
        description: Some("description".into()),
        project: Some("project".into()),
        sources: vec!["notes.md".into(), "https://example.com".into()],
        creation_token: Some("stable-creation-key".into()),
        ..Default::default()
    };
    let task = store.create(input.clone()).await.unwrap().task;
    let mut task = store
        .transition(
            &task.id,
            Transition::Block {
                reason: "waiting".into(),
            },
        )
        .await
        .unwrap()
        .task;
    let child = create(&store, "child", Some(&task.id)).await;
    let leaf = create(&store, "leaf", Some(&child.id)).await;
    let original: String = sqlx::query_scalar("SELECT creation_request FROM tasks WHERE id = ?")
        .bind(&task.id)
        .fetch_one(&mut conn)
        .await
        .unwrap();
    let untouched = [left.clone(), right.clone(), child, leaf];
    let mut saved = Vec::new();
    for record in &untouched {
        saved.push(history(&store, &record.id).await);
    }
    for parent in [Some(left.id.as_str()), Some(right.id.as_str()), None] {
        task = assert_move(&store, &task, parent).await;
        let before_retry = history(&store, &task.id).await;
        let retry = store.create(input.clone()).await.unwrap();
        assert!(retry.deduplicated);
        assert_eq!(retry.task, task);
        assert_eq!(history(&store, &task.id).await, before_retry);
        for (record, events) in untouched.iter().zip(&saved) {
            assert_eq!(store.get(&record.id).await.unwrap(), *record);
            assert_eq!(history(&store, &record.id).await, *events);
        }
    }
    let current: String = sqlx::query_scalar("SELECT creation_request FROM tasks WHERE id = ?")
        .bind(&task.id)
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(current, original);
}

#[tokio::test]
async fn unchanged_parent_is_noop_even_below_a_closed_ancestor() {
    let (_dir, store, mut conn) = setup().await;
    let root = create(&store, "root", None).await;
    let child = create(&store, "child", Some(&root.id)).await;
    // 非正常树 fixture：没有改变父级时，不重新验证已有祖先。
    sqlx::query("UPDATE tasks SET status = 'done', closed_at = updated_at WHERE id = ?")
        .bind(&root.id)
        .execute(&mut conn)
        .await
        .unwrap();
    let root = store.get(&root.id).await.unwrap();
    for task in [create(&store, "top-level", None).await, root, child] {
        let events = history(&store, &task.id).await;
        let result = store
            .set_parent(&task.id, task.parent_id.as_deref())
            .await
            .unwrap();
        assert!(!result.changed);
        assert_eq!(result.task, task);
        assert_eq!(store.get(&task.id).await.unwrap(), task);
        assert_eq!(history(&store, &task.id).await, events);
    }
}

#[tokio::test]
async fn invalid_ids_and_cycles_are_rejected_for_open_and_closed_tasks() {
    let (_dir, store, _conn) = setup().await;
    for closing in [
        None,
        Some(Transition::Done { cascade: true }),
        Some(Transition::Cancel {
            reason: Some("cancelled".into()),
            cascade: true,
        }),
    ] {
        let root = create(&store, "root", None).await;
        let middle = create(&store, "middle", Some(&root.id)).await;
        let leaf = create(&store, "leaf", Some(&middle.id)).await;
        if let Some(action) = closing {
            store.transition(&root.id, action).await.unwrap();
        }
        for (id, parent) in [
            (&root.id, &root.id),
            (&root.id, &middle.id),
            (&root.id, &leaf.id),
            (&middle.id, &leaf.id),
        ] {
            let before = store.get(id).await.unwrap();
            let events = history(&store, id).await;
            assert!(
                matches!(store.set_parent(id, Some(parent)).await.unwrap_err(),
                AppError::CycleDetected { task_id, parent_id }
                    if task_id == *id && parent_id == *parent)
            );
            assert_eq!(store.get(id).await.unwrap(), before);
            assert_eq!(history(&store, id).await, events);
        }
    }
    let task = create(&store, "valid", None).await;
    for parent in ["", " \n\u{3000}"] {
        assert!(
            matches!(store.set_parent(&task.id, Some(parent)).await.unwrap_err(),
            AppError::InvalidInput { field, .. } if field == "parent_id")
        );
    }
    let padded = format!(" {} ", task.id);
    for parent in ["missing", padded.as_str()] {
        assert!(
            matches!(store.set_parent(&task.id, Some(parent)).await.unwrap_err(),
            AppError::NotFound { entity: "task", id } if id == parent)
        );
    }
    assert!(
        matches!(store.set_parent("missing", None).await.unwrap_err(),
        AppError::NotFound { entity: "task", id } if id == "missing")
    );
    assert_eq!(store.get(&task.id).await.unwrap(), task);
    assert_eq!(store.history(&task.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn open_tasks_check_all_new_ancestors_but_can_leave_closed_old_ancestors() {
    let (_dir, store, mut conn) = setup().await;
    let root = create(&store, "root", None).await;
    let middle = create(&store, "middle", Some(&root.id)).await;
    let parent = create(&store, "parent", Some(&middle.id)).await;
    let child = create(&store, "old child", Some(&parent.id)).await;
    let other_child = create(&store, "other child", Some(&parent.id)).await;
    let destination = create(&store, "open destination", None).await;
    // 保留开放的直接父级，验证更深祖先同样会被检查且全部返回。
    sqlx::query("UPDATE tasks SET status = 'done', closed_at = updated_at WHERE id IN (?, ?)")
        .bind(&root.id)
        .bind(&middle.id)
        .execute(&mut conn)
        .await
        .unwrap();
    let task = create(&store, "outside", None).await;
    for action in [
        None,
        Some(Transition::Start),
        Some(Transition::Block {
            reason: "waiting".into(),
        }),
    ] {
        if let Some(action) = action {
            store.transition(&task.id, action).await.unwrap();
        }
        let before = store.get(&task.id).await.unwrap();
        let events = history(&store, &task.id).await;
        assert!(
            matches!(store.set_parent(&task.id, Some(&parent.id)).await.unwrap_err(),
            AppError::ClosedAncestor { parent_id, ancestor_ids }
                if parent_id == parent.id && ancestor_ids == [middle.id.clone(), root.id.clone()])
        );
        assert_eq!(store.get(&task.id).await.unwrap(), before);
        assert_eq!(history(&store, &task.id).await, events);
    }
    assert_move(&store, &child, None).await;
    assert_move(&store, &other_child, Some(&destination.id)).await;
}

#[tokio::test]
async fn closed_tasks_can_change_parents_without_reopening_or_changing_reasons() {
    let (_dir, store, _conn) = setup().await;
    let parent = create(&store, "closed parent", None).await;
    let parent = store
        .transition(&parent.id, Transition::Done { cascade: false })
        .await
        .unwrap()
        .task;
    let open = create(&store, "open parent", None).await;
    for closing in [
        Transition::Done { cascade: false },
        Transition::Cancel {
            reason: Some(" original reason ".into()),
            cascade: false,
        },
    ] {
        let task = create(&store, "closed child", None).await;
        let mut task = store.transition(&task.id, closing).await.unwrap().task;
        for next in [Some(parent.id.as_str()), Some(open.id.as_str()), None] {
            task = assert_move(&store, &task, next).await;
            let events = history(&store, &task.id).await;
            let noop = store.set_parent(&task.id, next).await.unwrap();
            assert!(!noop.changed);
            assert_eq!(noop.task, task);
            assert_eq!(history(&store, &task.id).await, events);
        }
    }
}

#[tokio::test]
async fn history_failure_rolls_back_parent_and_timestamp() {
    let (_dir, store, mut conn) = setup().await;
    let task = create(&store, "atomic", None).await;
    let parent = create(&store, "parent", None).await;
    sqlx::query("CREATE TRIGGER fail_parent_history BEFORE INSERT ON history WHEN NEW.kind = 'parent_changed' BEGIN SELECT RAISE(ABORT, 'injected history failure'); END")
        .execute(&mut conn).await.unwrap();
    assert!(matches!(
        store
            .set_parent(&task.id, Some(&parent.id))
            .await
            .unwrap_err(),
        AppError::Database(_)
    ));
    assert_eq!(store.get(&task.id).await.unwrap(), task);
    assert_eq!(store.history(&task.id).await.unwrap().len(), 1);
    assert_eq!(store.get(&parent.id).await.unwrap(), parent);
    assert_eq!(store.history(&parent.id).await.unwrap().len(), 1);
    sqlx::query("DROP TRIGGER fail_parent_history")
        .execute(&mut conn)
        .await
        .unwrap();
    assert_move(&store, &task, Some(&parent.id)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn simultaneous_cross_parent_moves_cannot_create_a_cycle() {
    let (dir, store, _conn) = setup().await;
    let independent = Store::open(dir.path().join("todos.db")).await.unwrap();
    for _ in 0..6 {
        let left = create(&store, "left", None).await;
        let right = create(&store, "right", None).await;
        let barrier = Barrier::new(2);
        let results = tokio::join!(
            async {
                barrier.wait().await;
                store.set_parent(&left.id, Some(&right.id)).await
            },
            async {
                barrier.wait().await;
                independent.set_parent(&right.id, Some(&left.id)).await
            }
        );
        let (moved, unchanged, result, error) = match results {
            (Ok(result), Err(error)) => (&left, &right, result, error),
            (Err(error), Ok(result)) => (&right, &left, result, error),
            results => panic!("Exactly one move must win: {results:?}"),
        };
        assert!(
            matches!(error, AppError::CycleDetected { task_id, parent_id }
            if task_id == unchanged.id && parent_id == moved.id)
        );
        assert!(result.changed);
        assert_eq!(
            result.task.parent_id.as_deref(),
            Some(unchanged.id.as_str())
        );
        assert_eq!(store.get(&moved.id).await.unwrap(), result.task);
        assert_eq!(store.get(&unchanged.id).await.unwrap(), *unchanged);
        assert_eq!(store.history(&moved.id).await.unwrap().len(), 2);
        assert_eq!(store.history(&unchanged.id).await.unwrap().len(), 1);
    }
    independent.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_parent_closing_and_move_cannot_leave_an_open_descendant() {
    let (dir, store, _conn) = setup().await;
    let independent = Store::open(dir.path().join("todos.db")).await.unwrap();
    for cascade in [false, true] {
        for _ in 0..4 {
            let parent = create(&store, "destination", None).await;
            let task = create(&store, "moving", None).await;
            let leaf = create(&store, "subtree", Some(&task.id)).await;
            let barrier = Barrier::new(2);
            let results = tokio::join!(
                async {
                    barrier.wait().await;
                    store
                        .transition(&parent.id, Transition::Done { cascade })
                        .await
                },
                async {
                    barrier.wait().await;
                    independent.set_parent(&task.id, Some(&parent.id)).await
                }
            );
            match results {
                (
                    Ok(closing),
                    Err(AppError::ClosedAncestor {
                        parent_id,
                        ancestor_ids,
                    }),
                ) => {
                    assert_eq!(closing.task.status, "done");
                    assert_eq!(parent_id, parent.id);
                    assert_eq!(ancestor_ids.as_slice(), std::slice::from_ref(&parent.id));
                    assert_eq!(store.get(&task.id).await.unwrap(), task);
                    assert_eq!(store.get(&leaf.id).await.unwrap(), leaf);
                    assert_eq!(store.history(&task.id).await.unwrap().len(), 1);
                }
                (
                    Err(AppError::OpenDescendants {
                        task_id,
                        mut blocking_task_ids,
                    }),
                    Ok(moved),
                ) => {
                    assert!(!cascade);
                    assert_eq!(task_id, parent.id);
                    blocking_task_ids.sort();
                    let mut expected = vec![task.id.clone(), leaf.id.clone()];
                    expected.sort();
                    assert_eq!(blocking_task_ids, expected);
                    assert_eq!(moved.task.parent_id.as_deref(), Some(parent.id.as_str()));
                    assert_eq!(store.get(&parent.id).await.unwrap(), parent);
                    assert_eq!(store.get(&task.id).await.unwrap(), moved.task);
                    assert_eq!(store.get(&leaf.id).await.unwrap(), leaf);
                }
                (Ok(closing), Ok(moved)) => {
                    assert!(cascade);
                    assert_eq!(closing.task.status, "done");
                    let current = store.get(&task.id).await.unwrap();
                    assert_eq!(current.parent_id.as_deref(), Some(parent.id.as_str()));
                    assert_eq!(current.status, "done");
                    assert_eq!(store.get(&leaf.id).await.unwrap().status, "done");
                    let entries = store.history(&task.id).await.unwrap();
                    assert_eq!(entries.len(), 3);
                    assert_eq!(
                        entries[1].changes.0,
                        json!({"before": task, "after": moved.task})
                    );
                    assert_eq!(
                        entries[2].changes.0,
                        json!({"before": moved.task, "after": current, "cascade_from": parent.id})
                    );
                }
                results => panic!("Unexpected closing/move outcome: {results:?}"),
            }
        }
    }
    independent.close().await;
}
