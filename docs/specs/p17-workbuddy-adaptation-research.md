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
