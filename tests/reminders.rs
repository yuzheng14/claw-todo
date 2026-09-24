use claw_todo::{AddReminder, AppError, CreateTask, EditReminder, EditTask, Store, Transition};
use serde_json::json;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

fn task_input(category: &str) -> CreateTask {
    CreateTask {
        title: "task".into(),
        category: category.into(),
        ..Default::default()
    }
}

fn reminder_input(channel: &str) -> AddReminder {
    AddReminder {
        external_id: " external-1 ".into(),
        scheduled_at: " 2026-09-23T09:00:00+08:00 ".into(),
        timezone: " Asia/Shanghai ".into(),
        channel: channel.into(),
        status: "scheduled".into(),
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
async fn reminder_crud_records_snapshots_and_updates_task_timestamp() {
    let (_dir, store, _conn) = setup().await;
    let task = store.create(task_input("personal")).await.unwrap().task;
    assert!(store.list_reminders(&task.id).await.unwrap().is_empty());
    let result = store
        .add_reminder(&task.id, reminder_input(" Email "))
        .await
        .unwrap();
    assert!(result.changed);
    let before = result.reminder;
    assert_eq!(
        uuid::Uuid::parse_str(&before.id).unwrap().get_version_num(),
        4
    );
    assert_eq!(before.task_id, task.id);
    assert_eq!(before.external_id, "external-1");
    assert_eq!(before.scheduled_at, "2026-09-23T09:00:00+08:00");
    assert_eq!(before.timezone, "Asia/Shanghai");
    assert_eq!(before.channel, "email");
    assert_eq!(before.status, "scheduled");
    assert_eq!(before.created_at, before.updated_at);
    assert_eq!(
        store.get(&task.id).await.unwrap().updated_at,
        before.created_at
    );
    assert_eq!(
        store.list_reminders(&task.id).await.unwrap(),
        vec![before.clone()]
    );
    let history = store.history(&task.id).await.unwrap();
    assert_eq!(history.len(), 2);
    let detail = store.show(&task.id).await.unwrap();
    assert_eq!(detail.reminders, vec![before.clone()]);
    assert_eq!(detail.task.updated_at, before.updated_at);
    assert_eq!(history[1].kind, "reminder_added");
    assert_eq!(history[1].at, before.created_at);
    assert_eq!(
        history[1].changes.0,
        json!({"before": null, "after": before})
    );

    let changed = store
        .edit_reminder(
            &before.id,
            EditReminder {
                external_id: Some("external-2".into()),
                scheduled_at: Some("2026-09-24T01:00:00.123456Z".into()),
                timezone: Some("UTC".into()),
                channel: Some(" DingTalk ".into()),
                status: Some("fired".into()),
            },
        )
        .await
        .unwrap();
    assert!(changed.changed);
    let after = changed.reminder;
    let mut expected = before.clone();
    expected.external_id = "external-2".into();
    expected.scheduled_at = "2026-09-24T01:00:00.123456Z".into();
    expected.timezone = "UTC".into();
    expected.channel = "dingtalk".into();
    expected.status = "fired".into();
    expected.updated_at = after.updated_at.clone();
    assert_eq!(after, expected);
    let history = store.history(&task.id).await.unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[2].kind, "reminder_updated");
    assert_eq!(history[2].at, after.updated_at);
    assert_eq!(
        history[2].changes.0,
        json!({"before": before, "after": after})
    );
    assert_eq!(
        store.get(&task.id).await.unwrap().updated_at,
        after.updated_at
    );

    assert_eq!(store.remove_reminder(&after.id).await.unwrap(), after);
    assert!(store.list_reminders(&task.id).await.unwrap().is_empty());
    let history = store.history(&task.id).await.unwrap();
    assert_eq!(history.len(), 4);
    assert_eq!(history[3].kind, "reminder_removed");
    assert_eq!(
        history[3].changes.0,
        json!({"before": after, "after": null})
    );
    assert_eq!(store.get(&task.id).await.unwrap().updated_at, history[3].at);
}

#[tokio::test]
async fn normalized_noop_edits_change_neither_timestamps_nor_history() {
    let (_dir, store, _conn) = setup().await;
    let task = store.create(task_input("personal")).await.unwrap().task;
    let reminder = store
        .add_reminder(&task.id, reminder_input("We_Chat"))
        .await
        .unwrap()
        .reminder;
    assert_eq!(reminder.channel, "wechat");
    let before = store.get(&task.id).await.unwrap();
    for input in [
        EditReminder::default(),
        EditReminder {
            external_id: Some(" external-1 ".into()),
            scheduled_at: Some(" 2026-09-23T09:00:00+08:00 ".into()),
            timezone: Some(" Asia/Shanghai ".into()),
            channel: Some(" WX ".into()),
            status: Some("scheduled".into()),
        },
    ] {
        let result = store.edit_reminder(&reminder.id, input).await.unwrap();
        assert!(!result.changed);
        assert_eq!(result.reminder, reminder);
        assert_eq!(store.get(&task.id).await.unwrap(), before);
        assert_eq!(store.history(&task.id).await.unwrap().len(), 2);
    }
}

#[tokio::test]
async fn multiple_reminders_are_task_scoped_and_stably_ordered_without_external_id_dedup() {
    let (_dir, store, mut conn) = setup().await;
    let task = store.create(task_input("personal")).await.unwrap().task;
    let other = store.create(task_input("personal")).await.unwrap().task;
    let mut expected = Vec::new();
    for status in ["scheduled", "fired", "cancelled", "unknown"] {
        let mut input = reminder_input("custom-channel");
        input.status = status.into();
        expected.push(store.add_reminder(&task.id, input).await.unwrap().reminder);
    }
    store
        .add_reminder(&other.id, reminder_input("email"))
        .await
        .unwrap();
    assert_eq!(store.list_reminders(&task.id).await.unwrap(), expected);
    // 强制时间相同，验证次级 ID 排序，而不是依赖数据库偶然返回顺序。
    sqlx::query("UPDATE reminders SET created_at = ? WHERE task_id = ?")
        .bind("2026-09-22T00:00:00.000000Z")
        .bind(&task.id)
        .execute(&mut conn)
        .await
        .unwrap();
    for reminder in &mut expected {
        reminder.created_at = "2026-09-22T00:00:00.000000Z".into();
    }
    expected.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(store.list_reminders(&task.id).await.unwrap(), expected);
    assert_eq!(store.list_reminders(&other.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn closing_and_reopening_do_not_change_reminders_and_closed_tasks_remain_editable() {
    let (_dir, store, _conn) = setup().await;
    let task = store.create(task_input("personal")).await.unwrap().task;
    let reminder = store
        .add_reminder(&task.id, reminder_input("wechat"))
        .await
        .unwrap()
        .reminder;
    let closed = store
        .transition(&task.id, Transition::Done { cascade: false })
        .await
        .unwrap()
        .task;
    assert_eq!(
        store.list_reminders(&task.id).await.unwrap(),
        vec![reminder.clone()]
    );
    let added = store
        .add_reminder(&task.id, reminder_input("email"))
        .await
        .unwrap()
        .reminder;
    let edited = store
        .edit_reminder(
            &reminder.id,
            EditReminder {
                status: Some("unknown".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .reminder;
    let current = store.get(&task.id).await.unwrap();
    assert_eq!(current.status, "done");
    assert_eq!(current.closed_at, closed.closed_at);
    store
        .transition(&task.id, Transition::Reopen)
        .await
        .unwrap();
    store
        .transition(
            &task.id,
            Transition::Cancel {
                reason: None,
                cascade: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store.list_reminders(&task.id).await.unwrap(),
        vec![edited, added]
    );
}

#[tokio::test]
async fn wechat_aliases_cannot_bypass_work_rules_on_add_edit_or_category_change() {
    let (_dir, store, _conn) = setup().await;
    let task = store.create(task_input("work")).await.unwrap().task;
    let reminder = store
        .add_reminder(&task.id, reminder_input("email"))
        .await
        .unwrap()
        .reminder;
    let before = store.get(&task.id).await.unwrap();
    for alias in [
        "wechat", "WeChat", " weixin ", "WX", "微信", "We-Chat", "we_ chat",
    ] {
        let error = store
            .add_reminder(&task.id, reminder_input(alias))
            .await
            .unwrap_err();
        assert!(
            matches!(error, AppError::ChannelForbidden { task_id, channel: "wechat", .. } if task_id == task.id)
        );
        let error = store
            .edit_reminder(
                &reminder.id,
                EditReminder {
                    channel: Some(alias.into()),
                    status: Some("cancelled".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(
            matches!(error, AppError::ChannelForbidden { reminder_ids, .. } if reminder_ids == vec![reminder.id.clone()])
        );
    }
    assert_eq!(store.get(&task.id).await.unwrap(), before);
    assert_eq!(
        store.list_reminders(&task.id).await.unwrap(),
        vec![reminder]
    );
    assert_eq!(store.history(&task.id).await.unwrap().len(), 2);

    let personal = store.create(task_input("personal")).await.unwrap().task;
    let mut input = reminder_input("WeiXin");
    input.status = "cancelled".into();
    let linked = store
        .add_reminder(&personal.id, input)
        .await
        .unwrap()
        .reminder;
    assert!(matches!(store.edit(&personal.id, EditTask {
        category: Some("work".into()), ..Default::default()
    }).await.unwrap_err(), AppError::ChannelForbidden { reminder_ids, .. } if reminder_ids == vec![linked.id.clone()]));
    store.remove_reminder(&linked.id).await.unwrap();
    assert_eq!(
        store
            .edit(
                &personal.id,
                EditTask {
                    category: Some("work".into()),
                    ..Default::default()
                }
            )
            .await
            .unwrap()
            .task
            .category,
        "work"
    );
}

#[tokio::test]
async fn invalid_fields_and_missing_entities_leave_no_writes() {
    let (_dir, store, _conn) = setup().await;
    let task = store.create(task_input("personal")).await.unwrap().task;
    let reminder = store
        .add_reminder(&task.id, reminder_input("email"))
        .await
        .unwrap()
        .reminder;
    let before = store.get(&task.id).await.unwrap();
    for (field, value) in [
        ("external_id", " \n\u{3000}"),
        ("timezone", ""),
        ("channel", " \n"),
        ("scheduled_at", "2026-09-23T09:00:00"),
        ("scheduled_at", "2026-02-30T09:00:00Z"),
        ("status", "SCHEDULED"),
        ("status", "scheduled "),
    ] {
        let mut input = reminder_input("email");
        let mut edit = EditReminder::default();
        match field {
            "external_id" => {
                input.external_id = value.into();
                edit.external_id = Some(value.into());
            }
            "timezone" => {
                input.timezone = value.into();
                edit.timezone = Some(value.into());
            }
            "channel" => {
                input.channel = value.into();
                edit.channel = Some(value.into());
            }
            "scheduled_at" => {
                input.scheduled_at = value.into();
                edit.scheduled_at = Some(value.into());
            }
            _ => {
                input.status = value.into();
                edit.status = Some(value.into());
            }
        }
        assert!(
            matches!(store.add_reminder(&task.id, input).await.unwrap_err(), AppError::InvalidInput { field: actual, .. } if actual == field)
        );
        assert!(
            matches!(store.edit_reminder(&reminder.id, edit).await.unwrap_err(), AppError::InvalidInput { field: actual, .. } if actual == field)
        );
    }
    assert!(matches!(
        store
            .add_reminder("missing", reminder_input("email"))
            .await
            .unwrap_err(),
        AppError::NotFound { entity: "task", .. }
    ));
    assert!(matches!(
        store.list_reminders("missing").await.unwrap_err(),
        AppError::NotFound { entity: "task", .. }
    ));
    assert!(matches!(
        store
            .edit_reminder("missing", EditReminder::default())
            .await
            .unwrap_err(),
        AppError::NotFound {
            entity: "reminder",
            ..
        }
    ));
    assert_eq!(store.get(&task.id).await.unwrap(), before);
    assert_eq!(store.history(&task.id).await.unwrap().len(), 2);
    assert_eq!(
        store.list_reminders(&task.id).await.unwrap(),
        vec![reminder.clone()]
    );
    store.remove_reminder(&reminder.id).await.unwrap();
    let removed = store.get(&task.id).await.unwrap();
    assert!(matches!(
        store.remove_reminder(&reminder.id).await.unwrap_err(),
        AppError::NotFound {
            entity: "reminder",
            ..
        }
    ));
    assert_eq!(store.get(&task.id).await.unwrap(), removed);
    assert_eq!(store.history(&task.id).await.unwrap().len(), 3);
}

#[tokio::test]
async fn history_failure_rolls_back_add_edit_remove_and_task_timestamp() {
    let (_dir, store, mut conn) = setup().await;
    let task = store.create(task_input("personal")).await.unwrap().task;
    let reminder = store
        .add_reminder(&task.id, reminder_input("email"))
        .await
        .unwrap()
        .reminder;
    let before = store.get(&task.id).await.unwrap();
    sqlx::query("CREATE TRIGGER fail_reminder_history BEFORE INSERT ON history WHEN NEW.kind LIKE 'reminder_%' BEGIN SELECT RAISE(ABORT, 'injected'); END")
        .execute(&mut conn).await.unwrap();
    assert!(matches!(
        store
            .add_reminder(&task.id, reminder_input("email"))
            .await
            .unwrap_err(),
        AppError::Database(_)
    ));
    assert!(matches!(
        store
            .edit_reminder(
                &reminder.id,
                EditReminder {
                    status: Some("fired".into()),
                    ..Default::default()
                }
            )
            .await
            .unwrap_err(),
        AppError::Database(_)
    ));
    assert!(matches!(
        store.remove_reminder(&reminder.id).await.unwrap_err(),
        AppError::Database(_)
    ));
    assert_eq!(store.get(&task.id).await.unwrap(), before);
    assert_eq!(
        store.list_reminders(&task.id).await.unwrap(),
        vec![reminder.clone()]
    );
    assert_eq!(store.history(&task.id).await.unwrap().len(), 2);
    sqlx::query("DROP TRIGGER fail_reminder_history")
        .execute(&mut conn)
        .await
        .unwrap();
    store.remove_reminder(&reminder.id).await.unwrap();
}

#[tokio::test]
async fn concurrent_wechat_add_and_work_classification_cannot_both_succeed() {
    let (dir, store, _conn) = setup().await;
    let other = Store::open(dir.path().join("todos.db")).await.unwrap();
    for _ in 0..6 {
        let task = store.create(task_input("personal")).await.unwrap().task;
        let (classification, reminder) = tokio::join!(
            store.edit(
                &task.id,
                EditTask {
                    category: Some("work".into()),
                    ..Default::default()
                }
            ),
            other.add_reminder(&task.id, reminder_input("WeChat")),
        );
        let (category, count, error) = match (classification, reminder) {
            (Ok(_), Err(error)) => ("work", 0, error),
            (Err(error), Ok(_)) => ("personal", 1, error),
            results => panic!("Exactly one concurrent mutation must succeed: {results:?}"),
        };
        assert!(matches!(error, AppError::ChannelForbidden { .. }));
        assert_eq!(store.get(&task.id).await.unwrap().category, category);
        assert_eq!(store.list_reminders(&task.id).await.unwrap().len(), count);
        assert_eq!(store.history(&task.id).await.unwrap().len(), 2);
    }
}
