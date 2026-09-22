# claw-todo

本地待办 CLI，完整产品语义见 [SPEC.md](SPEC.md)。当前是 **第 8 批：CLI 与 JSON 协议**，把已有业务能力接入非交互命令行；默认人类树状输出将在下一批接入。

## 本批 review 范围

建议按以下顺序阅读；`Cargo.lock` 和 `.sqlx/` 为生成文件，默认折叠，无需逐行审查。本批不修改第一批的数据库迁移。

1. [命令与调度](src/cli.rs)：clap derive 参数、业务接口映射、路径选择与帮助处理。
2. [JSON 与错误](src/output.rs)：统一信封、稳定错误码／上下文和退出码；[入口](src/main.rs) 将日志写到 stderr。
3. [命令验收](tests/cli.rs)：覆盖全部子命令、错误／帮助 JSON、参数冲突和跨目录共享数据库。

本批增加 clap、tracing 和 tracing-subscriber，不改迁移或业务写接口。本批业务结果暂时一律输出 JSON，帮助默认文本；下一批只切换非 `--json` 的展示，并补完整使用文档。

## 命令行与 JSON

```sh
SQLX_OFFLINE=true cargo run --locked -- --help
SQLX_OFFLINE=true cargo run --locked -- --db /tmp/claw-demo/todos.db create "试用 CLI" --category personal --creation-token demo-001 --json
SQLX_OFFLINE=true cargo run --locked -- --db /tmp/claw-demo/todos.db list --json
```

支持 `create`、`edit`、`parent`、`start`、`block`、`done`、`cancel`、`reopen`、`note`、`list`、`show`、`history`；`reminder` 下有 `add`、`list`、`edit`、`fired`、`cancel`、`remove`。每条命令均可用 `--help` 查看参数。`edit` 的 `--clear-description/project/sources` 与对应设置值互斥，父级使用独立 `parent ID --parent NEW_ID` 或 `--clear-parent`，两者必须二选一。

`--db PATH` > `CLAW_TODO_DB` > 用户级默认路径；默认库不随工作目录变化，显式相对路径则相对当前目录。`--db`、`--json` 都是全局参数。无参数、帮助及版本查询不会打开数据库，参数错误也不会执行业务操作。字符串值中的字面 `--json`（如 `--title=--json`），以及 `--` 后的文本，不会被当作 JSON 标志。

JSON 成功为 `{"ok":true,"data":...}`；失败为 `{"ok":false,"error":{"code":"...","message":"...","context":{...}}}`。JSON 模式 stdout 只输出一个结果，日志只写 stderr。帮助／版本分别为 `data.help`／`data.version`。业务结果字段沿用下文 Rust 返回类型；TaskView 展开 Task 字段，提醒与历史列表直接为数组，`show` 为 `{task, progress, reminders}`。可选字段为 null，空集合为 []。

| 退出码 | 错误码 | 上下文 |
| --- | --- | --- |
| 0 | 成功 | — |
| 2 | `INVALID_ARGUMENT` / `INVALID_INPUT` | 参数错误为空；业务校验含 `field` |
| 3 | `NOT_FOUND` | `entity`, `id` |
| 4 | `CREATION_TOKEN_CONFLICT` | `creation_token`, `task_id` |
| 4 | `CLOSED_ANCESTOR` | `parent_id`, `ancestor_ids` |
| 4 | `CYCLE_DETECTED` | `task_id`, `parent_id` |
| 4 | `INVALID_STATE` | `task_id`, `status`, `requested_status` |
| 4 | `OPEN_DESCENDANTS` | `task_id`, `blocking_task_ids` |
| 4 | `CHANNEL_FORBIDDEN` | `task_id`, `channel`, `reminder_ids` |
| 5 | `DATABASE_BUSY` | 空，允许有限退避重试 |
| 1 | `DATABASE_ERROR` / `MIGRATION_ERROR` / `IO_ERROR` / `JSON_ERROR` | 空 |

`message` 用于解释，不保证具体措辞。输出失败时业务可能已经提交；重试创建应保留原始请求和创建令牌，备注／新增提醒不能盲目重复。下游主动关闭管道按正常结束处理，不 panic。

## 提醒关联

`Store::add_reminder(task_id, AddReminder)` 和 `edit_reminder(id, EditReminder)` 返回 `{ reminder, changed }`；`list_reminders(task_id)` 按本地创建时间、ID 返回全部关联；`remove_reminder(id)` 返回移除前的完整记录。提醒 ID 是本地 UUIDv4，与 `external_id` 不同。一个任务可关联多个提醒；新增不做外部 ID 去重，编辑不能改变所属任务。

计划时间必须是带 `Z` 或明确偏移的 RFC 3339；时区只是非空标签，不负责校验时区数据库、换算或调度。外部 ID、计划时间、时区去首尾空白；渠道去首尾空白并转小写，`wechat`、`weixin`、`wx`、`微信`（含大小写、空格、横线／下划线变体）统一为 `wechat`。状态严格为 `scheduled`、`fired`、`cancelled`、`unknown`。

新增微信关联、把渠道改为微信、把已关联微信的任务改为 `work`，均返回 `ChannelForbidden`，与提醒是否已触发或取消无关。关闭任务仍可管理关联，任务关闭本身不会修改或移除提醒。

新增、实际编辑和解除分别记录 `reminder_added`、`reminder_updated`、`reminder_removed`，历史为 `{ before: Reminder|null, after: Reminder|null }` 完整快照，同时更新所属任务的 `updated_at`；均在同一个 `BEGIN IMMEDIATE` 事务中提交。规范化后无变化时不更新时间或历史。

**外部操作必须由调用方先完成，再更新本地关联。** 外部删除失败时不能先解除本地关联；创建外部微信提醒前应先检查分类。本工具不调用 OpenClaw、不发送／调度提醒，也不把本地状态当成外部操作成功的证明。

## 列表与详情

`Store::list(ListFilter)` 默认返回全部开放任务，没有分页或隐式数量上限。`all: true` 包含关闭任务；非空 `statuses` 明确指定状态，优先于默认状态和 `all`。分类、项目和状态之间取交集，多个状态取并集。`search` 对标题及描述做不区分大小写的字面子串匹配，`%`、`_` 不作为通配符；空字符串匹配全部。

结果 `tasks` 按 `created_at, id` 稳定排序，并补齐命中任务的全部祖先。补入的祖先标记 `context_only: true`，不计入 `summary.matched` 或 `matched_open`。`summary.top_level_open` 和 `total_open` 始终是数据库全局的顶层／全部开放数，不受筛选影响。一次全表读取保证筛选、上下文和统计来自同一快照。

每个节点的 `progress` 分别统计全部直接子任务的 `done`、`cancelled` 和 `total`，不受筛选影响，也不把孙辈或取消计为完成。`Store::show(id)` 在同一读事务中返回任务、直接子任务进度及全部提醒关联。查询不会修改任务、时间或历史。

## 父级操作

`Store::set_parent(id, Some(parent_id))` 设置或调整直接父级，`Store::set_parent(id, None)` 解除父级，使目标成为顶层任务，返回 `{ task, changed }`。父级 ID 不能全为空白，不自动修剪；目标或新父级不存在时返回 `NotFound`。

目标自身、直接子任务或任意深层后代都不能作为新父级，违反时返回 `CycleDetected { task_id, parent_id }`。开放任务的新父级及其全部祖先必须开放，否则返回 `ClosedAncestor`；已关闭任务可以移入关闭父级，但仍禁止循环，不会隐式重开。解除父级、移出旧位置不要求旧祖先开放。

父级与当前值相同时返回 `changed: false`，不更新时间或历史，包括顶层任务重复解除，以及已关闭任务的同父级重试。实际修改只更新目标的 `parent_id`、`updated_at`，并记录一条 `parent_changed` 历史，内容为 `{ before: Task, after: Task }` 完整快照，事件时间等于更新后的时间。

子树随目标调整归属，但后代各自的直接父级、时间和历史不变；旧父级和新父级自身也不产生额外修改。状态、原因、关闭时间、创建时间和创建幂等信息均保持不变，移动后使用原始创建请求重试仍返回当前记录。

读取、完整祖先校验、父级修改和历史写入都在同一个 `BEGIN IMMEDIATE` 事务中完成，避免并发互设父级形成环，以及关闭与移入竞争留下非法树；任何写入失败全部回滚。

## 状态操作

`Store::transition(id, Transition)` 返回 `{ task, changed, affected_task_ids }`。结果中的任务为目标的当前记录；实际修改的 ID 列表以目标为首，后代按 ID 排序，无变化时列表为空。

- `Start`：开放任务进入 `in_progress`。
- `Block { reason }`：开放任务进入 `blocked`；原因不能全为空白，保留输入原文。
- `Done { cascade }`：完成目标；不级联（`cascade: false`）时，有开放后代就返回 `OpenDescendants`，包含全部层级的阻塞任务 ID（按 ID 排序）。
- `Cancel { reason, cascade }`：取消目标，后代规则同完成；原因可选，提供时不能全为空白。
- `Reopen`：只将已关闭的目标重开为 `pending`，要求全部祖先开放；不会自动重开后代。

已关闭任务不能直接开始、阻塞或切换为另一种关闭状态，必须先显式重开，否则返回 `InvalidState`。对已经 `pending` 的任务重复重开是无变化操作；对 `in_progress`／`blocked` 执行重开则拒绝，避免重置处理进度。

同状态、同原因的重复操作返回 `changed: false`，不更新时间和历史。同状态下修改阻塞／取消原因只记录 `edited`，不伪造状态变化；`Cancel { reason: None, .. }` 明确表示无取消原因，已取消时会清空旧原因，但保留原关闭时间，也不会再次级联。退出阻塞时清空当前阻塞原因；重开时清空关闭时间和取消原因，旧值保留在历史里。

`cascade: true` 只完成／取消目标和全部开放后代，已经关闭的后代保持原样；关闭子任务不自动关闭父任务。后代查询先遍历完整子树，再过滤开放状态，不因关闭的中间节点漏掉更深层后代。级联取消时，所有受影响任务使用本次提供的取消原因。

读取、检查、状态修改和逐任务历史都在一个 `BEGIN IMMEDIATE` 事务内完成；级联中途失败会回滚整批。实际状态变化记录 `status_changed`，包含 `{ before: Task, after: Task }`；级联后代事件额外包含 `cascade_from` 根任务 ID。一次操作的所有变更共用同一 UTC 时间。

状态操作不会修改普通业务字段、父级、创建时间或创建幂等信息，也不删除／更新任何提醒关联；创建重试仍返回任务当前状态。

## 编辑与备注

`Store::edit(id, EditTask)` 支持修改标题、分类、描述、项目和来源引用，返回 `{ task, changed }`。标题、项目及每条来源引用提供时不能全为空白；分类只接受 `personal` 或 `work`，描述允许空字符串。字符串保持原样，不自动修剪。

- `title/category`：`None` 不修改，`Some(value)` 设置。
- `description/project`：`None` 不修改，`Some(None)` 清空，`Some(Some(value))` 设置。描述的空字符串与清空为 `None` 不同。
- `sources`：`None` 不修改，`Some(vec![])` 清空，其他值整体替换，保留顺序与重复引用。

`EditTask` 是 Rust 调用参数，本批不提供它的 JSON 反序列化协议。空更新或所有值均未变化时返回 `changed: false`，不更新时间，也不新增历史。有实际变化时写入一条 `edited` 历史，内容为 `{ before: Task, after: Task }` 完整快照。

`Store::note(id, body)` 追加非空白的进展备注，原样保存正文，更新时间并写入一条 `note` 历史，内容为 `{ body, before: Task, after: Task }`。返回 `{ task, changed: true }`；重复提交相同正文也会追加独立记录，不做备注去重。

编辑和备注都先开启 `BEGIN IMMEDIATE`，再读取当前记录；修改与历史在同一事务提交，失败时一并回滚。已关闭任务可以编辑或追加备注，但状态、关闭时间、父级、创建时间与创建幂等信息均保持不变。不存在的任务返回 `AppError::NotFound`。

改为 `work` 前检查已有全部微信提醒关联，不区分提醒状态；有冲突时返回 `AppError::ChannelForbidden`，包含任务 ID、渠道和按 ID 排序的提醒 ID，不落下部分编辑。

## 已有的创建与查询能力

`Store::create(CreateTask)` 创建 `pending` 任务并返回 `{ task, deduplicated }`；`Store::get(id)` 返回任务当前记录；`Store::history(id)` 按整数事件 ID 升序返回该任务全部历史。不存在的任务返回 `AppError::NotFound`。创建事件的内容为 `{ before: null, after: Task }`，与任务数据在同一事务提交。

`creation_token` 是可选的创建幂等键，Rust 模型、JSON 和数据库字段命名一致；后续 CLI 参数使用 `--creation-token`。相同令牌与完全相同的原始请求返回已有任务的当前状态，并标记 `deduplicated: true`；请求不同则返回 `AppError::CreationTokenConflict`，不覆盖任务。没有令牌或使用不同令牌时独立创建，不根据标题合并。

幂等比较包含原始字符串、可选值和来源数组顺序，不与后来编辑的任务内容比较。去重检查在新建校验之前，因此已有任务或祖先关闭后，原请求仍可安全重试。所有创建调用先开启 `BEGIN IMMEDIATE` 写事务，数据库还对 `creation_token` 施加唯一约束。

创建时标题和分类必填，分类为 `personal` 或 `work`；可选项目、父级、创建令牌及来源引用一旦提供就不能全为空白。父级必须存在，所有祖先必须开放。输入保持原样存储，不自动修剪或读取来源内容。新任务 ID 使用 UUIDv4，程序维护 UTC 时间。

默认路径为用户数据目录下的 `claw-todo/todos.db`，由 `default_db_path()` 提供；调用 `Store::open(path)` 可以显式指定测试数据库。CLI 的路径参数与环境变量将在命令行批次接入。

## 验证

需要 Rust stable 1.94+ 与平台 C 编译工具链。

```sh
SQLX_OFFLINE=true cargo build --locked
SQLX_OFFLINE=true cargo test --locked
SQLX_OFFLINE=true cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all -- --check
```

依赖已缓存时可增加 Cargo 的 `--offline`。固定 SQL 使用编译期查询宏，`.sqlx/` 提供离线元数据，构建不连接个人待办库；源码包同时包含元数据和内嵌迁移。`build.rs` 监控迁移目录变化。

修改 SQL 后，用独立临时数据库更新元数据（需要 SQLx 0.9 CLI）：

```sh
(
  TODO_SCHEMA_DIR=$(mktemp -d)
  export DATABASE_URL="sqlite://$TODO_SCHEMA_DIR/schema.db"
  export SQLX_OFFLINE=false
  cargo sqlx database create --no-dotenv
  cargo sqlx migrate run --no-dotenv
  cargo sqlx prepare --no-dotenv -- --all-targets
  cargo sqlx prepare --no-dotenv --check -- --all-targets
)
```

剩余批次按依赖堆叠 PR：查询 → 提醒关联 → CLI/JSON → 人类输出与文档。每批独立构建并附对应测试，可逐个 review，按依赖顺序合并。
