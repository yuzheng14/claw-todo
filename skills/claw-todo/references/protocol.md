# JSON 契约

解析 `claw-todo --json` 的返回值或处理错误时读取。操作决策与重试约束见 [skill 入口](../SKILL.md)，命令参数见 [命令参考](commands.md)。

## JSON 外壳与命令结果

成功为 `{"ok":true,"data":...}`，失败为 `{"ok":false,"error":{"code":"...","message":"...","context":{...}}}`。可选字段缺省时为 JSON `null`，集合为空时是 `[]`，不是省略；新增兼容字段时调用方应忽略未知字段。

| 命令 | `data` |
| --- | --- |
| `create` | `{task: Task, deduplicated: boolean}` |
| `edit`、`parent`、`note` | `{task: Task, changed: boolean}` |
| `start`、`block`、`done`、`cancel`、`reopen` | `{task: Task, changed: boolean, affected_task_ids: string[]}` |
| `list` | `{tasks: TaskView[], summary: ListSummary}` |
| `show` | `{task: Task, progress: Progress, reminders: Reminder[]}` |
| `history` | `HistoryEntry[]` |
| `reminder add/edit/fired/cancel` | `{reminder: Reminder, changed: boolean}` |
| `reminder list` | `Reminder[]` |
| `reminder remove` | 被移除的 `Reminder` 完整记录 |
| 帮助、版本 | `{help: string}` 或 `{version: string}` |

`changed: false` 表示没有新修改和历史。状态结果中的 `affected_task_ids` 是实际修改对象，目标在前、后代按 ID 排序；无变化为空。普通字段修改没有 `affected_task_ids`。`note` 每次成功均追加。

### 记录字段

`Task` 的字段：

| 字段 | 类型与含义 |
| --- | --- |
| `id` | string，程序生成的 UUIDv4，稳定任务标识 |
| `title` | string，非空白标题，保留输入原文 |
| `description` | string 或 null，描述 |
| `status` | `pending`、`in_progress`、`blocked`、`done`、`cancelled` |
| `category` | `personal` 或 `work` |
| `project` | string 或 null，项目名 |
| `parent_id` | string 或 null，直接父级 ID |
| `blocked_reason` | string 或 null，仅当前阻塞时存在 |
| `cancel_reason` | string 或 null，仅取消时可有值 |
| `sources` | string[]，来源引用，顺序保留 |
| `created_at`、`updated_at` | string，UTC RFC 3339，微秒格式 |
| `closed_at` | string 或 null，当前关闭时间，重开后为 null |
| `creation_token` | string 或 null，创建幂等键，创建后不可修改 |

`TaskView` 将全部 `Task` 字段平铺在同一对象上，另有 `context_only: boolean` 和 `progress: Progress`。不是 `{task: ...}` 嵌套结构。

`Progress` 为 `{done: number, cancelled: number, total: number}`，统计全部直接子任务，包含未命中筛选的任务，不把取消算完成。

`ListSummary` 为 `{matched, matched_open, top_level_open, total_open}`，均为非负整数；前两项只算真正命中，后两项算全库开放任务，不受筛选影响。筛选命中子任务时会补齐祖先，`context_only: true` 的行不是匹配结果。默认只匹配开放任务；`--all` 或显式关闭 `--status` 才能匹配关闭任务。显式状态优先于 `--all`；可重复或逗号分隔多个状态。

`tasks` 是一次完整快照的平铺数组，按 `created_at, id` 稳定排序，不分页、不截断；调用方按 `parent_id` 构建树。非匹配祖先不应算成额外待办；不要拿 `tasks.length` 代替 `summary.matched`。搜索是标题和描述转小写后的字面子串匹配；其他过滤条件取交集。

`Reminder` 包含字符串字段 `id`、`task_id`、`external_id`、`scheduled_at`、`timezone`、`channel`、`status`、`created_at`、`updated_at`。`id` 是本地 UUIDv4，`external_id` 是外部提醒 ID；两者不要混用。提醒状态仅为 `scheduled`／`fired`／`cancelled`／`unknown`。列表按本地创建时间、ID 排序。

`scheduled_at` 是带时区偏移的 RFC 3339，保留输入偏移及精度；`timezone` 是非空元数据标签，不做时区数据库或偏移一致性校验。外部 ID、计划时间、时区去首尾空白；渠道还会转小写，将 `wechat`／`weixin`／`wx`／`微信` 统一成 `wechat`。

`HistoryEntry` 为 `{id: number, task_id: string, kind: string, at: string, changes: object}`，按全表自增 `id` 升序返回。`at` 是 UTC RFC 3339 微秒格式，`changes` 不截断：

- `created`：`{before: null, after: Task}`。
- `edited`、`status_changed`、`parent_changed`：`{before: Task, after: Task}`；级联后代的状态事件另有 `cascade_from: string`。
- `note`：`{before: Task, after: Task, body: string}`。
- `reminder_added`：`{before: null, after: Reminder}`。
- `reminder_updated`：`{before: Reminder, after: Reminder}`。
- `reminder_removed`：`{before: Reminder, after: null}`。

同状态只修改原因使用 `edited`，不制造假的 `status_changed`。业务修改与相应历史在同一事务内提交。

## 错误与退出码

成功退出 `0`。失败码及必要上下文如下；`message` 供人读，不作程序接口：

| 退出码 | `error.code` | `context`／处理方式 |
| --- | --- | --- |
| 2 | `INVALID_ARGUMENT` | 命令行解析错误；修正选项或必填参数 |
| 2 | `INVALID_INPUT` | `field`；修正字段值 |
| 3 | `NOT_FOUND` | `entity`、`id`；核对真实 ID |
| 4 | `CREATION_TOKEN_CONFLICT` | `creation_token`、`task_id`；不能覆盖原创建请求 |
| 4 | `CLOSED_ANCESTOR` | `parent_id`、`ancestor_ids`；需明确决定是否先重开祖先 |
| 4 | `CYCLE_DETECTED` | `task_id`、`parent_id`；修正父级 |
| 4 | `INVALID_STATE` | `task_id`、`status`、`requested_status`；核对状态与用户意图 |
| 4 | `OPEN_DESCENDANTS` | `task_id`、`blocking_task_ids`；先处理后代，或取得级联意图后用 `--cascade` |
| 4 | `CHANNEL_FORBIDDEN` | `task_id`、`channel`、`reminder_ids`；工作任务不能关联微信 |
| 5 | `DATABASE_BUSY` | 有限次数退避重试；先核对非幂等操作的实际结果 |
| 1 | `DATABASE_ERROR`、`MIGRATION_ERROR`、`IO_ERROR`、`JSON_ERROR` | 存储／迁移／I/O／序列化问题，不要重复业务写入掩盖错误 |

例：

```json
{"ok":false,"error":{"code":"NOT_FOUND","message":"task missing was not found","context":{"entity":"task","id":"missing"}}}
```

空库成功例：

```json
{"ok":true,"data":{"tasks":[],"summary":{"matched":0,"matched_open":0,"top_level_open":0,"total_open":0}}}
```
