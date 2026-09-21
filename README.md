# claw-todo

本地待办 CLI，完整产品语义见 [SPEC.md](SPEC.md)。当前是 **第 4 批：状态流转与级联关闭**，在已合并的创建、编辑能力上增加 Rust library 接口；命令行入口将在后续批次接入。

## 本批 review 范围

建议按以下顺序阅读；`Cargo.lock` 和 `.sqlx/` 为生成文件，默认折叠，无需逐行审查。本批不修改第一批的数据库迁移。

1. [任务模型](src/model.rs)：新增 `Transition`、`TransitionResult`，注释说明原因与级联语义。
2. [状态流转](src/transitions.rs)：状态校验、完整后代检查、级联原子提交和逐任务历史。
3. [验收测试](tests/transitions.rs)：状态流转、无变化、祖先约束、级联、并发和整批回滚。

本批不包含父级调整、树列表、提醒操作或 CLI。已有创建流程不变；将编辑流程的历史写入函数移至 `db.rs` 复用，并让重开复用创建时的祖先检查。已有 `ChangeResult` 协议不变。

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

后续按 review 结果继续拆分父级操作、树查询、提醒关联和 CLI，每批附对应测试。完成当前 PR 的 review 后，才创建下一批 PR。
