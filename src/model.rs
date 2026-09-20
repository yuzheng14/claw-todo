use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::types::Json;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub category: String,
    pub project: Option<String>,
    pub parent_id: Option<String>,
    pub blocked_reason: Option<String>,
    pub cancel_reason: Option<String>,
    pub sources: Json<Vec<String>>,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub creation_token: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateTask {
    pub title: String,
    pub category: String,
    pub description: Option<String>,
    pub project: Option<String>,
    pub parent_id: Option<String>,
    pub sources: Vec<String>,
    pub creation_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateResult {
    pub task: Task,
    pub deduplicated: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub task_id: String,
    pub kind: String,
    pub at: String,
    pub changes: Json<Value>,
}
