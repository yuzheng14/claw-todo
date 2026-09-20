use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::types::Json;

/// 任务的当前记录，不是创建时的快照。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Task {
    /// 程序生成的 UUIDv4，后续操作通过此 ID 定位任务。
    pub id: String,
    /// 必填标题，不能全为空白；保留输入原文。
    pub title: String,
    /// 可选的详细描述。
    pub description: Option<String>,
    /// 当前状态：`pending`、`in_progress`、`blocked`、`done` 或 `cancelled`。
    pub status: String,
    /// 任务分类：`personal`（个人）或 `work`（工作）。
    pub category: String,
    /// 可选项目名，不关联独立的项目实体。
    pub project: Option<String>,
    /// 直接父任务的 ID；`None` 表示顶层任务。
    pub parent_id: Option<String>,
    /// 当前阻塞原因；仅 `blocked` 状态下存在且不能全为空白。
    pub blocked_reason: Option<String>,
    /// 可选取消原因；非 `cancelled` 状态时为 `None`。
    pub cancel_reason: Option<String>,
    /// 文件、文档或链接等来源引用；只保存引用，不读取其内容。
    pub sources: Json<Vec<String>>,
    /// 程序维护的创建时间，使用 UTC RFC 3339 格式、微秒精度。
    pub created_at: String,
    /// 最近更新时间，格式同 `created_at`；创建时与其相同。
    pub updated_at: String,
    /// 当前关闭时间，格式同 `created_at`；仅 `done`／`cancelled` 时有值，重开后清空。
    pub closed_at: Option<String>,
    /// 客户端提供的创建幂等键，创建后不可修改；不是认证凭据。
    pub creation_token: Option<String>,
}

/// 原始创建请求；幂等比较保留字符串原文、可选值和来源数组顺序。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateTask {
    /// 必填标题，不能全为空白。
    pub title: String,
    /// 必须明确指定 `personal` 或 `work`，没有默认分类。
    pub category: String,
    /// 可选的详细描述。
    pub description: Option<String>,
    /// 可选项目名；提供时不能全为空白。
    pub project: Option<String>,
    /// 可选父任务 ID；新建时父任务必须存在，且其自身及所有祖先均未关闭。
    pub parent_id: Option<String>,
    /// 来源引用列表，可为空；每条引用不能全为空白，列表顺序参与幂等比较。
    pub sources: Vec<String>,
    /// 可选创建幂等键，提供时不能全为空白；不提供则每次独立创建。
    /// 同键同原始请求返回已有任务的当前状态，同键不同请求返回冲突。
    pub creation_token: Option<String>,
}

/// 普通业务字段的局部更新；不修改状态、父级或创建幂等信息。
#[derive(Debug, Clone, Default)]
pub struct EditTask {
    /// 新标题，不能全为空白；`None` 表示不修改。
    pub title: Option<String>,
    /// 新分类，只能是 `personal` 或 `work`；`None` 表示不修改。
    pub category: Option<String>,
    /// `None` 不修改，`Some(None)` 清空，`Some(Some(value))` 设置描述。
    pub description: Option<Option<String>>,
    /// 三态语义同 `description`；设置的项目名不能全为空白。
    pub project: Option<Option<String>>,
    /// `None` 不修改，`Some(vec![])` 清空，否则整体替换；每条引用不能全为空白。
    pub sources: Option<Vec<String>>,
}

/// 创建或幂等重试的结果。
#[derive(Debug, Serialize)]
pub struct CreateResult {
    /// 新建任务或去重命中的已有任务；始终返回当前记录。
    pub task: Task,
    /// `true` 表示命中相同的原始创建请求，没有新增任务或历史记录。
    pub deduplicated: bool,
}

/// 普通编辑或追加备注的结果。
#[derive(Debug, Serialize)]
pub struct ChangeResult {
    /// 操作后的当前任务记录。
    pub task: Task,
    /// 是否产生修改和历史；无变化的编辑为 `false`，成功追加备注总是 `true`。
    pub changed: bool,
}

/// 与业务修改在同一事务中写入的任务历史记录。
#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// 整张历史表的自增序号，不是任务内编号；历史按此字段升序返回。
    pub id: i64,
    /// 此条历史所属任务的 ID。
    pub task_id: String,
    /// 事件类型；当前支持 `created`、`edited` 和 `note`。
    pub kind: String,
    /// 事件发生时间，使用 UTC RFC 3339 格式、微秒精度。
    pub at: String,
    /// 事件发生时的完整快照：`created` 为 `{before: null, after: Task}`，
    /// `edited` 为 `{before: Task, after: Task}`，`note` 在此基础上增加 `body` 正文。
    pub changes: Json<Value>,
}
