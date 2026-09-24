# claw-todo

本地待办 CLI，完整产品语义见 [SPEC.md](SPEC.md)。当前是 **第 6 批：完整查询与树数据**，在前序批次上增加 Rust library 查询接口；命令行入口将在后续批次接入。

## 本批 review 范围

建议按以下顺序阅读；`Cargo.lock` 和 `.sqlx/` 为生成文件，默认折叠，无需逐行审查。本批不修改第一批的数据库迁移。

1. [查询模型](src/model.rs)：筛选条件、上下文标记、直接子任务进度与统计。
2. [查询实现](src/query.rs)：完整读取、组合筛选、补齐祖先、稳定排序与详情。
3. [验收测试](tests/query.rs)：默认开放集、关闭状态、搜索、上下文计数及 1,100 条不截断。

本批不包含提醒操作、命令行或终端渲染，不改数据库迁移、依赖或已有写接口。

## 列表与详情

`Store::list(ListFilter)` 默认返回全部开放任务，没有分页或隐式数量上限。`all: true` 包含关闭任务；非空 `statuses` 明确指定状态，优先于默认状态和 `all`。分类、项目和状态之间取交集，多个状态取并集。`search` 对标题及描述做不区分大小写的字面子串匹配，`%`、`_` 不作为通配符；空字符串匹配全部。

结果 `tasks` 按 `created_at, id` 稳定排序，并补齐命中任务的全部祖先。补入的祖先标记 `context_only: true`，不计入 `summary.matched` 或 `matched_open`。`summary.top_level_open` 和 `total_open` 始终是数据库全局的顶层／全部开放数，不受筛选影响。一次全表读取保证筛选、上下文和统计来自同一快照。

每个节点的 `progress` 分别统计全部直接子任务的 `done`、`cancelled` 和 `total`，不受筛选影响，也不把孙辈或取消计为完成。`Store::show(id)` 在同一读事务中返回任务及直接子任务进度。查询不会修改任务、时间或历史。

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

改为 `work` 前检查已有全部微信提醒关联，不区分提醒状态；有冲突时返回 `AppError::ChannelForbidden`，包含任务 ID、渠道和按 ID 排序的提醒 ID，不落下部分编辑。此前编辑测试中的关闭状态使用 SQL fixture；提醒操作接口留待后续批次。

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
