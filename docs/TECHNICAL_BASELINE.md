# Trae Sync 技术基线

## 已验证产品

```text
TRAE Work CN / TRAE SOLO CN
```

当前数据库：

```text
%APPDATA%\TRAE SOLO CN\ModularData\ai-agent\database.db
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

生产探测必须在解密成功后校验以下 SQLCipher 4 关键参数；任一查询失败或不匹配都保持只读：

```text
cipher_page_size = 4096
kdf_iter = 256000
cipher_hmac_algorithm = HMAC_SHA512
cipher_kdf_algorithm = PBKDF2_HMAC_SHA512
```

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

以下 `account-A`、`account-B`、`account-C` 为跨文档保持一致的脱敏账号标签。

```text
source: account-A
target: account-B
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
source: account-B
target: account-C
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
<backup-root>\20260730-213132-before-account-B-to-account-C
```

全库原有 4 条孤立消息，写前写后数量未增加。第一次正式命令因 PowerShell BOM 影响首行 `.bail on` 而在事务前失败；完整恢复数据库三件套并验证哈希后，修正版先演练再正式提交。后续管道脚本必须把 `PRAGMA key` 放在第一行，`.bail on` 放在第二行。

## 已完成第三次迁移验证

验证日期：`2026-08-01`。产品版本：`TRAE SOLO CN 0.1.43`，文件版本：`2.3.62834`。

```text
source: account-C
target: account-A
source before: 20 projects / 49 sessions / 676 messages
target before: 1 project / 0 sessions / 0 messages
same biz_project_id conflicts: 1（双方均为空项目，保留不删）
changed project rows: 19
source after: 1 empty project / 0 sessions / 0 messages
target after: 20 projects / 49 sessions / 676 messages
all projects / sessions / messages: 21 / 49 / 680
project ID additions or removals: 0
non-user project field changes: 0
unexpected owner changes: 0
schema differences: 0
non-project table hash differences: 0
foreign key errors: 0
orphan sessions: 0
duplicate project owners: 0
cipher_integrity_check: no errors
integrity_check: ok
post-start main.log account check: passed
post-start alog.log account check: passed
post-start database error scan: passed
UI history confirmation: pending
```

演练写后与正式写后逻辑库 SHA-256 完全相同。写前原始三件套、演练副本、逻辑副本、写后冻结副本和完整操作清单保存在：

```text
<backup-root>\20260801-205004-before-account-C-to-account-A
```

## 已完成第四次迁移验证

验证日期：`2026-08-03`。产品版本：`TRAE SOLO CN 0.1.43`。

```text
source: account-A
target: account-B
source before: 20 projects / 55 sessions / 742 messages
target before: 0 projects / 0 sessions / 0 messages
same biz_project_id conflicts: 0
changed project rows: 20
source after: 0 projects / 0 sessions / 0 messages
target after: 20 projects / 55 sessions / 742 messages
all projects / sessions / messages: 21 / 55 / 746
foreign key errors: 0
orphan sessions: 0
duplicate project owners: 0
cipher_integrity_check: no errors
integrity_check: ok
post-start main.log account check: passed
post-start alog.log account check: passed
post-start database error scan: passed
UI history confirmation: pending
```

本次针对耗时进行收敛：保留完整三件套备份、副本预检、事务演练、事务断言和一次最终完整性检查；省略已重复证明的明文导出与逐表 SHA3。操作清单：

```text
<backup-root>\20260803-141808-before-account-A-to-account-B\OPERATION_MANIFEST.md
```

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

详细记录：原 `docs/SESSION_MERGE_FEASIBILITY.md`（2026-08-22 清理，git 历史可查；本节事实仍有效）。

## SQLCipher 逻辑副本导出契约（工程规程，Phase 3 主库原语沿用）

来源：原 ADR-0008（2026-08-22 文档清理时并入本基线）。要点：

1. Rust 依赖内的 `sqlite3_backup_*` 对 SQLCipher 加密库返回 `backup is not supported with encrypted databases`，不可用；逻辑副本 = 通过已解密连接执行 `sqlcipher_export('dst')` 生成的单文件副本。
2. 导出前记录源 DB/WAL/SHM 的存在性与 SHA-256，源三件套不得被修改；源连接读取必须纳入 WAL 已提交记录。
3. 目标连接 `synchronous=FULL` 或更强，禁止为提速关闭同步；`sqlcipher_export()` 成功后必须 `DETACH`、关闭连接、`sync_all()`，任一步失败不得返回成功。
4. 耐久同步后由独立连接对目标副本执行 `PRAGMA cipher_integrity_check` 与 `PRAGMA integrity_check`，并校验关键 schema 指纹与记录数。

## 已验证新设备注册与签到闭环（2026-08-25）

目标账号：`2873473361250299`。本节只记录已通过真实 HTTP 的路线，禁止用 refresh-mode 新设备铸造替代。

### 可重复设备铸造

当前 App 凭据可通过 `x-cloudide-token: <access token>` 请求：

```text
POST /cloudide/api/v3/trae/oauth/GetPCAuthCode
```

请求体使用新的 PKCE 对和新的 16 位 `DeviceID`：

```text
ClientID: en1oxy7wnw8j9n
CodeChallengeMethod: S256
RedirectURI: http://127.0.0.1:18963/authorize
PlatformCode: SOLO_PC
```

随后使用 AuthCode + CodeVerifier 调用：

```text
POST /trae/api/v3/oauth/ExchangeToken
```

实测必须保留真实 SOLO 客户端设备字段：

```text
PlatformCode: SOLO_PC
DeviceType: PC
DeviceModel: 81Q5
ClientVersion / IDEVersion: 0.1.54
DeviceBrand: LENOVO
DeviceCPU: Intel(R) Core(TM) i7-9750H CPU
OSInfo: windows
OSVersion: Windows 11 Home
DevicePublicKey: EC P-256 公钥
MachineID: 每次新设备随机值
```

2026-08-25 13:32（UTC+8）连续两次使用不同新设备均得到：`GetPCAuthCode HTTP 200`、`ExchangeToken HTTP 200`、返回账号匹配目标，随后只读 `status HTTP 200 code=0`。证据：
`.scratch/checkin-http/reports/p01-authcode-repeat-mint-20260825.json`。

### 首签判定

2026-08-25 13:20（UTC+8）首次完整闭环已通过：

```text
status-before: HTTP 200, code=0, checked_in=false
claim:         HTTP 200, code=0
status-after:  HTTP 200, code=0, checked_in=true
```

真实首签证据：`.scratch/checkin-http/reports/p01-authcode-token-new-device-20260825.json`。

客户端只允许在 `status.code=0 && checked_in=false` 时发送一次 claim；`claim.code=0` 仍必须用最终 `checked_in=true` 判定真实发放。请求中断时只重查 status，不自动重发 claim。探针不写回正式 `storage.json`，Token、Cookie、私钥不进入证据文件。

### 全账号重铸与客户端形态差异（2026-08-26）

对全部 6 个生产账号执行 AuthCode 重铸 + 当日真实发放（证据 `.scratch/checkin-http/reports/remint-*-20260826-*.json`），确立两个新协议事实：

1. **新设备首签的客户端形态门禁**：SOLO 形态（上表字段）的 AuthCode 新设备可立即首签；Work 形态（`ClientID: ono9krqynydwx5` / `PlatformCode: IDE_PC` / 空硬件字段）铸造成功（AuthCode/Token/账号匹配均通过）但 claim 被 9074 稳定拒绝。设备铸造与首签必须使用 SOLO 形态（App 代码入口：`OAuthClient::Solo`）。
2. **9074 频率维度**：同一 IP 短时间连续"铸造 + 首签"会被 9074 拒（与客户端形态无关）；冷却 3-10 分钟后以全新设备（新 DeviceID + 新 PKCE）重试可过，当日实测最多 3 次内全部成功。App 侧对应策略：重铸后冷却 3 分钟重试一次（`RemintCheckinRunner`），连续失败由用户手动重铸兜底。

### 重铸链路服务端变化复测（2026-09-02）

以未签到账号 LY 复测重铸链路（证据 `.scratch/checkin-http/reports/remint-probe-20260902-1250.md`），修正与新增四条协议事实：

1. **（修正当日早些时候的误判）ExchangeToken 无新增 WAF 头校验**：探针无浏览器头的生产实现 403 实为 20401 设备上限（当时未解析响应体误判为 WAF）。证据：同形态无浏览器头客户端当日 12:01 登录 + 自动重铸成功；12:53 带浏览器头 + 全新 AuthCode 的请求返回 HTTP 403 + 响应体业务码 20401。附带事实：403 失败尝试会消费 AuthCode，同码重试得业务码 10101（无效参数）——重试必须重新签发 AuthCode。
2. **9074 存在账号级稳定拒绝**：LY 当日 12:01 重新登录成功并自动重铸全新 SOLO 设备，12:42 与 14:06 以该新设备 claim 均被 9074 拒（重铸后 90 分钟以上仍拒绝）——9074 不全是短时频率问题，对被风控账号「冷却后重试」无效。
3. **设备配额上限 20401**：单账号服务端设备数有限额，超出后 ExchangeToken 返回 20401 "Device limit reached"（以 HTTP 403 + 响应体业务码形式返回）；反复重铸/多次登录会耗尽配额。**配额是服务端账号维度：本地删除账号档案不释放配额**——LY 删档后重新登录即在 ExchangeToken 处被拒（表现「登录凭证换取失败」）。App 的自动重铸重试策略必须收敛次数，禁止无限重铸。
4. **代码侧适配**：`parse_exchange_envelope` 已改为非 200 响应也解析响应体业务码（20401 等不再被吞成 Http(403)）；登录流新增 `LoginError::ExchangeDeviceLimit` 单独归类 20401，UI 提示设备上限的真实原因。
5. **（15:33 修正并推翻早前「账号级网关 403」结论）**：LY 重登的裸 `Http(403)` 实为 20401 设备上限——诊断日志捕获的完整响应体含 `"Code":"20401","Message":"Device limit reached."`。此前识别失败的根因：**服务端返回的业务码是 JSON 字符串形态**，解析只认数字导致 20401 被吞成裸 403。`exchange_error_code` 与 `GetPCAuthCode` 解析已改为数字/字符串双兼容。另：设备配额按 client_id 通道独立计数——LY 在 Work 通道（ono9krqynydwx5）配额耗尽，官方客户端（SOLO 通道）同账号可正常登录。
6. **（15:46 补充）`auth_from` 必须与登录通道一致**：官方 main.js 逆向实证 `auth_from = isSolo ? "solo" : "trae"`，SOLO 通道另追加 `hide_saas_login=true`。授权页按 `auth_from` 决定 AuthCode 的通道绑定：client_id 用 SOLO 而 auth_from 发 "trae" 时，ExchangeToken 报 20403 "Token device not match"（LY 实测，401 + 响应体业务码）。已修复 `build_login_url`。
7. **（15:40 决策）登录链路固定 SOLO 通道**：SOLO 与 Work 为同一产品改名（原 Trae SOLO → 现 Trae Work），服务端按 client_id 通道分别计设备配额。登录/换取/凭据存储统一切到 SOLO 通道（en1oxy7wnw8j9n）：a) Work 通道配额易被历史测试耗尽（20401）；b) SOLO 形态设备可直接首签，取消登录后自动重铸（每次登录设备注册 2 → 1）。后续通道可用性变化时按「能用的优先」原则切换，不绑定产品名。
8. **（16:05 LY 恢复实录）设备配额满 ≠ 账号不可用**：`checkin/retired/` 下的退役凭据包（DPAPI 加密，`{account_id}-{时间戳}.checkin`）含旧设备完整凭证。LY（3559551364241212）双通道配额满（20401）无法新登录，但其 8 月 26 日退役设备 2971060318912937 的 access token（至 09-05）与 refresh token（至 2027-02）均仍有效——GetUserInfo 实测 HTTP 200。恢复方法：凭据包复制到 `sha256(profile_id).checkin` 标准路径 + 重建 `accounts.json` 注册条目（`retired-devices.json` 提供 account_id↔profile_id 映射）。**refresh 模式续期用既有设备，不注册新设备，不触发 20401**——配额耗尽账号的标准救援路径。
9. **（16:07 修正第 2 条「账号级 9074」结论）9074 是设备信任门禁，非账号级封禁**：LY 恢复旧设备 2971060318912937 后 claim-only 探针实测 `business_0` 成功签到（status_before `[false,200]` → checked_in_after `true`）。此前当日 12:42/14:06 的 9074 拒绝全部发生在**新铸设备**上——有历史签到记录的老设备不受影响。结论：9074 拒「陌生设备」不拒「熟悉设备」，LY 账号本身在签到通道健康。
10. **（16:40 代码固化）9074/9095 触发后「恢复退役优先于重铸」已进主链路**：`RemintCheckinRunner` 决策树变更为 常规签到 → 9074/9095 → ①恢复退役设备（`DeviceRemintService::restore_retired`，检索 `retired/retired-devices.json` 未消耗条目，校验账号匹配 + refresh 未过期，凭据写回 + 注册表对齐 + 条目标记 consumed）→ 立即重试（老设备无频率冷却）；②无可用退役 → 原重铸路径。**恢复的老设备重试仍被拒时不再重铸**（每次运行至多换一次设备，防配额空烧）。恢复路径 0 设备注册、不触 20401；恢复失败的档案切换零副作用。

## 已验证当前账号证据

当前 `storage.json` 的 `iCubeAuthInfo://*` 值是 Base64 包装的 `tc` 二进制密文，不是参考账号管理器假设的明文 JSON，也不是可直接调用 `CryptUnprotectData` 解开的原始 DPAPI 数据块。

已从产品代码和多次真实启动会话确认：

```text
alog.log fetchLogTask.userId = 当前 iCube 用户信息 userId
renderer.log RouteService User info loaded.userId = 当前用户 ID
main.log updateUserInfo/getUserInfo.userId = 当前用户 ID
已验证启动会话至少两类来源一致
```

真实启动序列覆盖：

```text
account-A
account-B
account-C
```

`device-id-redacted` 已确认是 `deviceId`，不是用户账号 ID。`state.vscdb` 同时保留多个历史账号作用域键，不能单独判断当前账号。

V1 使用“权威日志 userId + 认证字段 SHA-256 指纹 + Local Storage 当前账号作用域”的证据组合。认证字段正文、Token 和 cookies 不进入 Trae Sync 数据库。证据缺失或冲突时停止写入，不从 `project.user_id` 猜测。

详细记录：原 `docs/ACCOUNT_DETECTION_FEASIBILITY.md`（2026-08-22 清理，git 历史可查；本节事实仍有效）。

## 已验证插件云端 API（2026-08-31，切号预同步）

TRAE 按账号云端插件列表调和本地安装（切号后云端为空 → 本地插件被卸载、市场显示为空）。以下协议为 solo-lite 前端路由表/构造器代码 + 真实日志 + 探针实测三方交叉验证；探针 `.scratch/history-u6/w0-probe/src/bin/plugin_sync_probe.rs` 真机同步 4 插件 4/4 成功并复核确认。

```text
GET  https://trae-api-cn.mchost.guru/api/remote/v1/plugins?page_size=200
     → 账号云端已装列表 {code:0, data:{items:[{marketplace_plugin_id, plugin_id, name, registry, version, ...}]}}
GET  https://api.trae.com.cn/extensions/api/-/plugin/detail?plugin_id=<市场ID>&registry=<registry>
     → 市场插件详情 {data:{plugin:{...}, download_url, checksum, file_size, manifest_json, connector_json, mcp_servers_json, skills_json, icon_url}}
POST https://trae-api-cn.mchost.guru/api/remote/v1/plugins
     → 安装到账号云端（code=0 成功；请求体与 solo-lite m() 构造器字段对齐 + bN() 产品参数）
```

鉴权头：`Authorization: Cloud-IDE-JWT <access_token>`、`Accept/Content-Type: application/json`、`X-Trae-Client-Type: lite`。列表/安装均按账号 Token 隔离；市场详情域名来自 boot config `marketApi`（实录 `api.trae.com.cn`），与 remote API 域名不同族。安装体必填 `uri`（= 详情 download_url）、`file_size`、`checksum`，故安装前必须先取详情；空串 JSON 字段（connector_json 等）按前端 `void 0` 语义省略。连续安装需串行间隔（实测 2s/项无触发限流）。无 `marketplace_plugin_id` 的用户自装插件无法跨账号同步。

生产实现：`infrastructure/plugin_cloud_sync.rs`（切号编排第 4.5 步，fail-soft）。

## 已验证插件市场目录与云端卸载 API（2026-08-31，api_gap_probe 实测）

**市场目录**（MarketplaceTransport 路由表，288.e82102fe.mjs @3030105）：`GET api.trae.com.cn/extensions/api/-/plugin/list`（Cloud-IDE-JWT 鉴权）→ 200，响应键为 `data.plugins` 数组（非 items），条目含 `plugin_id`/`name`/`display_name`/`description`/`i18n`。配套只读端点：`/plugin/recommended`、`/plugin/categories`、`/plugin/registries`。插件 tab「浏览 + 安装」能力成立。

**市场目录分页**（2026-09-02 marketpage 探针实测）：响应 `data` 含 `total`（155）与 `next_page_token`（游标，值如 `"50"`）；翻页参数名为 **`page_token`**（取上页 `next_page_token` 值），`page`/`page_num`/`cursor`/`offset` 均无效。单页 `page_size=50` 只返回首页 45 条；游标翻页或 `page_size>=200` 一次拉取可得全量 146 条（`total` 155 与可拉取 146 的差为服务端默认过滤，客户端同样不可见）。生产实现 `fetch_market_plugins` 已改为 `page_token` 游标循环翻页（去重 + 20 页安全上限）。

**云端卸载**：`DELETE trae-api-cn.mchost.guru/api/remote/v1/plugins/:plugin_id`（664.cf22b86e.mjs 路由表 @7594706；前端参数构造 `Je:()=>h`，h(e)={plugin_id:e}，h 即模块 45295）。实测关键事实：

- `:plugin_id` 必须用**列表项的不透明记录 ID**（如 `9.C3RY_-ZGFYN-`）——DELETE 后 code 0、列表实际缩短。
- 列表项 `plugin_id` 为 `builtin:<registry>:<name>` 形态（如 lark）的是**客户端内置插件**，云端无记录，DELETE/GET 单查均 404/992651——同步与卸载逻辑必须跳过 builtin 条目。
- `marketplace_plugin_id`（市场 UUID）不是卸载键（404）；`GET /api/remote/v1/plugins/:plugin_id` 单查路由对两种 ID 均 404（前端实际用 list+过滤，不走单查）。
- 往返验证：卸载（5→4）→ `sync_account_cloud_plugins` 重装（4→5）无损。「移除随行」能力成立。

## 已验证会话可见性开关与侧栏分组聚合（2026-08-31，归档功能）

TRAE 会话列表过滤只认 `chat_session.hidden_status` 的**原生枚举白名单**（`scheduled_task` / `voice_discussion`），自造值（实测 `traesync_archive`）写入存活、重启不被回写、但列表不隐藏——即排除式白名单，非空即隐藏不成立。会话归档借用 `voice_discussion` 值（安全论证：前端全部消费点为遥测上报与插件推荐过滤，无 UI 路由；库内原生同类会话存世一个月无异常视图）。恢复 = 还原为 NULL。其他候选字段均不可用：`deleted_at` 是删除语义（有清除风险）、`is_pinned` 是置顶、`extra`/`context` 为黑盒 JSON。

侧栏分组聚合（实测）：每个新「默认」会话都新建独立 work-mode 虚拟 project 行（不复用旧行）；「默认」分组是 UI 层按 work_mode 类型聚合的视图；恢复（hidden_status → NULL）的旧会话自动并入当前同类型分组，不产生重复分组。会话的 code/work 归属由 `chat_session.work_mode` 与 `project.work_mode` 共同决定，恢复时各回各模式。

## 权威过程文档

```text
<legacy-notes-root>\01-直接执行手册-账号历史转移.md
<legacy-notes-root>\02-解密逆向与迁移过程记录.md
<legacy-notes-root>\03-数据库结构与后续同步模块建议.md
```

后续实现必须以实际产品版本重新预检，不能假设所有 TRAE 产品共享相同路径、密钥或 schema。
