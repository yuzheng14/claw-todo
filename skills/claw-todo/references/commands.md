# 命令与路径

组装命令时读取；返回值和错误码见 [JSON 契约](protocol.md)。下列示例展示参数形式，不是需要整段执行的脚本。`TASK_ID`、`PARENT_ID`、`REMINDER_ID` 必须替换为实际查询或创建返回的 ID，创建键必须对应当前逻辑请求，不能直接复用示例值。

## 数据库

首次运行自动创建目录、数据库并执行内嵌迁移。默认是用户级统一数据库，从不同项目目录运行看到同一份待办：

| 系统 | 默认位置 |
| --- | --- |
| macOS | `$HOME/Library/Application Support/claw-todo/todos.db` |
| Linux | `$XDG_DATA_HOME/claw-todo/todos.db`；未设置时为 `$HOME/.local/share/claw-todo/todos.db` |
| Windows | `%LOCALAPPDATA%\claw-todo\todos.db` |

覆盖优先级为 `--db PATH` > `CLAW_TODO_DB` > 系统默认路径。显式指定的相对路径相对于当前工作目录；跨目录共享自定义库时使用绝对路径。`--db` 和 `--json` 是全局参数，可放在子命令后。帮助和版本查询不打开数据库。

```sh
# 仅在用户要求隔离数据库时使用此类显式路径。
claw-todo --db /tmp/claw-demo/todos.db list --json
```

## 任务操作

下面的 `TASK_ID`、`PARENT_ID`、`REMINDER_ID` 是前一步返回的真实 ID；带空格的文本用引号包裹。

```sh
claw-todo --json create "实现新功能" --category work --project claw \
  --description "依据规格实现" --source SPEC.md --creation-token feature-001
claw-todo --json create "补测试" --category work --parent PARENT_ID --creation-token child-001
claw-todo --json edit TASK_ID --title "完善新功能" --description "补充验收条件"
claw-todo --json edit TASK_ID --clear-description --clear-project --clear-sources
claw-todo --json parent TASK_ID --parent PARENT_ID
claw-todo --json parent TASK_ID --clear-parent
claw-todo --json note TASK_ID "已完成模型与存储，等待验收"
claw-todo --json start TASK_ID
claw-todo --json block TASK_ID --reason "等待上游接口"
claw-todo --json done TASK_ID
claw-todo --json reopen TASK_ID
claw-todo --json cancel TASK_ID --reason "不再需要"
```

- `create TITLE` 必须给 `--category personal|work`；其余参数是 `--description`、`--project`、`--parent`、可重复的 `--source` 和 `--creation-token`。创建后状态是 `pending`。
- `edit ID` 支持 `--title`、`--category`、`--description`、`--project`、可重复的 `--source`，以及上述三个 `--clear-*`。同字段不能同时设置和清空；来源列表为整体替换，不是追加。未提供的字段不改。
- 父级只能通过 `parent ID --parent NEW_ID` 或 `--clear-parent` 修改，两者必须二选一。禁止自身为父级及形成环。
- 状态只能通过显式状态命令修改。开放状态是 `pending`、`in_progress`、`blocked`；关闭状态是 `done`、`cancelled`。
- `block` 必须给非空原因；退出阻塞后当前原因清空，历史仍保留。`cancel` 原因可选；再次 `cancel` 可修正原因，不带 `--reason` 会清空原因，不重置关闭时间。
- `done`／`cancel` 存在开放后代时默认拒绝。明确要关闭整棵子树时加 `--cascade`，只修改开放后代，全部修改和历史原子提交。子任务全部关闭不会自动关闭父任务。
- 已关闭任务必须先 `reopen` 才能转换到其他状态；`reopen` 只重开目标为 `pending`，不能用于重置 `in_progress` 或 `blocked`。关闭祖先须先重开，才能重开后代或新增／移入开放任务。
- 修改普通字段、备注、创建重试不会重开任务。同内容编辑、同状态同原因重试不新增历史；`note` 每次成功都会追加，不能盲目重试。
- 不提供任务硬删除；不再需要的任务请 `cancel`。

## 查询、树与统计

```sh
claw-todo --json list
claw-todo --json list --category work --project claw
claw-todo --json list --status pending,blocked --search "接口"
claw-todo --json list --status done --status cancelled
claw-todo --json list --all
claw-todo --json show TASK_ID
claw-todo --json history TASK_ID
```

默认只查开放任务，一次返回全部结果，没有隐式截断或分页。`--all` 取消默认开放限制；显式 `--status` 则只查列出的状态，即使同时给 `--all` 也一样。多个状态取并集，其余条件与之取交集；分类和项目精确匹配，搜索对标题及描述做转小写后的子串匹配，不是正则或语义搜索。

不带 `--json` 时按树显示，每个任务只出现一次。匹配到子任务时会补齐祖先；祖先若未匹配，标为 `[context only]`，不计入 `Matched`。JSON 对应 `context_only` 与 `summary` 字段，详见协议参考。即使上下文祖先未满足状态条件，也只是展示关系，不代表命中。

`Matched` 是筛选命中数，`open` 是其中开放数。`Open across database` 的 `top-level` 和 `total` 分别是**全库**顶层开放数和全部开放数，不受筛选影响。`direct children` 按全部**直接子任务**统计 `done`、`cancelled`、`total`，不是全层后代、也不是仅可见子任务；取消不计为完成。

相同数据和筛选下顺序稳定：根及兄弟按创建时间、ID 排序。详情包含全部任务字段、直接子任务进度和提醒关联；历史按自增 ID 升序，保留完整修改前后快照。文本中的换行、终端转义符和双向文本控制字符会转义显示；JSON 保留原始字段值。

## 提醒关联

CLI 只保存关联，不调用 OpenClaw、不调度、不发送、不自动同步。先检查任务分类：工作任务禁止微信渠道（`wechat`、`weixin`、`wx`、`微信`，英文不区分大小写）；**不能先创建外部微信提醒，再期待本地检查兜底**。

外部操作成功后，再记录本地结果：

```sh
# 先由调用方创建外部提醒，取得其真实 ID。
claw-todo --json reminder add TASK_ID --external-id EXTERNAL_ID \
  --at 2026-10-01T09:00:00+08:00 --timezone Asia/Shanghai --channel email
claw-todo --json reminder list TASK_ID
claw-todo --json reminder edit REMINDER_ID --at 2026-10-02T09:00:00+08:00
claw-todo --json reminder fired REMINDER_ID
claw-todo --json reminder cancel REMINDER_ID
claw-todo --json reminder remove REMINDER_ID
```

`add` 必填外部 ID、计划时间、时区和渠道，可选 `--status scheduled|fired|cancelled|unknown`，默认为 `scheduled`。`edit` 可改同样的五个字段，省略表示不变；本地关联 ID 与所属任务不可修改。`REMINDER_ID` 是本地 ID，不是 `EXTERNAL_ID`。

`--at` 必须是含 `Z` 或 UTC 偏移的 RFC 3339 时间。`--timezone` 只保存非空时区标签，不校验 IANA 时区、不验证其与偏移的一致性，也不负责计算或调度。渠道去除首尾空白并转小写，微信别名统一为 `wechat`。

外部改期／取消／删除失败时，不得先修改／取消／移除本地关联。`fired` 仅记录调用方已确认的触发结果。关闭待办不自动取消提醒；移除本地关联也不删除外部提醒。每次新增关联都是独立记录，外部 ID 不作唯一键或幂等键；遇到不确定结果先查询、核对，避免重复新增。
