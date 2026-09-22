# claw-todo

本地、单用户、可被人和 Agent 可靠调用的待办 CLI，基于 Rust + clap + SQLx + SQLite + Tokio。任务当前状态以数据库为准，不从聊天或文档自动推断。

## 安装与快速开始

需要 Rust stable 1.94+ 和平台 C 编译工具链：

```sh
SQLX_OFFLINE=true cargo install --path . --locked
claw-todo create "完成 CLI review" --category work --project claw-todo --creation-token review-001
claw-todo list
claw-todo list --json
claw-todo --help
```

默认数据库是用户数据目录下的 `claw-todo/todos.db`，与当前工作目录无关。首次运行自动建库并执行内嵌迁移。用 `--db PATH` 或 `CLAW_TODO_DB` 显式隔离，参数优先于环境变量。试用时建议先指定临时库：

```sh
claw-todo --db /tmp/claw-demo/todos.db create "试用" --category personal
claw-todo --db /tmp/claw-demo/todos.db list
```

## 能力与边界

- 创建、编辑、清空字段、追加备注、状态流转和完整历史；不提供任务硬删除。
- 多层父子树、循环与关闭祖先检查；显式级联关闭原子完成。
- 默认完整列出开放任务，支持组合筛选和搜索；祖先上下文不混入命中数，直接子任务区分完成／取消进度。
- 同一 creation token 与原始请求重试返回当前记录，不会复活已关闭任务；数据库约束和事务保护并发。
- 全命令支持 `--json`，成功／失败信封、稳定错误码和退出码；日志只写 stderr。
- 维护本地提醒关联及历史，工作任务禁止微信。外部操作必须由调用方先完成；CLI 不发送、调度或自动同步提醒。

## 文档

- [使用说明](docs/USAGE.md)：全部命令、树与统计、数据库路径、提醒协作与安装。
- [Agent 调用约定](docs/AGENTS.md)：JSON 字段、错误码、退出码、重试和人工迁移流程。
- [产品规格](SPEC.md)：产品语义与验收约束。

`creation_token` 是创建幂等键，不是认证凭据。历史使用独立自增整数 ID 排序；任务和提醒关联使用 UUIDv4。业务变更与对应历史在同一事务提交。详情及历史保留完整内容，人类输出会转义终端／双向控制字符；JSON 保留字段原值。

## 开发与验证

```sh
SQLX_OFFLINE=true cargo build --locked
SQLX_OFFLINE=true cargo test --locked
SQLX_OFFLINE=true cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all -- --check
SQLX_OFFLINE=true cargo package --locked
```

依赖已缓存时可增加 Cargo 的 `--offline`。固定 SQL 使用编译期查询宏，`.sqlx/` 是必须提交的离线元数据，构建不连接个人待办库。源码包包含元数据、内嵌迁移及使用文档；`build.rs` 监控迁移目录变化。`Cargo.lock` 和 `.sqlx/` 在 GitHub 上标记为生成文件。

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

macOS 若默认 Xcode 尚未接受许可，但已安装 Command Line Tools，可对构建命令临时设置 `DEVELOPER_DIR=/Library/Developer/CommandLineTools`，不必修改系统全局配置。

## 增量 review

前五批为存储、创建、编辑、状态流转与父级调整。所有 PR 的合并目标统一为 `main`，不向其他 review 分支合并。每批保留独立、可构建的实现提交：

1. [第 6 批：查询与树数据](https://github.com/yuzheng14/claw-todo/pull/6) — `model.rs`、`query.rs` 和查询测试。
2. [第 7 批：提醒关联补入 main](https://github.com/yuzheng14/claw-todo/pull/10) — `reminders.rs`、详情整合和提醒测试；原 #7 误以第 6 批分支为目标，补提版本的文件内容不变。
3. [第 8 批：CLI 与 JSON](https://github.com/yuzheng14/claw-todo/pull/8) — `cli.rs`、`output.rs`、`main.rs` 和命令测试。
4. [第 9 批：人类输出与交付文档](https://github.com/yuzheng14/claw-todo/pull/9) — `human.rs`、人类输出测试及本文档。

第 6 批已进入 main，剩余按 #10 → #8 → #9 顺序合并。前置批次尚未进入 main 时，后续 PR 的 diff 会暂时包含依赖内容，可按独立提交 review。前一批 squash/rebase 合并后同步后继分支，消除已审内容的重复展示；所有 PR 的目标分支始终保持 `main`。
