# claw-todo

本地待办 CLI，完整产品语义见 [SPEC.md](SPEC.md)。当前是 **第 3 批：任务字段编辑与进展备注**，在已合并的创建能力上增加 Rust library 接口；命令行入口将在后续批次接入。

## 本批 review 范围

建议按以下顺序阅读；`Cargo.lock` 和 `.sqlx/` 为生成文件，默认折叠，无需逐行审查。本批不修改第一批的数据库迁移。

1. [任务模型](src/model.rs)：新增 `EditTask`、`ChangeResult`，字段注释说明更新与清空语义。
2. [编辑与备注](src/edits.rs)：局部更新、无变化检测、分类约束、业务与历史同事务。
3. [验收测试](tests/editing.rs)：清空与无变化、非法输入、关闭后编辑、幂等不变、并发更新与失败回滚。

本批不包含状态切换、父级调整、阻塞／取消原因编辑、树列表、提醒操作或 CLI，不提前引入后续接口。

## 编辑与备注

`Store::edit(id, EditTask)` 支持修改标题、分类、描述、项目和来源引用，返回 `{ task, changed }`。标题、项目及每条来源引用提供时不能全为空白；分类只接受 `personal` 或 `work`，描述允许空字符串。字符串保持原样，不自动修剪。

- `title/category`：`None` 不修改，`Some(value)` 设置。
- `description/project`：`None` 不修改，`Some(None)` 清空，`Some(Some(value))` 设置。描述的空字符串与清空为 `None` 不同。
- `sources`：`None` 不修改，`Some(vec![])` 清空，其他值整体替换，保留顺序与重复引用。

`EditTask` 是 Rust 调用参数，本批不提供它的 JSON 反序列化协议。空更新或所有值均未变化时返回 `changed: false`，不更新时间，也不新增历史。有实际变化时写入一条 `edited` 历史，内容为 `{ before: Task, after: Task }` 完整快照。

`Store::note(id, body)` 追加非空白的进展备注，原样保存正文，更新时间并写入一条 `note` 历史，内容为 `{ body, before: Task, after: Task }`。返回 `{ task, changed: true }`；重复提交相同正文也会追加独立记录，不做备注去重。

编辑和备注都先开启 `BEGIN IMMEDIATE`，再读取当前记录；修改与历史在同一事务提交，失败时一并回滚。已关闭任务可以编辑或追加备注，但状态、关闭时间、父级、创建时间与创建幂等信息均保持不变。不存在的任务返回 `AppError::NotFound`。

改为 `work` 前检查已有全部微信提醒关联，不区分提醒状态；有冲突时返回 `AppError::ChannelForbidden`，包含任务 ID、渠道和按 ID 排序的提醒 ID，不落下部分编辑。本批测试用直接 SQL 模拟关闭状态、提醒关联及历史写入故障；对应操作接口留待后续批次。

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

后续按 review 结果继续拆分状态与父级操作、树查询、提醒关联和 CLI，每批附对应测试。完成当前 PR 的 review 后，才创建下一批 PR。
