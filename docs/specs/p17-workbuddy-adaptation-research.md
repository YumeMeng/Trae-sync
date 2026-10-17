# 立项调研：WorkBuddy 工具适配（Phase 17 候选）

> 2026-10-17 立项调研。基于本机实测（WorkBuddy 5.8.2.0，E:\软件\workbuddy\）+ Trae Sync 现有架构盘点。**状态：2026-10-17 用户已界定范围（四大核心全要——账号/对话/签到/切换，确认 WorkBuddy 存在同类体系）；路径定案 = C→A（探针按 Trae Sync 扩展位开发，结论直接长成双产品扩展，不另起炉灶）；执行顺序 = 遗留优先（新号真机验收/OAuth 清理）→ 探针 → 扩展。**

## 1. 本机实测事实

| 维度 | Trae（Sync 已适配） | WorkBuddy（本机实测） |
|------|--------------------|---------------------|
| 技术栈 | Electron（VSCode fork 形态） | Electron + .NET 更新器（RepairApp/Newtonsoft.Json） |
| 数据根 | `%APPDATA%\Trae CN\`（官方） | `~/.workbuddy/app`（`--user-data-dir` 命令行显式指定） |
| 数据形态 | SQLCipher 加密 SQLite（61 表）+ storage.json 登录 blob（DPAPI 容器） | IndexedDB（`file__0.indexeddb.leveldb`）+ Local Storage leveldb + JSON 文件（app-config/expert-history/polling-lease） |
| 登录态 | storage.json 内 `iCubeAuthInfo://` 加密 blob（token/refresh/expires 齐全） | **本机未见登录态存储**（Cookies 空、leveldb 无 token 痕迹——账号体系待用户确认） |
| 用户身份 | TRAE 平台账号（user_id + DPAPI 容器 `fused-login`） | 未见平台账号；`polling-lease-1273223663.json` 文件名含 Windows SID 尾段（本机身份绑定线索） |
| 进程 | Trae CN.exe（映像名含 trae） | WorkBuddy.exe（映像名不含产品差异词——进程匹配规则需独立设计） |
| 协议层 | GetPCAuthCode/ExchangeToken/GetUserInfo/签到（已逆向实证） | **未知**（是否提供同类 API 完全未调研） |

## 2. 核心判断

**"功能点一致"是产品视角，不是实现视角。** 两者的数据/凭据/协议三层**没有可直接移植的代码**：

- **数据层不可移植**：SQLCipher+DPAPI（Trae）vs IndexedDB/LocalStorage（WorkBuddy）是两套完全不同的存储栈。
- **凭据层不可移植**：Trae 的登录 blob/容器铸造依赖 TraeCn 协议形态（client_id/设备身份/签名）——WorkBuddy 的账号体系未调研。
- **可复用的是架构骨架与工程纪律**：Tauri 壳与命令面、写操作闸门（Pending→Ready 验收制）、备份三件套治理（zstd/迁移/保留策略）、防逃逸判定（link_guard）、存储根收口（storage_roots）、单次确认纪律、探针驱动验收（隔离演练模式）、SOP 团队流程。**这些在 Phase 15/16 已被实机验证。**

## 3. 适配路径（三选一，待用户定）

- **路径 A：Trae Sync 扩展双产品支持**——在现有工具内引入产品描述符抽象（Phase 16 明确延迟的系统性抽象），WorkBuddy 作为第二产品接入。优点：一个工具管理两个 IDE；缺点：抽象层现在就要做对，且 WorkBuddy 数据层需从 IndexedDB/LocalStorage 读取（技术风险未知）。
- **路径 B：独立 WorkBuddy Sync 工具**——新仓库复用架构骨架（闸门/备份/纪律/流程模板），数据层按 WorkBuddy 实际重写。优点：零耦合风险、各自演进；缺点：两套工具的运维。
- **路径 C：先做能力探针调研**——不写产品代码，先出 WorkBuddy 的数据/协议探针（登录态定位、IndexedDB 结构解析、是否有云端 API），用探针结论再定 A/B。**与 P15-5 隔离演练先行是同一纪律。**

## 4. 用户决策记录（2026-10-17）

- **范围**：四大核心全部有真实需求（用户确认 WorkBuddy 存在同类账号/签到/多账号体系——"类似 agent 工具，使用方式类似"）。
- **账号体系**：用户不确定——**探针第一步骤即登录态定位**（登录页面观察 + 凭据存储解析，本机未见的登录态在探针中定位）。
- **路径**：C→A。用户强调"不必要重新开发，应复用并扩展"——探针按 Trae Sync 的扩展位开发（产品描述符位/数据层接口位），探针产物直接演进为双产品扩展实现。
- **顺序**：遗留优先。

## 4. 立项前置问题（grilling 第一轮，已全部回答）

1. **范围**：Trae Sync 四大核心（账号保存/对话主库/直连签到/账号切换）中，哪些在 WorkBuddy 上有真实对应需求？（WorkBuddy 是否存在签到体系？是否有多账号切换场景？）
2. **账号体系**：WorkBuddy 的登录用的是什么账号（TRAE 账号/腾讯账号/本机身份）？登录入口与凭证形态用户是否见过？
3. **对话数据**：WorkBuddy 的会话记录在云端还是本机 IndexedDB？用户想对它做什么（备份/导出/检索）？
4. **形态选择**：路径 A/B/C（第 3 节）。
5. **优先级**：与遗留债项（新号真机验收/OAuth 清理）的先后。

## 5. 已明确的低成本动作（无论选哪条路径）

- 进程匹配规则：`WorkBuddy.exe` 映像名 + `--user-data-dir` 参数解析（close_instance/launch 的 WorkBuddy 版）。
- 数据根常量：`~/.workbuddy/app`（storage_roots 模式新增 WorkBuddy 根组）。
- 备份治理复用：IndexedDB/LocalStorage 目录的压缩备份（zstd 对未加密数据有效——与 SQLCipher 不同，**这里压缩真有效**）。


## 6. 探针深挖补充（2026-10-17 asar 分析，P17-T1/T2 前置事实）

- **账号体系定论**：WorkBuddy 账号 = **CodeBuddy 账号**（account.js 内嵌 `www.codebuddy.cn/profile/plan` 套餐页链接 + `smh26tqjmz2qlj4i.api.tencentsmh.cn` 后端 API + `lightmake.site` 副域）。**"登录方式与 Trae 一样"的实质 = 同为网页 OAuth 授权码流（形态同构），账号源不同（CodeBuddy vs TRAE）**。
- **OAuth 客户端流完整**：`oauthClientId`（配置注入 e.oauth_client_id）、`oauthAppName`、`authorizedFlow/authorizedRecovery/authorizedCallbacks`——与 Trae Sync 的 OAuth 编排**模式同构，仅协议参数不同**。
- **凭据运行时**：`daemon.auth` 域（CLI 守护进程 401 观察链）——**凭据活动在 CodeBuddy daemon（CLI 后台进程）**；token 存储无 keytar/safeStorage 痕迹，渲染侧 localStorage + daemon 侧待 T1 正式解析。
- **本地服务**：`127.0.0.1:18488`（JSON API，`{"ok":false,"error":"Not Found"}` 形态）——路由表待 T2 从主代码解析。
- **内核**：`@genie` 命名空间（browser-runtime/extension-im-channels/workbuddy-browser-use）+ **内嵌 CodeBuddy CLI**（cli/bin/codebuddy + codebuddy-lite-wb.mjs）——WorkBuddy 的 agent 能力由 CodeBuddy CLI 驱动（与本项目同生态）。
- **对适配的直接影响**：Trae Sync 的捕获/铸造编排模式**可整体复用**，协议参数层（client_id/端点/设备绑定）按 CodeBuddy 协议重配——与"复用并扩展"的用户意图完全吻合。


## 7. T1/T2 探针执行结果（2026-10-17）

### T2 完成 ✅
- **IndexedDB/LocalStorage 定性**：本机为空壳/低使用状态（IndexedDB 仅 `__ai_page_offline_probe__` 键；LocalStorage 无 token 痕迹）——数据层探针结论：WorkBuddy 数据在**登录后产生**，正式解析需登录样本。
- **进程/路径适配点确认**：映像名 `WorkBuddy.exe`；数据根 `~/.workbuddy/app`（`--user-data-dir` 显式）；CodeBuddy CLI 工作目录 `~/.codebuddy/`（诊断+日志，凭据不在）。
- **18488 路由表**：`LocalProbeServer`（默认端口组 18488/18489/18490）——`GET /workbuddy/probe` 实测返回 `{"ok":true,"app":"workbuddy-desktop","version":"5.8.3","platform":"win32"}`（**WorkBuddy 本地探针 API 可用，与 Trae Sync 探针同构——集成入口已验证**）。
- **`/space/federation/v1`**：非本地服务——是 WorkBuddy **资料库联邦 URL 校验逻辑**（`AnonymousLibraryStaticRequest` + `normalizeEnterpriseLibraryOrigin`）——**WorkBuddy 存在"资料库/空间"产品概念与企业联邦端点协议**（对话/资料管理适配的对接线索）。
- **内核补充**：asar 全量提取完成（`.scratch/p17-probe/asar-full/`），main 区 154 文件（account.js 440KB 等），探查产物可复用。

### T1 部分完成（待登录样本）
- 登录态定位：本机 WorkBuddy 处于**未登录/低使用状态**（无 token 存储）——**精确定位需用户在 WorkBuddy 登录一次后重探**（localStorage/daemon 侧将出现可解析样本）。账号体系已定论（CodeBuddy 账号 + OAuth 授权码流，见第 6 节）。


### T1 定稿 ✅（2026-10-17 隔离环境解析，commit 9800cad 后续）

**隔离环境**：生产数据根完整复制到 `.scratch/p17-isolated/app`（52MB，零生产触碰）。

- **用户标识定论**：**平台用户 ID = 20 位数字字符串**（实测 `2079418751248068608`，腾讯平台 ID 风格；与 Trae 的 16 位 user_id 形态区分）。
- **标识存储**：Local Storage leveldb（`session/Local Storage/leveldb/`）键 `wb-slot-configs::{userUUID}:personal:zh:{tag}`，值为 **V8 序列化**（UTF-16-LE 交错，解析器已验证可提取）。
- **设备/实例标识**：`AEGIS_ID`（Local Storage，UUID a900ba02-...）+ `polling-lease-{SID尾段}.json`（ownerId UUID + pid + heartbeat——多实例租约机制）。
- **键名族**：`CODEBUDDY_DISABLE_AGENT_TEAMS`/`IDE_STORAGE_LANG`/`META:file://`（origin=file://）——配置与身份键族已摸清。
- **token 层**：Local Storage 未见 access token（在 daemon 侧或云端会话——捕获实现时随登录流程观察补定位；账号标识层已足够支撑适配设计）。
- **解析方法**：leveldb 值为 V8 序列化——UTF-16-LE 宽解码 + 交错剥离（python 片段见探针产物）。

## 8. T4c 凭据层定位定稿 ✅（2026-10-17 隔离环境解析 + 真机只读核对）

> 结论先行：**本机在 P17-T1 时就有活跃登录态**（T1 的「未登录」结论仅在 `app/` + `~/.codebuddy` 扫描范围内成立，未覆盖 `~/.workbuddy` 根与 `%LOCALAPPDATA%\CodeBuddyExtension`）；登录态/token/加密链路/刷新端点全部定位完成，**用户无需重新登录采样**。

### 8.1 存储位置（全部实测确认）

| 对象 | 路径 | 说明 |
|------|------|------|
| **主登录会话** | `%LOCALAPPDATA%\CodeBuddyExtension\Data\Public\auth\workbuddy-desktop.info` | JSON（7905B）；路径规则 `{sharedDataPath}/auth/{authentication.id}.info`，`authentication.id` 来自 ACC 产品配置（值 `workbuddy-desktop`）；sharedDataPath 由主进程 FilePathServiceImpl 推导（win32 base=`%LOCALAPPDATA%\CodeBuddyExtension`，`Data/Public` 固定段） |
| **密钥链** | `~/.workbuddy/keyblob` | JSON：`{version:1, keyId, slots:[{type:"static-v1", protectorKeyId, wrapped}]}`；wrapped 内层 `{suite:1, keyId, nonce, authTag, ciphertext}` |
| **fallback 键** | `~/.workbuddy-key-fallback/connector-keys/{keyId}.key` | 32 字节原始 AES-256 键（连接器用） |
| **每用户加密数据** | `~/.workbuddy/security/{uid}/data` | ACL 拒读（仅 SYSTEM/Administrators/守护进程身份）；持有者为沙箱中心命名管道——**直接文件捕获此路不通** |
| **账号快照** | `~/.workbuddy/storage/skeleton/account-snapshot.json` | `{primary:{uid,nickname,type,editionType,...}}` |
| **IPC 通道** | `~/.workbuddy/wbipc/endpoint.json` | `{endpoint:"\\\\.\\pipe\\wbipc-*", ticket}`——渲染层→daemon 的 RPC（凭据不过渲染层） |
| 渲染层 Local Storage | `~/.workbuddy/app/session/Local Storage/leveldb` | **实测无 token**（命中全为 UI 内容/遥测 session id） |

代码依据：`asar-full/main/file-authentication-storage.js`（FileAuthenticationStorage：store/restore/logout 标记/watcher 外部变更调和）、`app-lifetime-job.js`（at-rest 加密原语）、`workbuddy-paths.js`/`workbuddy-product-config.js`（configDir=`~/.workbuddy`）。

### 8.2 会话文件结构（隔离副本脱敏解析）

- `auth`：`accessToken`/`refreshToken` = **`{$wbEncrypted:1, envelope}` 加密信封**；`tokenType:"Bearer"`；`scope:"openid profile offline_access email"`（Keycloak 形态：sessionState/notBeforePolicy）；`domain:"www.workbuddy.cn"`；`expiresIn:2419200`（**access 28 天**）、`refreshExpiresIn:2592000`（**refresh 30 天**）；`lastRefreshTime/expiresAt/refreshExpiresAt` 毫秒时间戳。
- `account`/`accounts`/`allAccounts`：`uid:"89b16d6b-0549-4e1e-8ced-918b5c8152ae"`（梦梦，free）、`uin:"330126449009"`；`nickname`/`phoneNumber` 同为加密信封。
- 加密方案（app-lifetime-job.js 实读）：**AES-256-GCM（suite=1）**，12B nonce + 16B authTag，AAD=`buildAuthenticatedContextAad(keyId, suite, purpose, scheme)`（上下文绑定防挪用）；keyId 由键字节派生（16 hex）；keyblob static-v1 槽包裹数据键、protector 链同用户可解；失败类别含 `safe-storage`（Electron safeStorage=DPAPI 家族）。

### 8.3 刷新机制（daemon 日志实测）

- 端点：`https://www.workbuddy.cn/v2/plugin/auth/token/refresh`（daemon-server net-proxy 日志多次命中，DIRECT_TRUSTED no-proxy）。
- daemon 自动刷新（客户端运行期间周期调用；`lastRefreshTime` 与日志刷新调用吻合）；daemon RPC 通道实测存在 `auth:getCheckinStatus`/`auth:getAccountUsage` 等方法（wbipc）。
- 身份三形态并存（捕获实施时核对关联）：本地 uid UUID（89b16d6b…）、uin 12 位（330126449009）、P17-T1 的 20 位平台 ID（wb-slot-configs 内嵌）；设备级 `~/.workbuddy/device-id`（UUID）。

### 8.4 捕获编排凭据层设计（只设计，铸造实施待单次确认）

- **面 A（文件解密）**：读 `.info`（同用户可读，只读零破坏）→ 解析 `$wbEncrypted` 信封 → 按 keyblob/protector 链取 userKey（keyId 派生自键字节；AES-256-GCM open + AAD 上下文）→ 明文 token。解密链在实施时以真实数据验证。
- **面 B（RPC，优先探明）**：wbipc 命名管道 + ticket（endpoint.json）→ daemon `auth:*` RPC 表（若存在会话/token 读取方法则优先走 B，不触碰加密层）。实施时先探 RPC 表再定 A/B。
- **刷新**：工具侧只做只读健康检测；续期由 daemon 自理（与 Trae 融合版「客户端续期、工具只读」同构）；不写回会话文件。
- **入池铸造**（WorkBuddy 登录代次容器）：待用户单次确认后按 Trae 捕获编排同构实施。

### 8.5 备份范围缺口警示（T4b 后续建议）

T4b 真机备份覆盖 `~/.workbuddy/app`（Electron userData：IndexedDB/LocalStorage/JSON）。以下凭据材料**不在备份范围**：登录会话（CodeBuddyExtension auth）、`keyblob`、`device-id`、`login-device-nonce`、`account-snapshot.json`。建议 T4b 扩展一个「凭据材料备份」入口（加密信封形态落盘，属凭据材料、按单次确认纪律执行）——待用户确认后实施。
