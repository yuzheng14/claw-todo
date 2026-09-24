use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqliteConnection;
use uuid::Uuid;

use crate::{
    AppError, Result, Store,
    db::{get_task, record},
};

/// 外部提醒的本地关联快照；不会调度、发送或自动同步外部提醒。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Reminder {
    /// 程序生成的本地 UUIDv4；编辑和移除关联时使用，不是外部提醒 ID。
    pub id: String,
    /// 所属任务 ID，关联创建后不能修改。
    pub task_id: String,
    /// 调用方已创建的外部提醒 ID；不验证外部存在性，也不要求全局唯一。
    pub external_id: String,
    /// 带明确 UTC 偏移的 RFC 3339 计划时间，保留输入的偏移和精度。
    pub scheduled_at: String,
    /// 外部提醒的时区标签；仅保存非空白元数据，不校验时区数据库或与计划时间的对应关系。
    pub timezone: String,
    /// 投递渠道，去除首尾空白并转为小写；常见微信别名统一为 `wechat`。
    pub channel: String,
    /// 调用方报告的外部状态：`scheduled`、`fired`、`cancelled` 或 `unknown`。
    pub status: String,
    /// 本地关联创建时间，使用 UTC RFC 3339 格式、微秒精度。
    pub created_at: String,
    /// 本地关联最近修改时间，格式同 `created_at`；无变化的编辑不会更新。
    pub updated_at: String,
}

/// 创建本地关联；外部提醒应由调用方先创建成功，再提交这些信息。
#[derive(Debug, Clone)]
pub struct AddReminder {
    /// 非空白外部提醒 ID，去除首尾空白后保存。
    pub external_id: String,
    /// RFC 3339 时间，必须包含 `Z` 或明确偏移；去除首尾空白后保存。
    pub scheduled_at: String,
    /// 非空白时区标签，去除首尾空白后保存，不负责调度或转换时区。
    pub timezone: String,
    /// 非空白渠道，去除首尾空白、转小写，微信别名统一为 `wechat`。
    pub channel: String,
    /// 明确指定 `scheduled`、`fired`、`cancelled` 或 `unknown`，不自动推断。
    pub status: String,
}

/// 局部更新关联；所有 `None` 均表示不修改，字段验证和规范化同创建。
#[derive(Debug, Clone, Default)]
pub struct EditReminder {
    /// 新的外部提醒 ID，不能清空。
    pub external_id: Option<String>,
    /// 新的 RFC 3339 计划时间，必须有明确 UTC 偏移。
    pub scheduled_at: Option<String>,
    /// 新的非空白时区标签。
    pub timezone: Option<String>,
    /// 新渠道；工作任务禁止 `wechat`，包括其常见别名。
    pub channel: Option<String>,
    /// 调用方已确认的外部状态，不触发外部操作或任务状态变化。
    pub status: Option<String>,
}

/// 创建或编辑本地提醒关联的结果。
#[derive(Debug, Serialize)]
pub struct ReminderResult {
    /// 操作后的当前关联记录。
    pub reminder: Reminder,
    /// 是否产生修改和历史；规范化后无变化的编辑为 `false`。
    pub changed: bool,
}

impl Store {
    /// 仅记录调用方已创建的外部提醒；每次调用创建独立的本地关联。
    pub async fn add_reminder(&self, task_id: &str, input: AddReminder) -> Result<ReminderResult> {
        let mut reminder = Reminder {
            id: Uuid::new_v4().to_string(),
            task_id: task_id.to_owned(),
            external_id: input.external_id,
            scheduled_at: input.scheduled_at,
            timezone: input.timezone,
            channel: input.channel,
            status: input.status,
            created_at: String::new(),
            updated_at: String::new(),
        };
        validate(&mut reminder)?;
        let mut tx = self.write().await?;
        let task = get_task(&mut tx, task_id).await?;
        check_channel(&task.category, &reminder)?;
        let at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        reminder.created_at = at.clone();
        reminder.updated_at = at.clone();
        sqlx::query!(
            "INSERT INTO reminders (id, task_id, external_id, scheduled_at, timezone, channel, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            reminder.id,
            reminder.task_id,
            reminder.external_id,
            reminder.scheduled_at,
            reminder.timezone,
            reminder.channel,
            reminder.status,
            reminder.created_at,
            reminder.updated_at,
        )
        .execute(&mut *tx)
        .await?;
        touch_task(&mut tx, task_id, &at).await?;
        record(
            &mut tx,
            task_id,
            "reminder_added",
            &at,
            json!({"before": null, "after": reminder}),
        )
        .await?;
        tx.commit().await?;
        Ok(ReminderResult {
            reminder,
            changed: true,
        })
    }

    /// 按本地创建时间、ID 排序列出关联，在同一读取快照中检查任务是否存在。
    pub async fn list_reminders(&self, task_id: &str) -> Result<Vec<Reminder>> {
        let mut tx = self.pool.begin().await?;
        get_task(&mut tx, task_id).await?;
        let reminders = reminders_for_task(&mut tx, task_id).await?;
        tx.commit().await?;
        Ok(reminders)
    }

    /// 只修改本地元数据；调用方应先确认对应的外部操作已成功。
    pub async fn edit_reminder(&self, id: &str, input: EditReminder) -> Result<ReminderResult> {
        let mut tx = self.write().await?;
        let before = get_reminder(&mut tx, id).await?;
        let mut reminder = before.clone();
        if let Some(value) = input.external_id {
            reminder.external_id = value;
        }
        if let Some(value) = input.scheduled_at {
            reminder.scheduled_at = value;
        }
        if let Some(value) = input.timezone {
            reminder.timezone = value;
        }
        if let Some(value) = input.channel {
            reminder.channel = value;
        }
        if let Some(value) = input.status {
            reminder.status = value;
        }
        validate(&mut reminder)?;
        let task = get_task(&mut tx, &reminder.task_id).await?;
        check_channel(&task.category, &reminder)?;
        if reminder == before {
            tx.commit().await?;
            return Ok(ReminderResult {
                reminder: before,
                changed: false,
            });
        }
        let at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        reminder.updated_at = at.clone();
        sqlx::query!(
            "UPDATE reminders SET external_id = ?, scheduled_at = ?, timezone = ?, channel = ?, status = ?, updated_at = ? WHERE id = ?",
            reminder.external_id,
            reminder.scheduled_at,
            reminder.timezone,
            reminder.channel,
            reminder.status,
            at,
            id,
        )
        .execute(&mut *tx)
        .await?;
        touch_task(&mut tx, &reminder.task_id, &at).await?;
        record(
            &mut tx,
            &reminder.task_id,
            "reminder_updated",
            &at,
            json!({"before": before, "after": reminder}),
        )
        .await?;
        tx.commit().await?;
        Ok(ReminderResult {
            reminder,
            changed: true,
        })
    }

    /// 只移除本地关联，历史保留完整原记录；不删除或取消外部提醒。
    pub async fn remove_reminder(&self, id: &str) -> Result<Reminder> {
        let mut tx = self.write().await?;
        let reminder = get_reminder(&mut tx, id).await?;
        let at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        sqlx::query!("DELETE FROM reminders WHERE id = ?", id)
            .execute(&mut *tx)
            .await?;
        touch_task(&mut tx, &reminder.task_id, &at).await?;
        record(
            &mut tx,
            &reminder.task_id,
            "reminder_removed",
            &at,
            json!({"before": reminder, "after": null}),
        )
        .await?;
        tx.commit().await?;
        Ok(reminder)
    }
}

pub(crate) async fn reminders_for_task(
    conn: &mut SqliteConnection,
    task_id: &str,
) -> Result<Vec<Reminder>> {
    Ok(sqlx::query_as!(
        Reminder,
        "SELECT id, task_id, external_id, scheduled_at, timezone, channel, status, created_at, updated_at FROM reminders WHERE task_id = ? ORDER BY created_at, id",
        task_id,
    )
    .fetch_all(conn)
    .await?)
}

async fn get_reminder(conn: &mut SqliteConnection, id: &str) -> Result<Reminder> {
    sqlx::query_as!(
        Reminder,
        "SELECT id, task_id, external_id, scheduled_at, timezone, channel, status, created_at, updated_at FROM reminders WHERE id = ?",
        id,
    )
    .fetch_optional(conn)
    .await?
    .ok_or_else(|| AppError::NotFound { entity: "reminder", id: id.to_owned() })
}

async fn touch_task(conn: &mut SqliteConnection, task_id: &str, at: &str) -> Result<()> {
    sqlx::query!("UPDATE tasks SET updated_at = ? WHERE id = ?", at, task_id)
        .execute(conn)
        .await?;
    Ok(())
}

fn validate(reminder: &mut Reminder) -> Result<()> {
    reminder.external_id = reminder.external_id.trim().to_owned();
    reminder.scheduled_at = reminder.scheduled_at.trim().to_owned();
    reminder.timezone = reminder.timezone.trim().to_owned();
    reminder.channel = normalize_channel(&reminder.channel);
    for (field, value) in [
        ("external_id", &reminder.external_id),
        ("timezone", &reminder.timezone),
        ("channel", &reminder.channel),
    ] {
        if value.is_empty() {
            return Err(AppError::invalid(field, "Must not be empty or whitespace"));
        }
    }
    DateTime::parse_from_rfc3339(&reminder.scheduled_at).map_err(|_| {
        AppError::invalid(
            "scheduled_at",
            "Must be an RFC 3339 timestamp with an explicit UTC offset",
        )
    })?;
    if !["scheduled", "fired", "cancelled", "unknown"].contains(&reminder.status.as_str()) {
        return Err(AppError::invalid(
            "status",
            "Must be scheduled, fired, cancelled, or unknown",
        ));
    }
    Ok(())
}

fn normalize_channel(channel: &str) -> String {
    let channel = channel.trim().to_lowercase();
    // 微信常见别名允许空格、连字符或下划线，统一后再检查工作任务限制。
    let alias: String = channel
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '-' | '_'))
        .collect();
    match alias.as_str() {
        "wechat" | "weixin" | "wx" | "微信" => "wechat".to_owned(),
        _ => channel,
    }
}

fn check_channel(category: &str, reminder: &Reminder) -> Result<()> {
    if category == "work" && reminder.channel == "wechat" {
        return Err(AppError::ChannelForbidden {
            task_id: reminder.task_id.clone(),
            channel: "wechat",
            reminder_ids: vec![reminder.id.clone()],
        });
    }
    Ok(())
}
