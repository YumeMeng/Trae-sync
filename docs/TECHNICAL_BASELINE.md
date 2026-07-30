# Trae Sync 技术基线

## 已验证产品

```text
TRAE Work CN / TRAE SOLO CN
```

当前数据库：

```text
C:\Users\12732\AppData\Roaming\TRAE SOLO CN\ModularData\ai-agent\database.db
```

配套文件：

```text
database.db-wal
database.db-shm
```

## 已验证加密方式

```text
seed: 1CqAknayQsrfH9Byp2QzynTckHGzRom9
salt: 123456789abcdef01122334455667788
KDF: PBKDF2-HMAC-SHA256
iterations: 100000
length: 32 bytes
```

已验证 SQLCipher raw key：

```text
3605f6691095a993f03d5009c918352ef5be31ae31e8f000212b81ff058da773
```

必须使用 raw key 语法：

```sql
PRAGMA key = "x'3605f6691095a993f03d5009c918352ef5be31ae31e8f000212b81ff058da773'";
```

TRAE 内置 SQLCipher 为 `4.5.7`。SQLCipher `4.6.1 community` 已验证兼容。

## 已验证账号归属模型

```text
project.user_id
  -> project.project_id
  -> chat_session.project_id
  -> chat_session.session_id
  -> chat_message.session_id
```

`chat_session` 和 `chat_message` 不直接保存账号 `user_id`。修改 `project.user_id` 已被证明足以让历史跟随目标账号显示。

`ai_agent.dll` 的全局搜索查询还已确认使用：

```sql
FROM chat_session cs
JOIN project p ON cs.project_id = p.project_id
WHERE p.user_id = ?
```

因此，目标账号已存在同一逻辑项目时，可以保持整个会话内容图和所有稳定 ID 不变，只把选中的 `chat_session` 重新挂到目标 `project_id`。

关键唯一约束：

```text
project.project_id UNIQUE
project.(biz_project_id, user_id) UNIQUE
chat_session.session_id UNIQUE
chat_message.message_id UNIQUE
```

## 已完成迁移验证

```text
source: 2578820706078841
target: 1804778984702451
changed project rows: 15
target sessions after transfer: 32
target messages after transfer: 450
cipher_integrity_check: ok
integrity_check: ok
UI confirmation: passed
```

## 已完成第二次迁移验证

验证日期：`2026-07-30`。

```text
source: 1804778984702451
target: 3559551364241212
changed project rows: 17
target projects after transfer: 19
target sessions after transfer: 38
target messages after transfer: 478
all projects / sessions / messages: 20 / 38 / 482
project ID additions or removals: 0
non-user project field changes: 0
unexpected owner changes: 0
foreign key errors: 0
orphan sessions: 0
duplicate project owners: 0
cipher_integrity_check: no errors
integrity_check: ok
post-start main.log account check: passed
post-start alog.log account check: passed
UI history confirmation: pending
```

写前原始数据库三件套、演练副本、写前和写后逻辑副本、写后正式库副本及完整操作清单保存在：

```text
C:\Users\12732\Documents\Trae-History-Backups\20260730-213132-before-1804778984702451-to-3559551364241212
```

全库原有 4 条孤立消息，写前写后数量未增加。第一次正式命令因 PowerShell BOM 影响首行 `.bail on` 而在事务前失败；完整恢复数据库三件套并验证哈希后，修正版先演练再正式提交。后续管道脚本必须把 `PRAGMA key` 放在第一行，`.bail on` 放在第二行。

## 已完成会话级合并副本验证

验证日期：`2026-07-30`。

明文数据库副本：

```text
selected sessions: 7
single-session move: 1
remaining-session merge: 6
messages: 68
history_v2: 3508
server_history_info: 8018
content closure SHA-256 unchanged: yes
reverse reparent: passed
integrity_check: ok
```

SQLCipher 加密数据库副本在事务提交后由第二个连接重开：

```text
target sessions: 7
target messages: 68
session_project mismatch: 0
duplicate (biz_project_id, user_id): 0
FTS orphan sessions: 0
cipher_integrity_check: no errors
integrity_check: ok
```

这项验证证明数据库层可以精细到单个完整 `chat_session`。尚未完成的是隔离 TRAE UI 中的显示、搜索和继续发送消息验证，以及带 `snapshot`、artifact、worktree 等数据的 fixture。

详细记录：`docs/SESSION_MERGE_FEASIBILITY.md`。

## 已验证当前账号证据

当前 `storage.json` 的 `iCubeAuthInfo://*` 值是 Base64 包装的 `tc` 二进制密文，不是参考账号管理器假设的明文 JSON，也不是可直接调用 `CryptUnprotectData` 解开的原始 DPAPI 数据块。

已从产品代码和 10 个真实启动会话确认：

```text
alog.log fetchLogTask.userId = 当前 iCube 用户信息 userId
renderer.log RouteService User info loaded.userId = 当前用户 ID
main.log updateUserInfo/getUserInfo.userId = 当前用户 ID
10 / 10 启动会话至少两类来源一致
```

真实启动序列覆盖：

```text
2578820706078841
1804778984702451
```

`2490859781907946` 已确认是 `deviceId`，不是用户账号 ID。`state.vscdb` 同时保留多个历史账号作用域键，不能单独判断当前账号。

V1 使用“权威日志 userId + 认证字段 SHA-256 指纹 + Local Storage 当前账号作用域”的证据组合。认证字段正文、Token 和 cookies 不进入 Trae Sync 数据库。证据缺失或冲突时停止写入，不从 `project.user_id` 猜测。

详细记录：`docs/ACCOUNT_DETECTION_FEASIBILITY.md`。

## 权威过程文档

```text
E:\系统存储\桌面\TraeWork对话同步\01-直接执行手册-账号历史转移.md
E:\系统存储\桌面\TraeWork对话同步\02-解密逆向与迁移过程记录.md
E:\系统存储\桌面\TraeWork对话同步\03-数据库结构与后续同步模块建议.md
```

后续实现必须以实际产品版本重新预检，不能假设所有 TRAE 产品共享相同路径、密钥或 schema。
