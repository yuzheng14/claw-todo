# claw-todo

本地待办 CLI，完整产品语义见 [SPEC.md](SPEC.md)。当前分支是 **第 1 批：存储基础**，提供可独立构建、测试的 Rust library。

## 本批 review 范围

建议按以下顺序阅读；`Cargo.lock` 为生成文件，无需逐行审查。

1. [数据库迁移](migrations/202609200001_initial.sql)：任务、历史、提醒三张表；令牌唯一及不可变约束；状态字段与微信渠道约束。
2. [数据库初始化](src/db.rs)：用户级数据路径、自动创建目录、外键、WAL、2 个连接、锁等待及并发迁移。
3. [基础错误](src/error.rs) 和 [工程配置](Cargo.toml)：错误传播、依赖与迁移嵌入。

数据表先定义完整关系，后续批次在此基础上增加业务操作。当前测试覆盖 6 路同时首次打开数据库，验证迁移仅应用一次，以及外键和 WAL 配置生效。状态机、层级和历史写入的业务保证由后续批次补齐。

`tasks.creation_token` 是调用方提供的创建幂等键，与不可变的 `creation_request` 一起用于识别同一创建请求的重试。后续 Rust 模型及 JSON 字段统一使用 `creation_token`，CLI 参数使用 `--creation-token`。

默认路径为用户数据目录下的 `claw-todo/todos.db`，由 `default_db_path()` 提供；调用 `Store::open(path)` 可以显式指定测试数据库。CLI 的路径参数与环境变量将在命令行批次接入。

## 验证

需要 Rust stable 1.94+ 与平台 C 编译工具链。

```sh
cargo build --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all -- --check
```

依赖已缓存时可增加 `--offline`。本批没有查询宏，无需个人待办库或 `.sqlx`；迁移使用 `sqlx::migrate!()` 内嵌，`build.rs` 监控迁移目录变化。

后续按 review 结果依次提交任务操作、提醒关联及 CLI；任务部分会继续拆小，每批附对应测试。完成当前 PR 的 review 后，才创建下一批 PR。
