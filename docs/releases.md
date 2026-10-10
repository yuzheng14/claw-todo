# Release 维护说明

`.github/workflows/release.yml` 构建并发布 Darwin arm64 二进制及独立 skill 包。当前不提供其他平台的预编译版本，也不进行 Apple Developer ID 签名、公证或 skill registry 发布。

## 触发与产物

- 推送 `v*` tag 时自动运行；也可在 GitHub Actions 的 **Release** 工作流中手动指定一个已有 tag。
- tag 必须为 `v` 加合法 SemVer，并与 `Cargo.toml` 中的包版本完全一致；tag 指向的提交必须已经进入 `main`。
- 使用 `macos-15` 原生 arm64 runner、Rust 1.98.0 和 `aarch64-apple-darwin` target。执行格式检查、Clippy、测试、release 构建及使用隔离数据库的二进制 smoke test；构建使用 `Cargo.lock` 和 SQLx 离线元数据。

部署目标设置为 macOS 11.0（Apple Silicon），实际运行验证在 macOS 15 runner 上进行，不表示已在所有旧系统上测试。

平台选择依据：[GitHub runner 标签](https://docs.github.com/en/actions/how-tos/write-workflows/choose-where-workflows-run/choose-the-runner-for-a-job)、[Rust macOS target 要求](https://doc.rust-lang.org/rustc/platform-support/apple-darwin.html)。

每个 Release 提供以下附件（以 `v0.1.0` 为例）：

| 附件 | 内容 |
| --- | --- |
| `claw-todo-v0.1.0-darwin-arm64.tar.gz` | 同名顶层目录内的 `claw-todo` 可执行文件及 `LICENSE` |
| `claw-todo-skill-v0.1.0.zip` | `claw-todo/` 内的 `SKILL.md`、完整 `references/` 和 `LICENSE` |
| `SHA256SUMS` | 上述两个压缩包的 SHA-256 校验值 |

CLI 与 skill 共用版本号，但可分别安装。skill 的单一源码是 `skills/claw-todo/`；Release 只打包，不向个人 Agent 目录安装内容。若以后接入 ClawHub，可将同一版本的 skill 同步到 registry，不另维护一份技能文档。

## 发布一个版本

1. 在 PR 中修改 `Cargo.toml` 的 `version`，并运行 `SQLX_OFFLINE=true cargo check` 更新 `Cargo.lock` 中本包的版本。检查 lockfile diff，避免夹带无关依赖升级；只修改 skill 时也发布新的共享版本。
2. review 并合并 PR 到 `main`。首次接入流水线时，发布提交必须包含该工作流。
3. 拉取远端状态，确认要发布的提交，再给这个明确的提交打 tag：

   ```sh
   git fetch origin main --tags
   git log -1 --oneline origin/main
   # 将下面两个值替换为待发布版本和已确认的完整提交 SHA。
   release_tag=v0.1.0
   release_commit='填写已确认的完整提交SHA'
   git tag -a "$release_tag" "$release_commit" -m "Release $release_tag"
   git push origin "refs/tags/$release_tag"
   ```

4. 查看 Actions 结果和 Release 附件，下载后按 README 校验、安装。发布前无需手工创建 GitHub Release，也不要移动已经发布的 tag。

首次创建 Release 时，流水线先创建草稿、上传完整附件，再发布并生成 release notes；预发布版本标记为 prerelease。已有 Release 的说明等人工编辑内容会保留。

发布步骤直接写在工作流中：`cargo metadata` 配合 `jq` 核对版本，`git archive`、`tar`、`zip` 打包，`shasum`／`sha256sum` 校验，`gh` 创建和更新 Release。不需要额外的脚本运行时或依赖安装。

## 重跑与更新附件

在 Actions 中手动运行 **Release**，填写已有的 `tag`。默认 `replace_assets=false`：已有且内容一致的附件跳过，缺失的附件补传；同名但内容不同则失败，避免静默替换用户已下载的内容。失败留下的草稿可用同一个 tag 重跑完成。

仅当确实要替换同版本附件时，手动设置 `replace_assets=true`。替换使用 GitHub CLI 的覆盖上传机制，会先删除同名旧附件；若上传失败，旧附件不会自动恢复，应立即重跑完成并重新检查校验文件。不可变（immutable）Release 不允许覆盖附件；这种情况以及一般内容变更，都应发布新的版本，而不是移动 tag 或强行覆盖。

工作流使用仓库自带的 `GITHUB_TOKEN`：构建只需 `contents: read`，发布 job 使用 `contents: write`，无需额外 token 或签名 secrets。仓库或组织策略仍需允许工作流获得相应权限。

新增或修改工作流本身不会创建 tag 或发布版本；只有推送发布 tag，或手动触发工作流后，才会构建并创建／更新 Release。
