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

/// 显式状态操作；已关闭任务必须先重开，才能改为其他状态。
#[derive(Debug, Clone)]
pub enum Transition {
    /// 开始或继续处理，进入 `in_progress`。
    Start,
    /// 标记阻塞；已经阻塞时只更新原因，不重复记录状态变化。
    Block {
        /// 非空白阻塞原因，保留输入原文。
        reason: String,
    },
    /// 完成任务。
    Done {
        /// 是否同时完成全部开放后代；`false` 时有开放后代就拒绝。
        cascade: bool,
    },
    /// 取消任务；已经取消时允许修正或清空原因，保留原关闭时间。
    Cancel {
        /// 可选取消原因；提供时不能全为空白，`None` 表示清空原因。
        reason: Option<String>,
        /// 是否同时取消全部开放后代；已有关闭后代保持不变。
        cascade: bool,
    },
    /// 只将目标任务重开为 `pending`；全部祖先必须开放，已经 `pending` 时不修改。
    Reopen,
}

/// 创建或幂等重试的结果。
#[derive(Debug, Serialize)]
pub struct CreateResult {
    /// 新建任务或去重命中的已有任务；始终返回当前记录。
    pub task: Task,
    /// `true` 表示命中相同的原始创建请求，没有新增任务或历史记录。
    pub deduplicated: bool,
}

/// 普通编辑、父级调整或追加备注的结果。
#[derive(Debug, Serialize)]
pub struct ChangeResult {
    /// 操作后的当前任务记录。
    pub task: Task,
    /// 是否产生修改和历史；无变化的编辑／父级调整为 `false`，成功追加备注总是 `true`。
    pub changed: bool,
}

/// 状态操作的结果，包括级联实际修改的任务。
#[derive(Debug, Serialize)]
pub struct TransitionResult {
    /// 操作后目标任务的当前记录，不包含后代记录。
    pub task: Task,
    /// 状态或原因是否发生变化；`false` 时不更新时间或新增历史。
    pub changed: bool,
    /// 实际修改的任务 ID；目标在前，后代按 ID 排序，无变化时为空。
    pub affected_task_ids: Vec<String>,
}

/// 与业务修改在同一事务中写入的任务历史记录。
#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// 整张历史表的自增序号，不是任务内编号；历史按此字段升序返回。
    pub id: i64,
    /// 此条历史所属任务的 ID。
    pub task_id: String,
    /// 事件类型：`created`、`edited`、`note`、`status_changed`、`parent_changed`，
    /// 以及 `reminder_added`、`reminder_updated`、`reminder_removed`。
    pub kind: String,
    /// 事件发生时间，使用 UTC RFC 3339 格式、微秒精度。
    pub at: String,
    /// 事件发生时的完整快照：`created` 为 `{before: null, after: Task}`，
    /// `edited`／`status_changed`／`parent_changed` 为 `{before: Task, after: Task}`；
    /// `note` 额外包含 `body`，级联后代的状态事件额外包含根任务 ID `cascade_from`。
    /// 提醒事件使用 `{before: Reminder|null, after: Reminder|null}`，新增前／移除后为 null。
    pub changes: Json<Value>,
}

/// 列表筛选；不同字段取交集，同一状态列表内取并集。
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    /// 未显式筛选状态时，是否连同已关闭任务一起返回。
    pub all: bool,
    /// 按 `personal` 或 `work` 精确筛选。
    pub category: Option<String>,
    /// 按项目名精确筛选；`None` 不限制项目。
    pub project: Option<String>,
    /// 为空时默认只匹配开放状态；非空时优先于 `all`。
    pub statuses: Vec<String>,
    /// 标题或描述中的字面子串，使用 Unicode 小写转换后比较，不是正则或 SQL 通配符。
    pub search: Option<String>,
}

/// 直接子任务的进度，不统计孙辈，也不把取消计作完成。
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Progress {
    /// 已完成的直接子任务数。
    pub done: usize,
    /// 已取消的直接子任务数。
    pub cancelled: usize,
    /// 全部直接子任务数，不受列表筛选条件影响。
    pub total: usize,
}

/// 列表中的任务；JSON 展开任务字段，并附加展示信息。
#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    /// 当前任务记录。
    #[serde(flatten)]
    pub task: Task,
    /// 是否仅为展示层级补入的祖先，不计入筛选命中数量。
    pub context_only: bool,
    /// 未筛选的直接子任务进度。
    pub progress: Progress,
}

/// 命中数与全库开放任务数，均来自同一次读取快照。
#[derive(Debug, Serialize)]
pub struct ListSummary {
    /// 匹配筛选的任务数，不包含纯上下文祖先。
    pub matched: usize,
    /// 命中任务中的开放任务数。
    pub matched_open: usize,
    /// 全库没有父级的开放任务数，不受筛选影响。
    pub top_level_open: usize,
    /// 全库所有层级的开放任务数，不受筛选影响。
    pub total_open: usize,
}

/// 完整查询结果，无隐式分页或条数限制。
#[derive(Debug, Serialize)]
pub struct ListResult {
    /// 命中任务与必要祖先，按创建时间、ID 升序排列，每个任务只出现一次。
    pub tasks: Vec<TaskView>,
    /// 筛选命中数与全库统计。
    pub summary: ListSummary,
}

/// 同一读取快照中的任务、直接子任务进度和全部提醒关联。
#[derive(Debug, Serialize)]
pub struct TaskDetail {
    /// 任务全部当前字段。
    pub task: Task,
    /// 所有直接子任务的进度。
    pub progress: Progress,
    /// 全部状态的提醒关联，按本地创建时间、ID 升序排列。
    pub reminders: Vec<crate::Reminder>,
}
