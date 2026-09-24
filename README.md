# claw-todo

本地、单用户、可被人和 Agent 可靠调用的待办 CLI，基于 Rust + clap + SQLx + SQLite + Tokio。任务当前状态以数据库为准，不从聊天或文档自动推断。

## 安装与快速开始

### 预编译二进制

目前只提供 macOS Apple Silicon（Darwin arm64）版本。从 [GitHub Releases](https://github.com/yuzheng14/claw-todo/releases) 选择已发布的版本；以下以 `v0.1.0` 为例，需要 GitHub CLI `gh`：

```sh
release_tag=v0.1.0
release_dir=$(mktemp -d)
gh release download "$release_tag" --repo yuzheng14/claw-todo --dir "$release_dir" \
  --pattern "claw-todo-$release_tag-darwin-arm64.tar.gz" \
  --pattern "claw-todo-skill-$release_tag.zip" \
  --pattern SHA256SUMS
(
  cd "$release_dir" &&
  shasum -a 256 -c SHA256SUMS &&
  tar -xzf "claw-todo-$release_tag-darwin-arm64.tar.gz" &&
  mkdir -p "$HOME/.local/bin" &&
  install -m 755 "claw-todo-$release_tag-darwin-arm64/claw-todo" "$HOME/.local/bin/claw-todo"
)
```

将 `~/.local/bin` 加入自己的 `PATH`，再运行 `claw-todo --version`。也可以从 Release 页面下载上述三个附件后校验、解压安装；`SHA256SUMS` 同时覆盖二进制包和 skill 包。当前二进制未做 Apple Developer ID 签名或公证，不应为此全局关闭 Gatekeeper。

### 从源码安装

需要 Rust stable 1.94+ 和平台 C 编译工具链，在仓库目录执行：

```sh
SQLX_OFFLINE=true cargo install --path . --locked
```

### 快速开始

```sh
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

## Agent skill 与文档

- [claw-todo skill](skills/claw-todo/SKILL.md)：Agent 的触发入口、操作决策、幂等重试和提醒协作规则。
- [命令参考](skills/claw-todo/references/commands.md)：命令、筛选、统计与数据库路径，人类也可直接查阅。
- [JSON 契约](skills/claw-todo/references/protocol.md)：按需读取的返回字段、错误码和退出码。
- [产品规格](SPEC.md)：产品语义与验收约束。

skill 与 CLI 共用版本号、分开安装。同一个 Release 提供 `claw-todo-skill-v<版本>.zip`，解压后将整个 `claw-todo/` 目录（含 `SKILL.md`、`references/` 和许可证）安装到所用 Agent 支持的技能目录，由该 Agent 发现并加载。建议使用与 CLI 相同的版本；源码安装则取对应版本的 `skills/claw-todo/`。

单独克隆仓库或执行 `cargo install` 不会自动注册 skill，也不会改动个人 Agent 配置。先确保该 Agent 能执行 `claw-todo`；缺少 CLI 时 skill 会报告前置条件，不自动安装或另建一份待办。当前通过 GitHub Release 分发，不自动发布到 skill registry；后续可按需同步到 ClawHub。

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

维护者发布步骤、手动重跑与附件更新规则见 [Release 维护说明](docs/releases.md)。

## 增量 review

前五批为存储、创建、编辑、状态流转与父级调整。所有 PR 的合并目标统一为 `main`，不向其他 review 分支合并。每批保留独立、可构建的实现提交：

1. [第 6 批：查询与树数据](https://github.com/yuzheng14/claw-todo/pull/6) — `model.rs`、`query.rs` 和查询测试。
2. [第 7 批：提醒关联补入 main](https://github.com/yuzheng14/claw-todo/pull/10) — `reminders.rs`、详情整合和提醒测试；原 #7 误以第 6 批分支为目标，补提版本的文件内容不变。
3. [第 8 批：CLI 与 JSON](https://github.com/yuzheng14/claw-todo/pull/8) — `cli.rs`、`output.rs`、`main.rs` 和命令测试。
4. [第 9 批：人类输出与交付文档](https://github.com/yuzheng14/claw-todo/pull/9) — `human.rs`、人类输出测试及本文档。

以上实现批次均已进入 `main`；后续 PR 的目标分支继续保持 `main`。
