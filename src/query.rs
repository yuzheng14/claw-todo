use std::collections::{HashMap, HashSet};

use sqlx::types::Json;

use crate::{
    AppError, ListFilter, ListResult, ListSummary, Progress, Result, Store, Task, TaskDetail,
    TaskView, db::get_task,
};

impl Store {
    /// 完整列出命中任务，补入展示所需的祖先；统计基于同一次全表读取。
    pub async fn list(&self, filter: ListFilter) -> Result<ListResult> {
        if let Some(category) = &filter.category
            && !matches!(category.as_str(), "personal" | "work")
        {
            return Err(AppError::invalid(
                "category",
                "Category must be personal or work",
            ));
        }
        for status in &filter.statuses {
            if !matches!(
                status.as_str(),
                "pending" | "in_progress" | "blocked" | "done" | "cancelled"
            ) {
                return Err(AppError::invalid(
                    "status",
                    format!("Unknown task status: {status}"),
                ));
            }
        }
        // 单条 SELECT 取得一致快照；本地单用户数据规模下，在内存中筛选便于保留完整树上下文。
        let all = sqlx::query_as!(Task,
            r#"SELECT id, title, description, status, category, project, parent_id,
                blocked_reason, cancel_reason, sources AS "sources: Json<Vec<String>>",
                created_at, updated_at, closed_at, creation_token FROM tasks ORDER BY created_at, id"#
        ).fetch_all(&self.pool).await?;
        let search = filter.search.as_deref().map(str::to_lowercase);
        let matched: HashSet<&str> = all
            .iter()
            .filter(|task| {
                let status_matches = if filter.statuses.is_empty() {
                    filter.all || is_open(&task.status)
                } else {
                    filter.statuses.contains(&task.status)
                };
                status_matches
                    && filter
                        .category
                        .as_ref()
                        .is_none_or(|value| value == &task.category)
                    && filter
                        .project
                        .as_ref()
                        .is_none_or(|value| Some(value) == task.project.as_ref())
                    && search.as_ref().is_none_or(|needle| {
                        task.title.to_lowercase().contains(needle)
                            || task
                                .description
                                .as_ref()
                                .is_some_and(|text| text.to_lowercase().contains(needle))
                    })
            })
            .map(|task| task.id.as_str())
            .collect();
        let summary = ListSummary {
            matched: matched.len(),
            matched_open: all
                .iter()
                .filter(|task| matched.contains(task.id.as_str()) && is_open(&task.status))
                .count(),
            top_level_open: all
                .iter()
                .filter(|task| task.parent_id.is_none() && is_open(&task.status))
                .count(),
            total_open: all.iter().filter(|task| is_open(&task.status)).count(),
        };
        let by_id: HashMap<&str, &Task> = all.iter().map(|task| (task.id.as_str(), task)).collect();
        let mut visible = matched.clone();
        for id in &matched {
            let mut parent = by_id.get(id).and_then(|task| task.parent_id.as_deref());
            while let Some(parent_id) = parent {
                // 已加入的祖先链无需再次遍历，亦避免异常数据中的无限循环。
                if !visible.insert(parent_id) {
                    break;
                }
                parent = by_id
                    .get(parent_id)
                    .and_then(|task| task.parent_id.as_deref());
            }
        }
        let mut progress: HashMap<&str, Progress> = HashMap::new();
        for task in &all {
            if let Some(parent) = task.parent_id.as_deref() {
                let entry = progress.entry(parent).or_default();
                entry.total += 1;
                entry.done += usize::from(task.status == "done");
                entry.cancelled += usize::from(task.status == "cancelled");
            }
        }
        let tasks = all
            .iter()
            .filter(|task| visible.contains(task.id.as_str()))
            .map(|task| TaskView {
                task: task.clone(),
                context_only: !matched.contains(task.id.as_str()),
                progress: progress.get(task.id.as_str()).cloned().unwrap_or_default(),
            })
            .collect();
        Ok(ListResult { tasks, summary })
    }

    /// 在同一读事务中读取任务和直接子任务进度。
    pub async fn show(&self, id: &str) -> Result<TaskDetail> {
        let mut tx = self.pool.begin().await?;
        let task = get_task(&mut tx, id).await?;
        let statuses = sqlx::query!("SELECT status FROM tasks WHERE parent_id = ?", id)
            .fetch_all(&mut *tx)
            .await?;
        let progress = Progress {
            done: statuses
                .iter()
                .filter(|child| child.status == "done")
                .count(),
            cancelled: statuses
                .iter()
                .filter(|child| child.status == "cancelled")
                .count(),
            total: statuses.len(),
        };
        tx.commit().await?;
        Ok(TaskDetail { task, progress })
    }
}

fn is_open(status: &str) -> bool {
    matches!(status, "pending" | "in_progress" | "blocked")
}
