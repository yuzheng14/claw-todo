use claw_todo::{AppError, CreateTask, ListFilter, Progress, Store, Transition};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

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

fn request(title: &str, parent: Option<&str>) -> CreateTask {
    CreateTask {
        title: title.into(),
        category: "personal".into(),
        parent_id: parent.map(str::to_owned),
        ..Default::default()
    }
}

#[tokio::test]
async fn defaults_exclude_closed_tasks_and_progress_counts_only_direct_children() {
    let (_dir, store, _conn) = setup().await;
    let parent = store.create(request("parent", None)).await.unwrap().task;
    let child = store
        .create(request("child", Some(&parent.id)))
        .await
        .unwrap()
        .task;
    let grandchild = store
        .create(request("grandchild", Some(&child.id)))
        .await
        .unwrap()
        .task;
    let done = store
        .create(request("done", Some(&parent.id)))
        .await
        .unwrap()
        .task;
    let cancelled = store
        .create(request("cancelled", Some(&parent.id)))
        .await
        .unwrap()
        .task;
    store
        .transition(&done.id, Transition::Done { cascade: false })
        .await
        .unwrap();
    store
        .transition(
            &cancelled.id,
            Transition::Cancel {
                reason: None,
                cascade: false,
            },
        )
        .await
        .unwrap();
    store
        .transition(&child.id, Transition::Start)
        .await
        .unwrap();
    store
        .transition(
            &grandchild.id,
            Transition::Block {
                reason: "waiting".into(),
            },
        )
        .await
        .unwrap();
    let list = store.list(ListFilter::default()).await.unwrap();
    assert_eq!(list.summary.matched, 3);
    assert_eq!(list.summary.matched_open, 3);
    assert_eq!(list.summary.top_level_open, 1);
    assert_eq!(list.summary.total_open, 3);
    assert!(list.tasks.iter().all(
        |view| !view.context_only && !matches!(view.task.status.as_str(), "done" | "cancelled")
    ));
    let view = list
        .tasks
        .iter()
        .find(|view| view.task.id == parent.id)
        .unwrap();
    assert_eq!(
        view.progress,
        Progress {
            done: 1,
            cancelled: 1,
            total: 3
        }
    );
    let detail = store.show(&parent.id).await.unwrap();
    assert_eq!(detail.task, parent);
    assert_eq!(detail.progress, view.progress);
    assert_eq!(store.show(&child.id).await.unwrap().progress.total, 1);
    assert_eq!(
        store.show(&grandchild.id).await.unwrap().progress,
        Progress::default()
    );
    assert_eq!(
        store
            .list(ListFilter {
                all: true,
                ..Default::default()
            })
            .await
            .unwrap()
            .summary
            .matched,
        5
    );
    let closed = store
        .list(ListFilter {
            all: true,
            statuses: vec!["done".into(), "cancelled".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(closed.summary.matched, 2);
    assert_eq!(closed.summary.matched_open, 0);
    assert_eq!(closed.summary.total_open, 3);
    assert_eq!(closed.tasks.len(), 3);
    assert!(
        closed
            .tasks
            .iter()
            .find(|view| view.task.id == parent.id)
            .unwrap()
            .context_only
    );
}

#[tokio::test]
async fn combined_filters_keep_all_ancestor_context_without_inflating_match_counts() {
    let (_dir, store, _conn) = setup().await;
    let root = store.create(request("root", None)).await.unwrap().task;
    let parent = store
        .create(request("parent", Some(&root.id)))
        .await
        .unwrap()
        .task;
    let mut input = request("Build Rust", Some(&parent.id));
    input.category = "work".into();
    input.project = Some("cli".into());
    input.description = Some("中文说明 Literal %_".into());
    let matched = store.create(input).await.unwrap().task;
    store.create(request("unrelated", None)).await.unwrap();
    let filter = |search: &str| ListFilter {
        category: Some("work".into()),
        project: Some("cli".into()),
        statuses: vec!["pending".into()],
        search: Some(search.into()),
        ..Default::default()
    };
    for text in ["rUsT", "中文", "%_", ""] {
        let result = store.list(filter(text)).await.unwrap();
        assert_eq!(result.summary.matched, 1);
        assert_eq!(result.summary.matched_open, 1);
        assert_eq!(result.summary.top_level_open, 2);
        assert_eq!(result.summary.total_open, 4);
        assert_eq!(result.tasks.len(), 3);
        assert_eq!(
            result.tasks.iter().filter(|view| view.context_only).count(),
            2
        );
        assert_eq!(
            result
                .tasks
                .iter()
                .find(|view| !view.context_only)
                .unwrap()
                .task
                .id,
            matched.id
        );
    }
    assert!(
        store
            .list(filter("not present"))
            .await
            .unwrap()
            .tasks
            .is_empty()
    );
    let all = store
        .list(ListFilter {
            search: Some("r".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let ids: std::collections::HashSet<_> = all.tasks.iter().map(|view| &view.task.id).collect();
    assert_eq!(ids.len(), all.tasks.len());
    assert!(ids.contains(&root.id) && ids.contains(&parent.id));
}

#[tokio::test]
async fn empty_invalid_and_missing_queries_are_read_only() {
    let (_dir, store, _conn) = setup().await;
    let empty = store.list(ListFilter::default()).await.unwrap();
    assert!(empty.tasks.is_empty());
    assert_eq!(empty.summary.total_open, 0);
    for filter in [
        ListFilter {
            category: Some("other".into()),
            ..Default::default()
        },
        ListFilter {
            statuses: vec!["unknown".into()],
            ..Default::default()
        },
    ] {
        assert!(matches!(
            store.list(filter).await.unwrap_err(),
            AppError::InvalidInput { .. }
        ));
    }
    assert!(
        matches!(store.show("missing").await.unwrap_err(), AppError::NotFound { entity: "task", id } if id == "missing")
    );
}

#[tokio::test]
async fn large_results_are_untruncated_and_timestamp_ties_have_stable_id_order() {
    let (_dir, store, mut conn) = setup().await;
    // 批量 fixture 用相同时间戳，验证不隐藏分页且次级 ID 排序稳定。
    sqlx::query("WITH RECURSIVE n(value) AS (SELECT 1 UNION ALL SELECT value + 1 FROM n WHERE value < 1100) INSERT INTO tasks (id, title, status, category, created_at, updated_at, creation_request) SELECT printf('id-%04d', value), 'bulk', 'pending', 'personal', '2026-01-01T00:00:00.000000Z', '2026-01-01T00:00:00.000000Z', '{}' FROM n")
        .execute(&mut conn).await.unwrap();
    let first = store.list(ListFilter::default()).await.unwrap();
    let second = store.list(ListFilter::default()).await.unwrap();
    assert_eq!(first.tasks.len(), 1100);
    assert_eq!(first.summary.matched, 1100);
    assert_eq!(first.summary.top_level_open, 1100);
    let ids: Vec<_> = first
        .tasks
        .iter()
        .map(|view| view.task.id.as_str())
        .collect();
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        ids,
        second
            .tasks
            .iter()
            .map(|view| view.task.id.as_str())
            .collect::<Vec<_>>()
    );
}
