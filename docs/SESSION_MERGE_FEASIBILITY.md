# TRAE Work CN 会话级合并可行性验证

## 1. 结论

在 TRAE Work CN `1.107.1` 的同一个活动数据库中，两个账号拥有同一逻辑项目的不同对话时，可以把来源项目中的一条或多条完整对话合并到目标账号已有项目下。

这里不需要复制整套消息图，也不需要重新生成 `session_id`、`message_id`、`turn_id` 或 `task_id`。正确做法是保留整个会话图不变，只把选中的 `chat_session` 重新挂接到目标 `project_id`。

此前“V1 只能按项目执行、不能单独同步一条对话”的结论过于保守。它把以下两种操作混为一谈：

```text
创建第二份独立会话副本   需要克隆完整关系图并重写 ID
让现有会话跟随目标项目   只需重挂项目关系，ID 保持不变
```

Trae Sync 的“历史跟随当前账号”需要的是第二种操作。

## 2. 真实数据关系

账号可见性查询的核心关系是：

```text
project.user_id
  -> project.project_id
  -> chat_session.project_id
  -> chat_session.session_id
  -> 其余会话内容表
```

`ai_agent.dll` 中的真实全局搜索 SQL 包含：

```sql
FROM chat_session cs
JOIN project p ON cs.project_id = p.project_id
WHERE cs.deleted_at = 0
  AND cs.hidden_status IS NULL
  AND (p.deleted_at IS NULL OR p.deleted_at = 0)
  AND p.user_id = ?
```

DLL 同时存在 `get_lite_sessions_by_projects` 和 `list_chat_sessions_by_projects` 路径。因此，把一个会话的 `chat_session.project_id` 改为目标账号项目行的 `project_id`，会让该会话进入目标账号的项目与搜索查询结果。

当前样本中，`chat_session` 与 `session_project` 是一致的一对一挂接：

```text
chat_session rows:           34
session_project rows:        34
missing matching relation:    0
extra relation:               0
multi-project sessions:       0
```

## 3. 会话级合并操作

对选中的 `session_id` 集合，事务至少需要更新：

```text
chat_session.project_id
session_project.project_id
snapshot.project_id            按 chat_session_id 限定
staging.project_id             按 chat_session_id 限定
local_artifact.source_project_id / user_id
local_artifact_version.source_project_id
```

当前真实样本中后四类表没有相关行，但适配器不能因此省略处理逻辑。

以下内容不需要改 ID 或复制：

```text
chat_message
chat_message_general
chat_message_task
chat_turn
task
history_v2
history_todo_list
agent_run
rules_attachment
server_history_info
FTS 数据
```

它们通过保持不变的 `session_id`、`message_id`、`task_id`、`turn_id` 和 `agent_run_id` 继续关联。

正文 JSON 中发现的项目或会话 ID 位于历史工具调用参数、命令输出和文件路径中，属于对话正文证据，不应做字符串替换。

## 4. 明文数据库副本验证

测试只操作迁移前明文副本，不接触活动数据库。

构造方式：复制一个真实项目行到目标账号，保持相同 `biz_project_id`、路径和项目元数据，并生成新的 `project_id`；先把 1 条真实会话挂到目标项目，形成两个账号在同一逻辑项目下各有历史的场景，再合并剩余会话。

结果：

```text
真实项目会话数：                     7
先单独移动：                         1
移动后来源 / 目标：                  6 / 1
再合并剩余：                         6
合并后来源 / 目标：                  0 / 7

会话消息：                          68
history_v2：                      3508
server_history_info：             8018

会话内容闭包 SHA-256：              前后一致
session_project 不一致：             0
重复 (biz_project_id, user_id)：     0
PRAGMA integrity_check：             ok
```

随后把全部 7 条会话反向挂回来源项目：

```text
反向后来源 / 目标：                  7 / 0
会话内容闭包 SHA-256：              仍然一致
PRAGMA integrity_check：             ok
```

这同时验证了单条对话粒度、项目内全部对话合并和可逆性。

## 5. SQLCipher 加密副本验证

使用迁移前 `database.db`、`database.db-wal`、`database.db-shm` 的独立副本，在 SQLCipher `4.6.1 community` 中重复相同核心事务。

事务提交后，使用第二个独立连接重新打开加密数据库，结果为：

```text
schema objects：                       178
符合条件的合并项目对：                  1
目标项目会话：                          7
目标项目消息：                         68
session_project 不一致：                0
重复 (biz_project_id, user_id)：        0
FTS 孤立会话：                          0
cipher_integrity_check：                无错误
integrity_check：                       ok
```

这证明会话级重挂接可以直接在原 SQLCipher 数据格式中完成，不依赖先导出明文库。

## 6. 数据库外数据

当前文件系统证据：

```text
snapshot/<session_id>/...
sandbox/<project_id>.json
sandbox/<project_id>-hooks.json
```

会话快照目录以保持不变的 `session_id` 命名，重挂接不需要移动该目录。

项目 sandbox 配置以 `project_id` 命名。目标项目已经存在且确实代表同一工作区时，应使用目标项目配置；目标配置缺失时，必须在副本测试中验证复制配置并修改 JSON `name` 后的行为，不能只改数据库后假定工具执行可用。

同路径不一定表示同一项目。真实样本中出现了 `virtual` 与 `unsaved_multi_root` 项目指向相同目录但语义不同的情况，所以项目匹配必须同时比较 `biz_project_id`、路径、`workspace_status`、`work_mode`、remote 和 fallback 关系。

## 7. 两账号同项目时的处理规则

```text
目标账号没有同一项目
  -> 继续使用 project.user_id 归属转移，改动最少

目标账号已有同一项目，双方 session_id 不同
  -> 选择目标项目行，将来源会话逐条重挂，形成对话并集

同一 session_id、内容完全相同
  -> 活动库中本来只能有一条 chat_session；计划中去重并跳过

同一逻辑会话在不同快照中为严格扩展
  -> 历史库可选较完整版本；写回旧归档版本属于另一项恢复能力

同一逻辑会话内容分叉
  -> 两个版本都保留，不自动拼接消息正文

仅标题相同、session_id 不同
  -> 默认作为两条独立对话保留，不因标题相同删除
```

## 8. 尚未完成的验证

数据库结构和加密副本操作已经验证，但以下内容仍需真实 UI Gate：

1. 在隔离副本中启动 TRAE，确认目标账号列表、搜索和对话正文均正常显示。
2. 打开重挂后的旧会话并发送一轮新消息，确认新旧 `server_history_info.user_id` 共存不会影响继续对话。
3. 覆盖存在 `snapshot`、`staging`、`local_artifact`、`core_memory`、定时任务和 worktree 的会话 fixture。
4. 验证目标 sandbox 配置不存在、不同或损坏时的明确阻断规则。
5. 验证真实分叉快照写回；这不是活动数据库会话重挂接的一部分。

在这些 Gate 完成前，可以确认“数据库层可精细到单条完整对话”，但不能把“所有类型会话均已通过 TRAE 继续编辑验证”写成已完成。
