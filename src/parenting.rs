use std::collections::HashSet;

use chrono::{SecondsFormat, Utc};
use serde_json::json;
use sqlx::SqliteConnection;

use crate::{
    AppError, ChangeResult, Result, Store,
    db::{get_task, record},
};

impl Store {
    /// 设置或调整父级；`None` 解除父级，父级不变时不更新时间或历史。
    /// 开放任务要求新位置的全部祖先开放；关闭任务可调整归属，但不重开。
    pub async fn set_parent(&self, id: &str, parent_id: Option<&str>) -> Result<ChangeResult> {
        if parent_id.is_some_and(|value| value.trim().is_empty()) {
            return Err(AppError::invalid("parent_id", "Must not be empty"));
        }
        // 先取得写事务，再读取和检查祖先，防止两个并发移动分别校验通过后形成环。
        let mut tx = self.write().await?;
        let before = get_task(&mut tx, id).await?;
        if before.parent_id.as_deref() == parent_id {
            tx.commit().await?;
            return Ok(ChangeResult {
                task: before,
                changed: false,
            });
        }
        if let Some(parent_id) = parent_id {
            let requires_open = !matches!(before.status.as_str(), "done" | "cancelled");
            validate_parent(&mut tx, parent_id, Some(id), requires_open).await?;
        }
        let mut task = before.clone();
        task.parent_id = parent_id.map(str::to_owned);
        task.updated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        sqlx::query!(
            "UPDATE tasks SET parent_id = ?, updated_at = ? WHERE id = ?",
            task.parent_id,
            task.updated_at,
            id
        )
        .execute(&mut *tx)
        .await?;
        record(
            &mut tx,
            id,
            "parent_changed",
            &task.updated_at,
            json!({"before": before, "after": task}),
        )
        .await?;
        tx.commit().await?;
        Ok(ChangeResult {
            task,
            changed: true,
        })
    }
}

/// 创建和重开复用相同的完整祖先检查。
pub(crate) async fn require_open_ancestors(
    conn: &mut SqliteConnection,
    parent_id: &str,
) -> Result<()> {
    validate_parent(conn, parent_id, None, true).await
}

async fn validate_parent(
    conn: &mut SqliteConnection,
    parent_id: &str,
    task_id: Option<&str>,
    requires_open: bool,
) -> Result<()> {
    let mut current = Some(parent_id.to_owned());
    let mut seen = HashSet::new();
    let mut closed = Vec::new();
    while let Some(id) = current {
        // 新父级不能是目标自身或目标的任意后代，不论目标是否关闭。
        if task_id == Some(id.as_str()) {
            return Err(AppError::CycleDetected {
                task_id: id,
                parent_id: parent_id.to_owned(),
            });
        }
        if !seen.insert(id.clone()) {
            return Err(AppError::invalid(
                "parent_id",
                "The existing parent chain contains a cycle",
            ));
        }
        let parent = get_task(conn, &id).await?;
        if requires_open && matches!(parent.status.as_str(), "done" | "cancelled") {
            closed.push(parent.id);
        }
        current = parent.parent_id;
    }
    if !closed.is_empty() {
        return Err(AppError::ClosedAncestor {
            parent_id: parent_id.to_owned(),
            ancestor_ids: closed,
        });
    }
    Ok(())
}
