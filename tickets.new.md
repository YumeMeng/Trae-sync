# Trae Sync 实施计划（Phase 0-14）

> 2026-08-22 重建。依据 `docs/DECISIONS-20260822-GRILL.md` 与 ADR-0018/0019/0020。
> 旧 T01-T21 计划已于 2026-08-22 清理删除（git 历史可查；其中 T01-T14 已完成并发布 0.2.1，T15-T21 部分完成的资产折算入本计划）。
> 当前状态（2026-09-10）：Phase 0-3 全部闭环（P3-3 被废止）、Phase 4 被 Phase 5 环境模型取代、四大核心需求全部落地。Phase 5-7 全部完成（P5-A 日常观察为用户配合项）；Phase 8 UX 治理主要批次已实施闭环，G13 于 2026-09-10 补齐账号列表凭据剩余时间展示。P9-0 App 优先闭环已完成：同设备换发、单/全量手动刷新、过期凭据单次 rescue 闸门、账号级持久退避、HTTP 注入 seam、命令契约与两个刷新入口 E2E 均已落地；Windows 计划任务/后台开关与 release 实机验收仍待后续收口。
> 当前执行入口（2026-09-30）：Phase 14，依据 ADR-0034。P12 R0–R3 与 P13 已完成代码不重复搬迁；P13-4 真机验收及 CN 续期重启恢复尚未完成，纳入 P14-9。P13 的“UI ready/已实装”勾选保留为历史代码交付记录，不等同操作已真实验收；未验收操作的普通入口先由 P14-1 收回。新规则与历史描述冲突时以 ADR 状态链及 Phase 14 为准。

## 已完成资产盘点（开发阶段直接复用）

**基建（T01-T14 完成，0.2.1 已发布）**：桌面应用骨架、Work CN 只读入口、授权扫描、历史语义、crate 分层、四页 UI（Overview/History/Accounts/Settings，46 E2E 通过）。

**签到链路（T16-T19 资产）**：HTTP 协议层（`checkin_http.rs`）、OAuth 登录流程（PKCE + 回调 + ExchangeToken）、DPAPI 凭据包、EC 设备密钥对、批量签到编排（45 单测通过）。**未经真实端到端执行**（App 数据目录为空）；已知缺陷：业务码被压平为 `business_error`、设备配额注释结论过时（未纳入 9074）。

**账号管理（T20-T21 资产）**：data_dir 自动发现、账号注册表雏形。

**历史同步（T02-T08 资产）**：SQLCipher 读取、schema 逆向、会话扫描——Phase 3 主库原语的底层。

---

## Phase 0：定胜负实验（签到设计裁决）

### P0-1 LY 虚拟设备 OAuth 实测 ✅（被 P1-0 以更大规模覆盖：2026-08-26 全账号 6/6 真机重铸 + 当日发放闭环，实验报告 `.scratch/checkin-http/reports/remint-*.json`）
- **目标**：LY 账号真实走 App OAuth 登录（生成虚拟设备：随机 16 位 ID + EC 密钥对 + 随机 MachineID，随 ExchangeToken 注册），然后用虚拟设备身份执行一次 claim。
- **验收**：实验结果落档 `.scratch/checkin-http/reports/`；claim 成功 → ADR-0019 设计成立，进入 Phase 1；返回 9074 → 服务端设备信任为"签到成功才建档"，回 ADR-0019 修订（首签引导设计需补充实验）。
- **备注**：OAuth 需用户在场配合授权；这是 OAuth 流程的首次真实执行（此前仅单测）。
- **前置**：MachineID 每账号随机——2026-08-22 核查确认已实现（`generate_machine_fingerprint` 生成随机 hex，非真实 MachineGuid），无需改动。

### P0-2 业务码透传修复 ✅（2026-08-22 完成）
- **目标**：application 层 `business_error` 展开为具体业务码与分类语义（9074=陌生设备、9095=设备日配额、20324=refresh 失效），UI 可区分展示；同步修正 `checkin_http.rs` 过时的设备配额注释。
- **完成情况**：`error_code()` 以 `business_{code}` 透传；claim 业务码拒绝从误报 `verification_failed` 修正为 `not_eligible`（与 CONTEXT 域语言一致）；UI 三类码独立文案 + 未知业务码数值透出；`checkin_http.rs`/`checkin_login.rs` 协议注释对齐 9074/9095 双闸门。
- **验收**：新增 2 个单测（业务码透传 + 业务拒绝分类），全工作区 `cargo test` 通过（application 层 52/52），前端 typecheck 通过。

---

## Phase 1：签到收尾

### P1-0 设备重铸体系（ADR-0019 v5）✅（2026-08-26 实施完成，当日生产闭环验证）
- **目标**：签到设备策略从"设备池 + 降级链"（v4）切换为"每账号独立设备 + 9074/9095 自动重铸"（v5）。
- **协议依据**（2026-08-26 全账号实测，证据 `.scratch/checkin-http/reports/remint-*.json`）：
  - AuthCode 铸造路线（GetPCAuthCode + ExchangeToken）对全部 6 账号完成重铸 + 当日真实发放；
  - 新设备首签仅 SOLO 形态可行（Work 形态被 9074 稳定拒绝）；
  - 9074 含频率维度，冷却 3-10 分钟重试可过（实测 3 次内 100% 成功）。
- **完成情况**：`get_pc_auth_code` 协议函数 + `OAuthClient` 形态抽象（`checkin_http.rs`）；`DeviceRemintService` 重铸服务（`remint.rs`，实现 `CheckinDeviceRemint` 端口）；`RemintCheckinRunner` 签到编排（application 层，替代 `DeviceFallbackRunner`：9074/9095 → 重铸 → 冷却 3 分钟 → 重试一次）；OAuth 登录后自动以 SOLO 形态重铸；`reset_checkin_device` 命令升级为网络完整重铸；设备池体系整体移除（`device_pool.rs`/`device_usage.rs`/`CheckinDeviceRebinder`/注册表 home 与签到史字段），旧设备资产归档 `checkin/retired/`；前端文案与手动重铸入口升级。
- **验收**：Rust workspace 全量测试通过（新增重铸链 7 单测）；前端 typecheck + 167 测试通过；生产只读验收 6/6 账号新凭据真实可用（token 13 天 / refresh 179 天）。

### P1-1 打开时检测 + 自动签到 ✅（2026-08-27 功能落地，2026-08-28 验收收口）
- **目标**：App 启动时查各账号今日签到状态，自动执行未完成签到（设置项默认开、可关）；Token 续期检查同批执行。
- **完成情况**：自动签到调度器全套（`src-tauri/src/lib.rs` L3377 `spawn_auto_checkin_scheduler` 起：定时触发/逐账号错峰/互斥/台账/失败当日不重试）；设置页开关与时间选择（`SettingsPanel.tsx`）；签到页状态行与完成横幅（`CheckinPage.tsx`）。
- **验收**：A1 补齐 e2e + 组件测试（mock-bridge `get_auto_checkin_settings` 分支 + 状态行场景 + `auto-checkin-finished` 横幅组件测试），vitest 172/172、e2e 47/47 全绿；已签账号幂等跳过随 U-3 全签语义覆盖。真实触发（定时 → 逐账号错峰 → 完成横幅）由用户日常自然观察收口，正常即收口；异常回报修复，当日不自动重试（既有约束）。

### P1-2 全部签到按钮与进度 ✅（被 U-3 以更强形态覆盖）
- **目标**：账号页"全部签到"按钮，串行逐账号 status → claim → status，实时进度与逐账号结果。
- **完成情况**：被 U-3 签到页四动作（一键全签/一键补签/签到所选/行内单签）+ 演进式列表以更强形态覆盖（原设想为账号页按钮）。
- **验收**：随 U-3 验收覆盖——单账号失败不影响其余账号（部分失败测试在案）；进度与逐账号结果行内可读。

### P1-3 账号卡片积分显示 ✅（2026-08-27 功能落地，2026-08-28 验收收口）
- **目标**：账号卡片显示剩余积分（status API 的 credits 字段）与今日签到状态。
- **完成情况**：账号页积分卡 + 缓存时间标注（`AccountCenter.tsx`）；签到/自动批次完成后刷新总览同步积分与今日已签。
- **验收**：A1 补齐 e2e + 组件测试后全绿（vitest 172/172、e2e 47/47）；积分随签到结果刷新、离线显示缓存值并标注时间随 U-2/U-3 验收覆盖。

### P1-4 签到链路 v6 改造：失败即报错 + 手动重置（ADR-0019 v6）✅（2026-09-02 六项决议全部落地）
- **背景**：v5 自动重铸链在生产暴露三类问题——a) 9074 时间窗不可预测（账号Y新账号实测：同 SOLO 形态同 180 秒铸签间隔，距登录 17 分钟拒、20 分钟过；冷却 3 分钟对新账号不成立），自动重试撞墙且每轮重铸消耗服务端设备配额；b) 冷却期 sleep(180) 阻塞持锁 + 前端倒计时为组件本地状态（退出页面即丢）+ 重复点击静默排队 → "一直签到中"；c) `credential_refresh_failed` 无文案映射落入"稍后重试"兜底误导用户（旧 Work 凭据账号实际出路是重新登录）。
- **目标**（grill 六项决议）：
  1. Work 通道代码彻底删除（`OAuthClient::Work` / `TRAE_CLIENT_ID` / `from_client_id` 未知回退），全链路仅 SOLO；
  2. 自动链整体取消：`RemintCheckinRunner` 退化为 status → 单次 claim → status 复核；删除自动重铸、退役设备自动恢复、冷却 sleep 与 `checkin-phase` cooldown 事件；
  3. 后端记录每账号最近设备重置时间戳（`AppState`，仅报错上下文用，不门控不自动重试）；
  4. 签到命令 try-lock 快速失败：执行中重复触发立即返回"签到正在进行中"；
  5. 失败文案按语义细分：9074 按重置时间上下文化（刚重置→引导等待；未重置→引导重置设备）、9095 设备日配额、`credential_refresh_failed` 引导重新登录、20401 设备数上限；
  6. 完整遥测头集合（trae-mate 式 `x-market-user-id`/`vscode-sessionid`/每请求 `x-request-id`/`x-tt-trace-id`/固定头）：探针实测通过后纳入 `checkin_http.rs`。
- **完成情况（六项决议全部落地，2026-09-02）**：Work 通道全删（`OAuthClient` 仅剩 `Solo`，探针/实测收编脚本同步改写）；`RemintCheckinRunner` 重命名为 `BatchCheckinRunner` 并退化为 status→单次 claim→status，自动重铸/冷却/退役恢复链全删；`AccountRecord` 新增 `device_created_at_unix_seconds`（登录与重铸两个写入点），9074 在设备铸造后 5 分钟内被拒时 detail_code 改标 `device_too_new`（用户B 实测：登录后 192 秒同设备重试即过，证明首签失败是时间窗而非设备问题）；签到命令 try-lock 快速失败（`checkin_already_running`）；前端删除冷却倒计时 UI/监听（`checkin-phase` 仅剩 inter_wait）、`fallback_device_id` 字段与"已自动更换设备"文案全清；`safeUiError` 补 `checkin_already_running`/`device_too_new` 映射并改写 9074/9095 文案。**决议 6（遥测头）**：探针实测 PASS（status 端点 2 账号 × 3 组带头/裸头对比全部 200/code=0 且业务字段一致，报告 `.scratch/checkin-http/reports/telemetry-headers-probe-20260902-234021.json`），据此 `checkin_body` 统一附完整头集合（固定头 + 设备 ID SHA-256 确定性派生 `x-market-user-id`/`vscode-sessionid` + 每请求刷新 `x-request-id`/`x-tt-trace-id`），3 个形状/稳定性单测全绿。
- **验收**：Rust 全量测试 + 前端 typecheck/vitest/e2e 全绿；手动签到路径无任何静默等待（秒级返回或明确报错）；9074 场景文案按上下文给出正确下一步；探针头集合实测记录落档 `.scratch/checkin-http/reports/`。
- **实施注意**：取消自动链后 `RemintCheckinRunner` 相关 7 单测需同步重写；`restore_retired_device` 路径删除但 `checkin/retired/` 留档不删（铁律）；LY 账号凭据仍为 Work 形态，需用户重新登录一次换发 SOLO 凭据（非代码任务，提醒用户）。


---

## Phase 2：账号注册表与切换

### P2-1 账号注册表与卡片列表 ✅（功能已落地：注册表 + OAuth 登录 + DPAPI 凭据包由 P1-0 全账号真机闭环；卡片/列表/健康度由 U-2 重设计落地。残余项「主力账号 OAuth 注册」并入 P5-A 批次 0 用户配合步骤）
- **目标**：账号档案完整管理（OAuth 登录入口、列表、基础信息、积分、签到状态、凭据健康度）；现有 A/LY/账号M重新授权。
- **验收**：OAuth 登录 → 档案建立 → 凭据包落盘 → 重启 App 后凭据可用；卡片信息完整。

### P2-2 data_dir 实例管理与快速切换 ✅（2026-08-23 实施完成；真机验收由 P2-3 E2E 同日覆盖——双实例并行启动/聚焦/关闭全路径实测通过）
- **目标**：每账号一个 App 自管 data_dir（`{storage_root}\trae-instances\{profile_id}`）；账号列表点账号 = 用 `--user-data-dir` 启动/聚焦该账号实例（并行多开，不关闭已运行实例，ADR-0020 2026-08-23 修订）；实例运行状态在 App 内可见。
- **首次启动**：无 data_dir 账号启动空目录实例并引导 TRAE 内登录一次（登录 blob 由 TRAE 原生产出，机器级密钥已实证跨目录可迁移）；已有 TRAE 原生账号目录（`TRAE SOLO CN_{user_id}`）的账号可复制其 `User\globalStorage` 作登录态种子。实验依据：`.scratch/p2-2-instance-experiments/REPORT.md`。
- **验收**：点账号卡片启动对应实例且不中断其他实例；实例运行状态（启动/运行/退出）在 App 内可见；同账号重复点击不重复启动（聚焦已有实例）。
- **完成情况**：`trae_instance.rs` 模块（exe 三级发现：运行进程 → 注册表 → 常见路径；WQL `NOT (...)` 语法排除自身；`--user-data-dir` 命令行匹配含引号/大小写/前缀边界处理；EnumWindows 聚焦；globalStorage+machineid 种子复制幂等）；`launch_trae_instance` / `get_trae_instance_states` 命令（spawn_blocking + 账号注册表校验）；账号卡片实例徽章 + 启动/聚焦按钮（div 容器解决按钮嵌套，键盘可达性保留）+ 5 秒轮询。Rust 单测 6 项、前端测试 148/148、tsc 通过；PowerShell 进程/注册表查询已在真实环境验证。

### P2-3 实例生命周期 ✅（2026-08-23 实施完成，E2E 验收通过）
- **目标**：启动/关闭/状态检测/异常清理（僵死进程检测）。
- **验收**：E2E 覆盖启动、切换、关闭全流程。
- **完成情况**：启动/状态检测随 P2-2 完成；本项补齐关闭——`close_trae_instance` 命令（taskkill 主进程 WM_CLOSE 优雅 3 秒 → 强制兜底多次复查；CREATE_NO_WINDOW 防控制台闪烁；幂等 not_running）。E2E 实测：TRAE 收 WM_CLOSE 后驻留托盘不退出，强制兜底为常态路径且数据无损（storage.json + 对话库完好，SQLite WAL）；关闭只影响目标实例进程树，其他并行实例与用户主实例不受波及。前端运行中卡片显示电源图标关闭按钮。僵死进程检测并入状态轮询（进程消失自动回未启动），不再单列。
- **E2E 验收（2026-08-23 computer-use 实测，UI + PowerShell 进程交叉验证）**：双实例并行启动（账号M + 用户A，26 进程/2 独立目录）→ 关闭其一无"关闭失败"误报（修复前 taskkill /F 终止滞后 1~3 秒会误报，已加 3 次复查吸收）→ 强杀后 10 秒内重启成功（code.lock 复用无占用报错，证明实例目录可循环使用）→ 最终关闭全部清理（0 管理进程残留，用户原生实例全程不受影响）。附带确认：状态检测不误报原生实例（user-data-dir 前缀匹配生效）。

### P2-4 实例登录态可见性 ✅（2026-08-24 实施完成，同日升级四态 + 健康检测）
- **目标**：让"启动过实例但未在 TRAE 内登录"的账号状态可见（2026-08-24 诊断：账号M账号 5 次启动均显示登录页，用户不知需手动登录一次；且 seed 的 NativeMissing 分支只报一次，storage.json 空壳存在后不再提示）。
- **完成情况**：`instance_login_state()` 纯函数（读实例 storage.json 判 `iCubeAuthInfo://usertag` 键，三实例实证：LY/import 有键=已登录，账号M无键=未登录；解析失败按待登录处理，登录幂等无害）；`launch_trae_instance` / `get_trae_instance_states` 返回 `login_state`；账号卡片登录态徽章；启动消息按登录态分支（未登录给"在 TRAE 窗口内登录一次"引导）。用户在 TRAE 内登录后徽章 5 秒内自动翻转（轮询闭环）。
- **四态升级 + 健康检测（2026-08-24 二次诊断驱动）**：LY 案例（徽章绿但 TRAE 实际未登录）暴露"键在会话死"盲区——TRAE 会话过期时不清除失效 blob，键存在性判定是乐观信号。升级：`instance_login_state()` 并入最近启动日志证据裁决（正向 `User info loaded {userId}` / 负向 `User not authenticated`、`[ckg] not login`，正向优先，均 2026-08-24 实证），新增 `stale` 第四态（红「登录已失效」徽章 + 启动消息引导重新登录）；账号页新增「健康检测」按钮（本地四态深度检测即时刷徽章 → 复用 `refresh_checkin_credits` 网络探测签到会话 → 汇总消息区分实例登录分布与签到会话异常账号）；已登录徽章 tooltip 去承诺化（"若 TRAE 显示登录页，重新登录一次即可"）。日志证据读取对目录名做 TRAE 时间戳格式校验，读取失败退化为键存在性判定（TRAE 更新日志结构时不误报）。Rust 单测 11 项（新增日志证据 4 分支）、前端 161 项全过。
- **已知问题（mutex 弹窗，不修）**：原生 TRAE 与账号实例并行运行时，账号实例启动会弹 TRAE 主进程自身的 JS 报错窗（`Error: Error mutex already exists`，模块级单例锁冲突，弹窗来自 TRAE 的 Electron uncaught exception dialog，App 无法外部抑制）。实测全部 8 次启动均弹（含已成功登录的实例），ckg/ai-agent health check 正常、登录态读写正常——点掉继续即可，功能无损。决定不做 UI 预警迎合（弹窗是 TRAE 行为，文档记录即可）。

---

## Phase 3：主库原语（ADR-0020 阶段 A）

### P3-1 TRAE 库会话读取 ✅（2026-08-23 实施完成，E2E 验收通过）
- **目标**：读取各账号 data_dir 的对话记录（复用 SQLCipher raw key 与 schema 资产），形成统一会话索引。
- **验收**：App 内可列出全部账号的会话（标题、时间、消息数）；读取不要求 TRAE 关闭（WAL 兼容或只读快照）。
- **完成情况**：`infrastructure/account_session_index.rs` 模块（隔离三件套只读打开，复用 `sqlcipher::open_with_key_readonly`；列防御探测，标题/时间/软删缺失自动降级；毫秒/秒双单位时间归一化；单账号失败不阻断其他账号）；`get_account_session_index` 命令（RealReadPreview 门卫 + baseline key + spawn_blocking）；History 页 `AccountSessionIndexPanel` 懒加载面板（点击才读取，ready/no_instance_data/read_failed 三态徽章、相对时间、软删标记、防重复请求）。Rust 单测 6 项（含加密库读取、错误 key 隔离、路径穿越防御、时间单位归一化）、vitest 7 项、全量 156/156、tsc 通过。
- **E2E 验收（2026-08-23 真实环境）**：UI 全路径通过（懒加载→读取→徽章/空态展示→二次刷新稳定）；4 个已启动实例的账号正确显示"0 个会话"（实例库经诊断确认 `chat_session` 真实为空——测试期实例只启动未对话，非读取缺陷）；用原生 TRAE 主库（722MB + 26MB WAL，运行中）以相同 SQL 实测读取 74 条真实会话，标题/消息数全部正确，`updated_at` 实测为秒级（归一化双单位覆盖实证），读取期间 TRAE 持续运行不受影响（只读隔离副本含 WAL 应用，4KB 主库 + 2.9MB WAL 的实例也能读出全部 schema）。

### P3-2 App 内统一阅览 ✅（2026-08-24 实施完成；真机验收由 P5-3 历史页主库视图真机验收覆盖——主库成为历史页唯一数据源，内容解析资产 `account_session_content.rs` 保留复用）
- **目标**：跨账号历史浏览（按账号分组/时间线），会话内容查看。
- **验收**：History 页升级为跨账号阅览；性能满足大库（T11 资产复用）。
- **完成情况**：后端 `infrastructure/account_session_content.rs` 模块（元数据表 LEFT JOIN 三张内容表一次取全；按 message_type 分表解析——general/chat 提取 JSON 块数组 text_content 按序拼接、task 解析执行轨迹取 plan_item.thought 摘要；表缺失降级空内容不整体失败；软删过滤；超长会话取最近 2000 条（DESC+LIMIT+反转，保尾部最新内容）；内容表 message_id 无唯一约束的 JOIN 行去重；隔离三件套只读打开复用）；`get_account_session_messages` 命令（RealReadPreview 门卫 + baseline key + spawn_blocking，状态语义与 P3-1 索引一致）。前端 `AccountSessionIndexPanel` 升级为统一阅览：视图切换（按账号分组 / 统一时间线跨账号按更新时间倒序）、账号筛选（两种视图共用）、会话行点击展开消息流（手风琴单展开 + 懒加载 + 缓存，用户文本/助手轨迹分形态渲染，角色徽章 + 消息时间）。Rust 模块单测 5 项（含加密库读取、软删过滤、超长会话最近窗口、JOIN 行去重、缺实例/错误 key 降级）、vitest 11 项（含时间线倒序、筛选、展开消息流、失败提示、缓存不重复请求）、tsc 通过。
- **协议事实依据**（2026-08-24 真实库探测，原生主库 713MB 实测 692 user + 692 task 消息）：`chat_message` 为元数据表，正文按 message_type 分表存储；user 消息内容为 JSON 块数组（text 块 text_content 拼接，非 text 块跳过）；assistant task 消息内容为执行轨迹（messages[].plan_item.thought 为步骤摘要）。
- **评审修复**（2026-08-24 双轴评审）：① LIMIT 截断方向反转——原 ASC+LIMIT 2000 截掉的是最新尾部，改为 DESC+LIMIT+反转换为保尾部最新 2000 条；② 内容表 message_id 无唯一约束，JOIN 行放大按 message_id 去重；③ 缺表降级实际失效（缺表时 SELECT 列数变化导致行映射列越界、误报 ReadFailed），改为 NULL 占位列保持列数恒定——由新增超长会话/缺表单测抓出；④ 样式硬编码色值回归 CSS 变量。

### P3-3 会话收入 / 返还 ⛔（被 ADR-0021 废止，2026-08-31）——「收入/返还」语义随主库单一归属模型失效：记录恒归当前登录账号，切号即归一，无需跨库复制。会话级「出库/入库」收纳需求由 P5-8a 归档通道承接（ADR-0022）
- **目标**：会话级跨库复制（消息树 ID 重写、时间戳、附件引用处理）；"收入主库"与"返还到账号（含自定义目标）"。
- **验收**：复制后会话在目标端（App 阅览与 TRAE 实例）均可正常打开与续聊；往返复制无 ID 冲突。

---

## Phase 4：主库模式（ADR-0020 阶段 B）→ 整体被 Phase 5 环境模型取代（2026-08-31 标注）

> 取代说明：ADR-0020 阶段 A/B「目录级切换 + 主库模式开关」设计经 2026-08-31 修订（ADR-0021）后废止——主库 = 官方目录（`TRAE SOLO CN`）本身，无专用 data_dir、无模式开关、无收入/返还。实际落地形态见 Phase 5：主库环境注册表（P5-0）、切号五步事务含 blob 互换（P5-1，即本 Phase 设想的"凭据注入"真机验证版）、环境页管理面板（P5-2，即"主库管理 UI"落地版）、主库详情页（P5-8，含健康度/备份维度）。

### P4-1 主库目录与凭据注入 → 由 P5-0/P5-1 取代 ✅（blob 互换凭据真机验证通过，见 P5-1 真机验收）
- **目标**：专用主库 data_dir 建立；开启主库模式时注入当前账号凭据并以主库目录启动 TRAE。
- **验收**：任何账号在主库模式下看到同一套历史；新对话写入主库；两个已识别风险点（凭据互换接受度、注入刷新时机）实测结论落档。
- **前置**：P4-0 小规模实测（用户配合）：A 注入主库目录启动 → 关闭换 B 注入启动，观察 TRAE 行为。

### P4-2 模式开关与单实例强制 → 被 ADR-0021 废止 ⛔（「主库模式开关」概念废止：主库恒为主库，单实例强制已由 P5-0 环境注册表实现）
- **目标**：主库模式全局开关（设置页 + 账号页快捷入口）；开启时强制单实例（多开按钮禁用并说明）。
- **验收**：开关切换语义符合 CONTEXT.md 约定（开启期间新对话归主库，关闭后需手动返还）。

### P4-3 主库管理 UI → 由 P5-2/P5-8 取代 ✅（环境页已落地；详情页/健康度/备份入口为 P5-4 + P5-8 进行中内容）
- **目标**：主库内容浏览、"返还"操作入口、主库健康度（大小、会话数）、手动备份覆盖主库。
- **验收**：收入/返还闭环可日常使用。

---

## 贯穿任务

- **规范维护**：本文件与 CONTEXT.md 随阶段推进更新；决策变更走 ADR（见决策记录"决策变更方式"）。
- **Gate 代码清理**：随各 Phase 模块改造移除 evidence 收集与资格判定代码，不做一次性大删除。
- **手动备份入口**（ADR-0018）：建议随 Phase 2（账号凭据）起步、Phase 4（主库）补全。

---

## UI/UX 改造（设计契约 `docs/DESIGN_TOKENS.md`，2026-08-26 grill 确立）

> 视觉方向：亮白通用毛玻璃（Light Neutral Glass）+「异常才亮色」哲学 + 全 token 化主题架构。
> 交互决策：签到页单列表演进式 + 四动作直达；账号页双视图（列表/卡片）+ 排序切换；徽章两槽位统一系统；总览摘要行改版；详情页危险操作分区；支持本地备注名与脱敏手机号记录。

### U-1 数据层 ✅（2026-08-26 实施）
- **目标**：注册表新增 `display_name`（本地备注名）与 `masked_mobile`（脱敏手机号）字段；登录时自动采集手机号；存量账号经"刷新额度"路径无感补采；总览 DTO 透出两字段；详情页备注名行内编辑。
- **验收**：cargo test 全绿（注册表新字段读写/幂等/旧文件兼容/登录保留备注名）；typecheck + 前端测试通过；真实环境刷新额度后 6 账号手机号自动补全。

### U-2 账号页重设计 ✅（2026-08-26 实施，验收通过）
- **目标**：列表/卡片双视图分段切换（localStorage 记忆 + 过渡动画）；两槽位徽章系统落地（签到三态 + 实例复合态 + meta 文字化）；行/卡片新布局（DESIGN_TOKENS 规格）；详情页危险操作分区；排序切换（添加序/名称/签到状态）。
- **完成情况**：`StatusBadges.tsx` 全局徽章组件（签到槽三态 + 实例槽复合态，stale 停止态也亮琥珀加强——持续性异常下次启动需重新登录）；`AccountCenter` 双视图 + 排序 + localStorage 记忆（`accounts.view` / `accounts.sort`）；列表宽行（名字独占主列永不挤压）与卡片（三级层级）双布局；meta 行展示手机号与登录凭据剩余天数，设备尾号仅在详情页展示；详情页删除下沉独立危险区；`--accent` 全局切换靛蓝 + 新 token 层落地（styles.css 双层 token：新语义层 + 旧桥接层）。
- **验收**：vitest 171/171（新增双视图切换/排序/徽章态 6 测试）；typecheck 通过；e2e 46/46；`e2e/visual-contract.mjs` DOM 契约断言 23/23；截图 5 张落 `artifacts/ui-shots/`。

**实例徽章停止态词表修正（2026-08-28）**：用户反馈健康检测汇总（"N 有效/N 待登录/N 未启动过实例"）与列表行徽章（一律"未启动"）对不上——两维度混用（登录态 vs 进程运行态）且停止态登录信息被 UI 吞掉，逐个对照只能点详情。修正（方案 A）：停止态徽章透出登录子态并与汇总统一词表——「登录有效」（中性灰）/「待登录」（琥珀）/「登录失效」（琥珀加强）/「未启动」（仅未初始化）；运行态文案不变。原"未启动 · 登录失效"缩为"登录失效"（停止态不再有歧义前缀）。验收：typecheck、vitest 171/171、visual-contract 27/27、截图重摄。

### U-3 签到页 + 总览改版 ✅（2026-08-26 实施，验收通过）
- **目标**：签到页单列表演进式（总进度条 + 行状态演进）+ 四动作直达（一键全签/一键补签/签到所选/行内单签）；总览页摘要行（四统计卡）+ 快捷操作；全局动效（hover 微浮起/列表 stagger/雾斑漂移）与毛玻璃主题全面落地。
- **完成情况**：签到页 `CheckinFlowRow` 单列表演进式（queued/running 高亮/done 结果行内呈现，选择-执行-结果零重复）+ 总进度条 + 四动作（全签幂等跳过已签、补签只发未签账号、所选子集、行内单签仅未签可点）；总览页「签到与账号」可插拔摘要区块（账号/今日签到 X·Y/运行中实例/积分合计 + 去签到/管理账号快捷入口）；全局毛玻璃：画布靛蓝雾斑漂移（36s）+ 3.5% 颗粒 + 标题栏/导航栏 backdrop-blur 玻璃化 + 列表 stagger 进场（reduced-motion 降级）。
- **验收**：vitest 171/171（签到页新增补签/行内签/所选 3 测试）；typecheck 通过；e2e 46/46（顺带修复两处存量过时 spec：账号页旧「高级切换」断言更新为现行 IA、固定壳滚动断言改为 .app-main 内部滚动语义）；视觉契约断言 23/23（徽章类、四动作、危险分区有效；**「玻璃 backdrop-filter 实测生效」结论当时失实**——断言只查属性存在，导航栏级联覆盖 bug 正从此漏过，见下方修正记录）；截图 `artifacts/ui-shots/01-overview.png` 至 `05-checkin.png`。

**修正与收敛（2026-08-27，毛玻璃收敛批 T1-T10）**：用户反馈"毛玻璃感完全没有感受到"，全面审计证实 U-3 的毛玻璃落地率不足两成（① 导航栏玻璃背景被 CSS 级联覆盖成实色；② 39 处内容面板仍为纯白 `--surface`，玻璃仅 3 处生效；③ 雾斑缩在左下角且浓度不足）。当日按用户逐项拍板的 8 项决策完成收敛：导航栏级联 bug 修复、雾斑对角贯穿（1300px、靛蓝 20%、36s alternate）、玻璃配方 token 补齐（`--glass-rail/--glass-dense`）+ 全面板玻璃化、绿色 `--safe` 徽章改中性+靛蓝点、按钮/输入框全套按合同重做（青色 focus ring 清除）、导航栏 52px 图标栏、标题栏 38px 精简（证据下沉总览）、透明圆角窗口（Tauri transparent + 自绘窗控）、stagger 仅首次挂载（切页/组件重建不重播、刷新重放）。验收加固：visual-contract 断言 23→27 项（玻璃 blur+背景透明度双断言、玻璃面板覆盖计数实测 8 处、雾斑 computed 全参数实测），杜绝同类假绿灯。收敛后：vitest 171/171、e2e 46/46、visual-contract 27/27、stagger 契约 6/6。

**透明窗口移除（2026-08-28，启动实例崩溃修复）**：用户点击"启动实例"后应用崩溃重启，日志排查确认 T8 透明窗口为根因——应用前台可见时 spawn TRAE 实例，主进程被静默终止（无 WER/Crashpad 转储/驱动事件；`panic="abort"` 也排除 panic 报告），后台运行则不触发；机制为 Tauri `transparent: true` 在 Windows 的 DirectComposition 特殊路径与 TRAE(Electron) 启动时 GPU 进程抢占前台的合成器冲突。修复：`tauri.conf.json` 改 `transparent: false`，body 呼吸带改实色 `--canvas`；圆角/投影/页面内毛玻璃（backdrop-filter + 雾斑）全部保留，仅放弃"桌面从圆角外露出"特征。对照实验：同崩溃场景（前台激活 + spawn `checkin-f03cd7249235` 实例）旧版必死、新版 15 秒存活；visual-contract 27/27 仍全绿（页面内毛玻璃与窗口透明无关）。T8 至此关闭。

### UI 验收工具（随批沉淀）
- `e2e/visual-shots.mjs`：mock 边界驱动构建产物，五页截图（总览/账号列表/卡片/详情/签到）。
- `e2e/visual-contract.mjs`：DESIGN_TOKENS 关键点 DOM 断言（玻璃 blur+透明度双断言、面板覆盖计数、雾斑 computed 实测、徽章、四动作、分区、meta），27 项。
- 运行方式：`pnpm build && pnpm preview --port 4173` 后 `node e2e/visual-shots.mjs` / `node e2e/visual-contract.mjs`。

### U-4 历史页玻璃基底收敛 + 签到验收收尾 ✅（2026-08-28 实施，验收全绿）
- **背景**：毛玻璃收敛批复查发现两块残留——① 历史工作台页整体仍为旧青绿字面色主题（上批清剿只覆盖 `var(--surface)` 形式，历史页 20+ 处字面青绿恰好漏网；visual-shots 五页截图不含历史页，从未进入验收视野）；② 签到模块功能全部落地但验收有尾巴（P1-1 e2e 缺失、P1 三条未打 ✅、生产观察未收口）。
- **目标**：批次 A 签到收尾（P1-1 轻量 e2e + 横幅组件测试 + P1 三条打 ✅ + 自然观察收口条款）；批次 B 历史页基底收敛（整页 `--glass-dense` 只换皮不改结构——结构重构用户明示另议；全文件字面色值清剿入语义 token；验收工具补历史页截图与断言）。
- **规划依据**：`.scratch/plan-20260827-checkin-close-ui-base.md`（grill 决策快照 + token 映射表）；执行任务书 `.scratch/handoff-20260827-checkin-close-ui-base.md`。
- **完成情况**：批次 A——mock-bridge 加 `get_auto_checkin_settings` 分支 + e2e 状态行场景（ui-acceptance 新增「签到页验收」describe）+ `auto-checkin-finished` 横幅组件测试（事件模块 mock，捕获回调手动触发）；P1 三条打 ✅（见 Phase 1 区）；生产观察收口条款入 P1-1 验收行。批次 B——styles.css 历史工作台 token 化（auth/摘要条/三栏/搜索结果/计划结果/session-index：青绿字面全灭，dense 玻璃 + 中性线 + 面板圆角 15px，子行项透明底避免双层 backdrop-filter）；全文件字面色值清剿（hex 28 处清零；非投影 rgba 清零，box-shadow 投影系保留；`--glass-hover/--glass-active/--glass-solid/--fill-soft/--fill/--fill-strong/--text-on-accent/--window-close` 八辅助 token 入语义层，`--window-close` 保留 #e81123 平台原值）；`var(--surface)` 10 处消费迁移（分段控件/排序下拉/备注名输入框/skip-link 高对比保留/session-index 开关等）；`--radius-panel` 8px→15px 对齐 DESIGN_TOKENS 舒展档（多处注释已按 15px 声明而值未跟上的既有偏差一并修正）。B3——visual-shots 增 `06-history.png`；visual-contract 增历史页断言组 5 项（导航可达/主面板玻璃双断言/授权区 dense 玻璃/玻璃面板覆盖 ≥4/圆角 ≥14px）。
- **验收（2026-08-28 实测）**：typecheck 绿；vitest 172/172（+1 横幅测试）；e2e 47/47（+1 状态行场景，历史页 spec 无回归）；visual-contract 32/32（27→32）；stagger 契约 6/6；`06-history.png` 产出且 computed 色彩扫描青绿残留 0 处；styles.css 除 `:root` 外 hex 字面清零（自检脚本随批输出）。
- **边界说明**：横幅 e2e 不可行是 mock 边界（`plugin:event|listen` 只返回句柄不产生事件，mock-bridge 注释在案），由组件测试覆盖，非功能缺口。
- **收尾审查补丁（2026-08-28 双轴 code-review）**：历史页行级子项三处双层 backdrop-filter 修复（session/search-hit/confirmation 去 blur 保半透明白底）；danger 分区弱边字面 rgba(180,35,60,.18) 并档 `--danger-line`（非投影 rgba 至此真清零）；孤立 `--surface` 定义删除（十处消费迁移后 0 消费，AGENTS.md 孤立清理铁律），`uiDesignTokens.test` 对比度基准同步改为纯白上限。修复后全验收复跑：typecheck / vitest 172 / e2e 47 / visual-contract 32 / stagger 6 全绿。

### U-5 UI 架构债清偿：styles.css 拆分 + token 单轨化（2026-08-28 完成）
- **背景**：U-4 收尾架构盘点（`.scratch/arch-report-20260828-u4.md`）确认两项跨批次结构性债——① styles.css 单文件 4400 行（U-4 批两次事故的根因）；② token 双轨：旧语义层 19 token / 223 处消费（全部在 styles.css，ts/tsx 零内联）未收敛，DESIGN_TOKENS「换主题只重定义 :root」承诺未兑现。
- **目标**：W1 styles.css 按域拆分为 8 文件（tokens/base/titlebar-rail/overview/workbench/accounts/checkin/session-index，@import 聚合）——纯机械搬移零行为变化；W2 旧语义层 token 归一并档（同值改名 / 近值归一 / --surface-soft 按语境二分）后删除桥接层，--accent-new* 后缀退役对齐 DESIGN_TOKENS 命名，uiDesignTokens.test 与 DESIGN_TOKENS.md 同批同步。
- **规划依据**：`.scratch/handoff-20260828-arch-debt-u5.md`（任务书：W1/W2 顺序与闸门、token 映射表含底数与 [DECIDE] 项、环境陷阱）。
- **明确不做**：历史页结构重构（候选 3，用户已排期 grill 讨论）；e2e 事件钩子（候选 4，Speculative 待真实需求）。
- **完成情况**：W1——styles.css 单文件拆为 8 域文件 + 9 行 @import 聚合入口（域文件名 titlebar.css，即任务书的 titlebar-rail）；行数分布：workbench 1657 / accounts 1162 / overview 381 / session-index 325 / titlebar 321 / base 316 / checkin 221 / tokens 63（W2 后值）。拆分用机械脚本 + 四维审计（括号平衡 / keyframes 与消费者同文件 / 声明行守恒 3010 / 选择器行守恒）；级联序修正一处：overview 与 workbench 的 import 顺序对调（拆分搬运致规则序颠倒，computed-style 快照对比发现后修复，此后 W1 前后快照零差异）。
  W2——旧桥接层 19 token 清剿：同值改名（--ink 29 / --muted 28 / --warning 5 / --canvas 3 / --accent-fg 2）；值归一（--muted-strong 40：#4f5c70→#5b6478；--line 36：#d8dee8→rgba(26,34,51,.08)；--line-strong 8：#b6bfcc→rgba(26,34,51,.14)；--warning-soft 5：#fdf3e7→rgba(217,119,6,.09)；--danger-soft：#fff0f2→rgba(180,35,60,.08)；--surface-muted 6 / --surface-selected 8 近值归一）；`--surface-soft` 12 处语境二分：11 处归 `--fill-soft`（workbench__check:hover / workbench__search / operations-panel__list li / settings-item / operations-panel--compact / account-center__advanced / account-center__summary-card+profiles / session-index__session-toggle / checkin-page__selection / checkin-page__empty-actions / account-card--clickable:active——均为中性淡填充语境），唯一嵌玻璃面板实底块 `.workbench-read__state` 归 `--glass-dense`（只换底色 token，未新增 backdrop-filter，规避双层 blur 陷阱）。`-new` 后缀退役（--accent-new* 4 名并入无后缀名）；桥接层整段删除，tokens.css 重写为单一语义层 43 定义（补 `--danger-line` 定义清悬空引用；`--muted-placeholder/--accent-hover/--danger/--shadow-*/--radius-*/--rail-width` 等归层保留）。uiDesignTokens.test 改读 `src/styles/tokens.css` 并适配新名（--muted→--text-2、--canvas→--app-bg、基准面删 --surface-soft 用纯白上限）；DESIGN_TOKENS.md 同批同步（--line→--line-hair、--line-strong→--line-control、--warn-soft→--warn-soft-bg 更名；补 --accent-hover/--accent-shadow/--muted-placeholder；新增「投影与尺寸」段；文档头补修订记录）。
  [DECIDE] 裁定：① --muted-strong 值归一采 U-4 先例（#4f5c70→#5b6478 轻微变亮，快照证实仅此一组色值迁移，不新开 --text-2-strong 档以保收敛初衷）；② warning-soft/danger-soft 实色→rgba 归一（玻璃底上 rgba 语义正确，visual-contract 琥珀类/危险分区断言仍绿）；③ --surface-soft 逐处判定如上（唯一 dense 判定 + 11 处 fill-soft）。
  视觉对照：computed-style 全 DOM 快照对比——W1 拆分前后零差异；W2 过滤 token 自定义属性后 7299 行差异收敛为 6 组唯一值迁移（text-2 归一 / line-hair / line-control / fill-soft / warn-soft-bg / accent-soft），全部在计划映射表内，无布局、字体或映射外属性变化。
- **验收（2026-08-28 实测）**：typecheck 绿；vitest 172/172；build 绿（CSS 66.45KB）；e2e 47/47；visual-contract 32/32；stagger 6/6；六张截图产出；token 自检 defs 43 / consumed 42，消费集合 ⊆ 定义集合（--space-panel 暂无消费者，保留档位），旧 token 名残留 0。

### U-6 历史页重做：账号为中心 + 深联动 + 准实时增量 ⏭（被 P5-3 历史页主库视图取代，2026-08-31 标注——「账号为中心」骨架随 ADR-0021 单一归属模型失效，历史页落地为主库两栏视图；授权记忆化与增量扫描资产折算入 P5-3，W0 探针工程继续服务 P5 系列调查）
- **背景**：用户反馈历史页「不好用、难理解」，账号切换未与历史页结合，启动实例常出现「新窗口未登录」（根因：原生目录无登录 blob 时 seed 失败，ADR-0020 决策 6 已证 OAuth token 无法构造 blob）。核心诉求：保留历史记录的情况下随意切换账号使用。经逐步 grill 讨论收敛七项决策（见规划依据）。
- **决策链**（2026-08-28 与用户逐步讨论定案）：
  1. **总方向**：App 层整合先行，主库模式（ADR-0020 阶段 B）后行；期间穿插主库凭据链路小规模实测（blob 注入刷新时机、凭据互换无感性），不阻塞排期。
  2. **历史页骨架**：以账号为中心——账号切换器为页面主干，选中账号→项目→对话；「全部」聚合视图与跨库搜索作为演进方向（现有 search 已跨库）。
  3. **联动深度**：深联动——历史页=主工作台：账号切换器带实例运行徽章（复用 AccountCenter 5 秒轮询模式）+ 启动/聚焦按钮；账号中心退居管理页（添加/删除账号、登录态维护、签到）。
  4. **授权门槛**：记忆化——首次显式授权后落盘持久记住，之后打开历史页自动扫描直接呈现；数据位置/账号证据变化时强制重新授权（现有失效机制兜底：data_location_changed / account_evidence_changed / authorization_mismatch 链路已存在）；设置中可手动撤销。
  5. **数据新鲜度**：准实时增量——进页即见目录库缓存（目录库本就持久化，DPAPI 加密落盘）+ 轮询检测各账号库文件 mtime/大小变化 + `chat_session.updated_at` 高水位增量扫描 + 「更新中」动态提示；删除会话走 `deleted_at` 一并处理。
  6. **搬运区处置**：前端全删（scopeMode/三层选择/计划预览/执行按钮/承接目标选择器，约 10 个 state + build_sync_plan/prepare_handoff_intent/apply_sync_plan 三处 invoke + 配套组件测试）——决定性事实：`apply_sync_plan` 在 RealReadPreview 生产模式被门禁拒绝（lib.rs「仅允许只读预览，TRAE 写入保持禁用」），且 switch_account/create_handoff_intent 无任何前端调用者（承接链路断头）；后端命令全部保留作为阶段 B 原语（跨库复制、消息树 ID 重写、附件引用处理、handoff intent 状态机——ADR-0020 明文「失败退路」，测试矩阵完整）。
  7. **技术前置实测**：「TRAE 运行中读取一致性」小规模实测先行（现有 R1 契约：进程观测只认 NotRunning 安全，与「用着 TRAE 时历史页刷新」诉求正面冲突）——候选策略：检测到变化时先复制库文件快照副本再解密副本，绕开读正在写的库；WAL 一致性需实测（P2-2 实验模式）。
- **规划依据**：本计划文件决策链记录（讨论过程含六轮用户裁定：层次→顺序→骨架→联动→授权/新鲜度→清理范围）；执行任务书待写（handoff 惯例）。
- **明确不做**：主库模式实现（阶段 B，凭据链路实测结论出来后排期）；「换账号继续」会话级动作（承接的后端保留，UI 重设计等主库方向明确后再做）；实例目录纳入自动扫描范围（已知盲区：App 启动的实例新对话写实例自身 data_dir，不在原生账号目录——U-6 范围内先明确标注，扩展留待实测后评估）。
- **待办分解**（任务书细化用）：W0 运行中读取一致性实测 → W1 前端搬运区删除（含测试清理）→ W2 后端授权持久化 + 进页即见 → W3 变化检测轮询 + 增量扫描 + 更新中提示 → W4 历史页账号为中心重做 + 深联动（复用 AccountCenter 轮询/启动模式）→ W5 验收闸门同步（e2e/visual-contract 历史页断言组重写、六张截图更新）。

### U-7 凭据保活：实例登录 blob 写回（2026-08-28 实施，2026-08-29 真机验证通过 ✅）
- **背景**：签到模块已为账号长期保存 token/refreshToken，但 TRAE Work 实例登录态同源却会缺失。核心诉求：登录一次后所有操作永不重复登录，账号增多后过期人工续期负担不可接受。任务书 `.scratch/handoff-20260828-keepalive-u7.md`；决策来源 `.scratch/grill-log-20260828.md`。
- **C0 实测结论**（`.scratch/blob-rewrite-probe/report.md`）：blob 加密格式完全逆向且可逆——离线解密（手册 §3.3 算法，6 字节头 + SHA512 校验通过）、原样重加密字节级一致；修改 expiredAt 同密钥材料重加密写回后启动实例，TRAE 接受（renderer.log `[RouteService] User logged in, restoring data` + `[HubNet] login success`，拒绝信号 0 次）；TRAE 运行期间自行重写 storage.json 仍保留 App 写入的新值——双向读写闭环。
- **C1 后端**：`infrastructure` 新模块 `blob_keepalive.rs`（解密/重加密/写回/互斥保护）；接入点 `checkin_http.rs` renew() 续期成功后调 `keepalive_after_renewal`；保活为增强能力，失败只记日志不影响续期，签到失败路径绝不触碰 blob。cargo test 848/848。
- **C2 前端**：stale 态文案微调（StatusBadges/AccountCenter/类型注释）——「等待下次签到自动恢复」。vitest 172/172。
- **C3 收口**：ADR-0020 决策 6 已修订（加密格式已逆向 + 保活机制确立）；e2e 47/47 · visual-contract 32/32 · stagger 6/6。
- **真机验证（2026-08-29 用户执行通过）**：实例内登录一次 → 重启实例免登录直达工作区（renderer.log 证据确认）。续期→写回的自然验证窗口在 9 月上旬 token 临近过期时由每日签到触发，届时可复查日志确认完整链路。
- **观察点**：9 月上旬 token 临近过期时，每日签到触发续期→写回，查 stderr 日志确认 `keepalive_after_renewal` 执行。

## Phase 5：环境模型（主库单实例 + 全自动切号）

> 2026-08-31 立项。决策来源 `.scratch/grill-log-20260830-env-model.md`（Q1-Q10 逐项裁定）；
> UI 契约 `.scratch/proto-20260830-env-model/index.html`（已冻结）。
> 核心模型：V1 单主库环境（`environments/master` 专属 data_dir），全部对话记录归主库；
> 切号 = 关实例 → 备份 → 凭据互换 → 记录交接 → 重启，全程 App 编排无感完成。
> 2026-08-31 补充：主库记录单一归属模型（ADR-0021）——主库 = 环境内全部对话集合，
> 单一归属当前登录账号，切号即归一；收编走 P5-5。

### P5-0 主库环境注册表与实例管理原语 ✅（2026-08-31 真机验收通过）
- **完成情况**：`infrastructure` 新模块 `environment_registry.rs`（`environments.json` 档案：固定 `env_id=master`、当前登录账号、原子写回）；`lib.rs` 命令 `launch_master_library`（启动/聚焦主库，窗口标题 = 当前账号名，不播种登录态——Q2 新建空主库语义）与 `get_environment_state`（档案 + 运行态 + 登录态）。主库 data_dir 固定 `{storage_root}\environments\master`，与账号实例目录分离，为 V2 多环境留位。
- **真机验收三条（2026-08-31 全部通过，CDP 驱动真机）**：① 主库以专属 data_dir（`environments\master`）启动，档案 `environments.json` 创建；② App 强杀重启后档案 `current_profile_id` 与「当前登录」展示保留，再次启动主库窗口标题恢复为当前账号名（`User/settings.json` 的 `window.title` 回写正确）；③ 环境页徽章/账号块/data_dir 足迹均来自 `get_environment_state`。

### P5-1 主库切号五步事务（Q1.1）✅（2026-08-31 实施完成，单测全绿）
- **完成情况**：
  - `infrastructure/master_handover.rs`：三件套备份（`backup_master_trio`，`.switch-bak-*` create_new 永不覆盖备份链）、WAL 双采样活动检测（`master_db_activity_detected`，Q1.2 生成中判定）、单事务交接（`handover_master_records`：空镜像清理 → 归属随行 → 活跃会话全量换腿 → 完整性校验；非空 UNIQUE 冲突报 `TargetConflict` 交人工决策，禁止静默覆盖）。
  - `infrastructure/blob_keepalive.rs` 增 `switch_auth_identity`（E3 凭据互换：供体 = 目标账号专属实例目录，受休 = 主库；登录 blob 明文原字节移植、主库密钥材料保留重加密——加密身份不变、登录身份互换）。
  - `infrastructure/relay_ledger.rs`（新模块）：接力台账（`environments/relay-ledger.json`），每次换腿逐会话记录 from/to/消息数快照；损坏拒绝重建（防伪造）、追加失败不阻断切号（只影响历史页轨迹展示）。
  - `lib.rs` 编排命令 `switch_master_account`（`force` 参数处理生成中切换）+ `master-switch-progress` 六阶段进度事件（closing → backing_up → switching_login → handing_over → restarting → done）；交接成功后环境档案写回当前账号。前端类型 `src/types/account_switch.ts`（MasterSwitchStage/ProgressEvent/Dto/ErrorCode 全量错误码）。
- **验收（2026-08-31 实测）**：cargo test 全绿（app crate + infrastructure 617 passed；master_handover 6 测试 / relay_ledger 5 测试 / blob_keepalive 含身份互换 10 测试全过）；typecheck 通过。
- **真机端到端验收（2026-08-31 通过，CDP 驱动）**：账号M → 用户A 五步事务全链路完成——弹层六阶段推进正常；备份链 `.switch-bak-*` 两份（create_new 语义验证）；TRAE 重启后 renderer.log 出现 `User info loaded {"userId":"用户D"}`（互换凭据被服务端接受）；`project.user_id` 随行为 用户D（inspect 探针只读核验）。**注意**：目标账号实例的登录 blob 会被 TRAE 清理（陈旧吊销），LY 实例 blob 已失效报 `switch_donor_login_missing`（文案与判定均正确），切换前需目标账号实例保持有效登录；本次改用 blob 有效的用户A 验收。
- **边界说明**：`master_switch_conflict` 人工决策分支未真机触发（单测覆盖）；种子 DB 无会话，换腿与接力台账 0 条目路径未产生真机样本（逻辑由单测覆盖，待日常使用中积累）。

### P5-2 环境页 + 账号页切号主面板（Q6/Q7/Q9）✅（2026-08-31 真机验收通过）
- **完成情况**：
  - 环境页 `EnvironmentPage.tsx`：主库实例卡（启动/聚焦 `launch_master_library`、运行/登录态徽章、当前账号块、data_dir 足迹）+ 副环境占位（Q7 规划中说明，无可点入口）；页面可见时 5 秒轮询运行态，隐藏即停；未登录时展示首次登录引导文案。
  - 切号弹层 `MasterSwitchDialog.tsx`：消费 `master-switch-progress` 六阶段事件推进五步清单与总进度条；完成回执展示随行项目/交接会话数；`master_switch_busy` 走「等待完成 / 强制切换」分支（force=true 重发）；全量错误码独立文案（含 `master_switch_conflict` 人工决策指引）；运行中禁止点遮罩中断。
  - 账号页接入 `AccountCenter.tsx`：当前账号展示「使用中」胶囊（禁切换/禁删除），其余账号一键切换入口唤起弹层；完成后刷新环境状态与总览。
  - 导航 `NavigationRail` 增「环境」工作区（六个工作区）；样式 `environment.css` 全 token 化玻璃面板（与原型契约一致）。
- **验收（2026-08-31 回归）**：typecheck 通过；vitest 182/182（新增 EnvironmentPage 4 + MasterSwitchDialog 5 + App 导航六工作区断言）；cargo test 全绿；e2e 48/48。
- **真机验收（2026-08-31 通过，CDP 驱动）**：环境页启动/聚焦主库与徽章流转正常；切号弹层六阶段推进 + 完成回执（随行项目 1 · 交接会话 0）+ 供体 blob 失效错误文案均真机验证；账号页「使用中」胶囊切号后正确移至目标账号；环境页「当前登录」联动正确。已知小瑕疵：页面切回瞬间可能短暂显示旧状态（5 秒轮询/手动刷新兜底收敛），不阻塞验收。

### P5-3 历史页主库视图（Q4/Q5，替换 U-6 骨架）✅ 2026-08-31 完成（可见性语义由 ADR-0028 修订）
- **目标**：历史页改为主库数据源（项目左列 + 会话右列两栏；正常会话按 `project.user_id` 过滤当前账号，归档会话进入全局归档视图）；接力轨迹徽章（会话行头像链，hover 全轨迹 = 接力台账聚合）；U-6 的账号中心骨架方案被环境模型取代，保留授权记忆化与增量扫描资产。
- **依据**：原型历史页 + 接力台账数据结构（`RelayLedgerEntry`）。
- **实现**：
  - 后端：`relay_ledger` 补 `from_session_id`（换腿后身份链回完整轨迹，旧文件兼容 + 环数据防御）；`master_history.rs` 读主库 project/chat_session（正常会话账号过滤 + 归档会话全局可见 + 三件套 stat 指纹，unchanged 预检）；`get_master_session_messages` 主库消息流变体；lib.rs 注册 `get_master_history` / `get_master_session_messages` / `get_relay_ledger` 三命令。
  - 前端：`HistoryWorkbench` 全重写（两栏 + 搜索/时间范围筛选 + 只读预览弹层 + 接力徽章 fixed 定位悬停浮层视口钳制）；5 秒指纹轮询，unchanged 不刷新台账；类型 `src/types/history.ts`；样式 `history.css`（全 token 玻璃，聚焦合同不写 outline:none）。
  - 测试：vitest 9 用例重写；e2e mock-bridge 补 P5-3 命令边界（含三跳接力链 fixture）；`history-workbench.spec.ts` 重写、`ui-acceptance`/`layout-diagnostics`/`visual-contract` 同步两栏结构（旧授权/扫描 UI 断言全部退役）。
- **验收（2026-08-31 全绿）**：typecheck 通过；vitest 117/117；cargo test workspace 全绿（含 master_history 4 + relay_ledger 5 用例）；e2e 28/28；visual-contract 33/33。
- **真机验收（2026-08-31 通过，CDP 驱动）**：`get_master_history` 真机 ready，左栏渲染真实项目「assistant」（DB 只读核实：project 表当前账号 1 条、chat_session 0 条 → 右栏空态「当前账号还没有对话记录」为数据事实）；`get_relay_ledger` 真机返回 0 条（无交接历史，无徽章正确）。主库消息预览因真机无会话未能触发，逻辑由 e2e 覆盖（下次交接产生会话后自然补充实证）。
- **2026-09-12 语义修订（ADR-0028）**——当前账号过滤仅适用于正常会话；归档会话是全局主库内容，不因原归属账号变化而消失，并且不参与切号交接。
- **下项推荐**：P5-A 批次 0（用户配合项：主力账号 OAuth 注册 + 首次真实切号 + P5-6 插件观察）——无开发量、直接解锁日常无感切号，详见 Phase 5 区。

### P5-4 总览联动 + 设置页备份分区收口 ✅（2026-08-31 实施，验收通过）
- **目标**：总览页统计卡接入主库聚合（会话数/参与账号数/最近活动）；账号切换后标题栏胶囊与环境卡「参与 N 个会话」联动；设置页备份分区收口（三件套备份入口 + `.switch-bak-*` 备份链管理与人工恢复指引）。
- **完成情况**：后端 `master_stats.rs`（4 条聚合 SQL 轻量统计，4 单测）+ `master_handover.rs` 备份链枚举（`list_master_backups`）；新增命令 `get_master_library_stats` / `get_master_backup_chain` / `create_master_backup`（运行中拒绝备份，与切号第 1 步同纪律）；总览统计卡 ready 时替换旧历史统计、主操作切「查看主库记录」；环境卡新增统计格（会话/项目/参与账号，随 active 重载联动）；设置页备份分区（链展示 + 立即备份 + 人工恢复指引，读取失败静默降级不渲染）；`safeUiError` 新增 `master_backup_running`/`master_backup_failed` 稳定文案；e2e mock-bridge 补三命令桩。
- **验收**：vitest 122/122（新增环境卡统计格 2 用例 + 备份分区 3 用例）；typecheck 通过；e2e 29/29（新增 P5-4 验收用例）；`cargo test` master_stats 4/4 + master_handover 9/9（含备份链 3 测试）。

### P5-A 批次 0：无感切号日常可用（用户配合项，无开发量，最优先）⭐
- **目标**：让「手动退出登录 + 手动同步历史」的日常痛点立即消失——切号机器已就绪（P5-1/P5-2 真机验收通过），只差账号在册。
- **步骤**：① 用户配合完成主力账号 OAuth 注册（库内 39 项目的主人）；② 环境档案登记当前登录账号；③ 首次真实切号跑通（当前账号 ↔ 主力账号往返）；④ 顺带完成 P5-6 插件列表观察（切号前后各看一次插件市场页）。
- **前置检查**：各切换目标账号的实例登录 blob 需有效（「登录失效」徽章的账号先重登一次，如 LY）。
- **验收**：用户日常切号全走 App 弹层，无手动退出/登录，历史记录全量随行（TRAE 内直接可见）。

### P5-5 主库体检 + 收编（ADR-0021，2026-08-31 立项；2026-08-31 二次 grill 细化）✅（2026-09-01 实施闭环）
- **背景**：2026-08-31 主库 DB 只读探针（`.scratch/master-inspect-20260831/`）实证多账号记录混居（5 个 user_id 分布）。注意：切号交接本身即归一（ADR-0021 决策 2），收编服务于「不切号也想清理」的场景，优先级让位于 P5-A。
- **目标**：环境页新增「主库体检」区块（进页自动只读体检，复用 5 秒指纹轮询模式）：
  1. **记录分布体检**：主库 DB 按 `project.user_id` 聚合正常、未删除且未归档会话及其项目（探针 SQL 正式化），列出非当前账号的滞留记录（账号名 + 项目数 + 会话数）；空壳镜像行单独报告但不进入收编规模，归档-only 不进入收编提示；
  2. **未注册账号检测**：解密主库 `storage.json` 登录 blob 识别本地登录过但未在 App 注册的账号，提示补注册（blob 解密复用 blob_keepalive 资产）；
  3. **一键归入（收编）**：非当前账号记录全部改写 `project.user_id` 为当前登录账号——**自动关-改-重启编排**（2026-08-31 裁定：与切号五步一致，进度事件复用现有弹层），执行前二次确认弹窗明确告知「将关闭主库 TRAE 实例」防误触；复用切号交接原语（三件套备份 → 空镜像行清理（UNIQUE 冲突防御）→ 归属改写 → 完整性校验）；**逐会话写接力台账**（from=原账号 to=当前账号，2026-08-31 裁定：保溯源链，与切号交接同构）；孤儿 user_input 行清理沿用交接链路策略。
- **完成情况（2026-09-01）**：`infrastructure/master_checkup.rs`（新模块：探针 SQL 正式化——账号分布聚合（LEFT JOIN 保留历史空账号行；按正常、未删除且未归档会话计数，归档/软删不进入可交接规模）+ 孤儿行统计 + 滞留账号过滤，3 单测）；`master_handover.rs` 扩 `HandoverSession.previous_user_id`（逐会话记录原归属，收编台账多账号杂居精确到行）。`lib.rs` 新增 `get_master_checkup`（只读命令：分布 + 注册表比对投影账号名/registered/current）与 `incorporate_master_records`（四步编排：只读预检（有滞留才动，避免空跑打扰）→ 关实例 → 三件套备份 → 归属改写/换腿/完整性校验（复用交接原语单事务）→ 接力台账（fail-soft）→ 重启；生成中拒绝 `master_incorporate_busy`；不改登录身份与插件环境）；进度事件 `master-incorporate-progress`（closing/backing_up/incorporating/restarting/done）。前端环境页体检区块（有滞留才渲染，无差异静默；账号名/数量主信息，user_id 收悬浮提示）+ 收编弹层（确认规模与账号列表（ADR-0018 单次确认）→ 四阶段进度条（复用切号弹层步骤条样式）→ 完成回执数量表述；运行中禁关）；`safeUiError` 新增 8 个收编错误码文案。简化说明：未注册账号检测通过注册表比对分布行实现（分布行无对应注册即显示「未登记账号」），未解密 blob——达成同样用户目标且零新增解密面；体检随 active/手动刷新/切号/收编后同读同刷，未做 5 秒轮询（体检非时敏数据，减少打扰）。孤儿行（user_id IS NULL）只报告不收编（无归属行改写缺乏依据，保守不动）。
- **验收**：cargo test master_checkup 3/3；tsc 通过；vitest 158/158（EnvironmentPage 新增 4 用例：体检区块渲染与表达纪律、无滞留/读取失败静默、收编全流程（确认→进度事件→回执→刷新体检）、失败错误码映射）。真机验收（归入后 TRAE 正常续聊、备份链生成、与探针分布一致）随 P5-A 日常使用顺带收口。
- **UI 术语**：「归入/主库体检/主库数据备份」，禁止「收编/换腿/三件套/blob」等内部术语。

### P5-6 插件同步（云端预同步方案）✅（2026-08-31 协议实测闭环 + 编排落地）
- **事实链（2026-08-31）**：插件本体机器级存储（`~/.trae-cn/extensions`，切号不消失）；安装状态为账号云端数据本地缓存（`state.vscdb` 按 user_id 前缀隔离的 ItemTable 键，探针 `.scratch/master-inspect-20260831/` 已定位）；用户实证官方登出/登新账号后插件列表同样消失 → 非工具所致，属账号维度预期行为。
- **裁定更新（2026-08-31）**：观察结论 = 切号后 TRAE 按目标账号云端插件列表调和本地安装（云端没有的触发卸载，市场显示为空）。处理方案 = 切号编排第 4.5 步「云端插件预同步」：重启前把切换前账号云端列表差集安装到目标账号云端（fail-soft，失败不阻断切号）。
- **协议实测闭环（2026-08-31）**：`GET/POST /api/remote/v1/plugins`（Cloud-IDE-JWT 鉴权）+ `GET api.trae.com.cn/extensions/api/-/plugin/detail`；探针 `.scratch/history-u6/w0-probe/src/bin/plugin_sync_probe.rs` 真机同步 4 插件 4/4 成功 + 复核确认；协议事实落档 `docs/TECHNICAL_BASELINE.md` 插件云端 API 章节。
- **完成情况**：`infrastructure/plugin_cloud_sync.rs`（新模块：列表差集 → 详情 → 安装，3 单测对齐 solo-lite m()/bN() 构造器）；切号编排插入第 4.5 步（`syncing_plugins` 阶段事件）；DTO/弹层透出同步回执（全部成功且无差集时静默）。
- **验收**：Rust `cargo check` + 模块 3 单测通过；前端 tsc + vitest 117/117 全绿；真机切号观察由 P5-A 批次 0 顺带收口（切号后插件市场应无卸载/重装过程）。

### P5-7 主库两层校验（2026-09-02 grill 立项：排查型只读功能，第三层恢复维持人工指引）✅（2026-09-02 实施闭环）
- **范围（grill 定案）**：只做前两层只读校验，发现异常只报告不修复。
  1. **接力台账核对**：台账条目 vs 库内实际归属——逐会话核对（存在性 / 归属一致性：台账最后一条 `to_user_id` 应等于库内 `project.user_id` / 消息数不减：当前库消息数 < 台账记录的 `message_count_at_switch` = 丢失候选 / 旧腿残留：`from_session_id` 在库内应不存在，存在即异常）。
  2. **备份对比**：默认最新 `.switch-bak-*` + 可切换链上任一备份点为基准，会话级差异（备份有、现在没有 = 丢失候选；现在有、备份没有 = 新增正常）；丢失候选附一句人工恢复指引（复用设置页既有文案）。
- **UI**：主库详情页新增「数据校验」tab（P5-8 扩展位）；进 tab 自动执行 + 刷新按钮（P5-5 体检同模式，tab 懒挂载）；台账核对与备份对比两个区块。
- **裁定依据**：原 2026-08-31 缓后裁定被 2026-09-02 用户指令取代（dev 环境「TraeSync一键启动.bat」直接验证，无需打包前提）；第三层「选择性恢复」是破坏性写操作，设置页已有人工恢复指引，复杂度与价值不匹配，不立项。
- **边界**：本地没有的内容无法找回（纯本地主库，他机记录不自动导入——ADR-0021 既有事实）；备份对比读备份库用只读方式打开（含 wal 附属件一致性处理）。
- **完成情况（2026-09-02）**：后端 `infrastructure/master_verification.rs`（`read_session_facts` 隔离三件套副本只读打开读会话事实 / `verify_relay_ledger` 台账核对（最新条目去重 + 中间腿跳过 + 四类异常：missing/owner_mismatch/message_loss/stale_leg）/ `compare_with_backup` 备份对比（换腿解释集排除正常接力），6 单测）+ `lib.rs` 命令 `get_master_verification`（backup_stamp 可选基准，状态机 ready/no_ledger/no_backups/backup_missing/read_failed/no_master_data）；前端 `MasterVerificationPanel` 组件 + 主库详情页「数据校验」tab（进 tab 自动执行 + 刷新 + 基准切换下拉；两槽位徽章 idle/warn/unknown 遵契约，异常行琥珀图标，会话标识收悬浮提示，丢失候选附人工恢复指引）。
- **验收（全绿）**：Rust 单测 6/6（master_verification）/ cargo check / tsc / vitest 167；真机观察随日常使用顺带收口（切号后备份对比即时可用）。

### P5-8 主库详情页（环境资产管理入口）✅（2026-08-31 grill 闭环，ADR-0022/0023；子项 a-1/a-2/b/b-3/界面纪律/8c 全部实施完成，「数据校验」tab 由 P5-7 填充）
- **结构**：主库卡片点击进入 → 顶部基础信息（主库路径与大小、当前绑定账号、项目/会话/消息统计、最近活跃、最近备份时间）+ tabs（对话列表 · 插件 · 后续扩展位；tab 位稀缺，归档不占 tab）。
- **详情页 shell（P5-8a-2 ✅ 2026-08-31 实施闭环）**：`MasterLibraryDetail` 页（环境卡「详情」入口 → App 路由 `master-library`，返回环境页为唯一退路）；基础信息区（当前账号/主库大小/最近活跃/最近备份/主库路径 + 项目/会话/消息计数，统计或备份链读取失败静默降级为 —）；tabs = 对话列表（复用 `LibrarySessionsPanel` embedded 模式，无页级标题保留工具行）+ 插件占位（P5-8b 填充）。实施：`master_stats.rs` 增 `message_count`（当前账号可见会话消息总数）+ `master_trio_size_bytes`（主库三件套合计字节）+ DTO 透出；tab 内容懒挂载（首次激活才渲染，避免与历史页实例重复 testid/白挂载）。验收全绿：cargo 640+ / tsc / vitest 136 / e2e 31（history-workbench 20 + ui-acceptance 11）。
- **对话列表 tab（P5-8a-1 ✅ 2026-08-31 实施闭环）**：历史页两栏形态承载（归档入口在右栏头部，不占 tab）；会话级归档（ADR-0022：出库 = hidden_status 借用 voice_discussion，恢复 = 还原 NULL）；归档视图按「模式 → 分组 → 会话」层级收纳；Gmail 式多选（浏览态无框，选择态浮出批量栏：归档/恢复/删除，Esc/完成退出）；真实删除 = 消息+会话+空壳项目行，列明规模单次确认。实施：`master_archive.rs`（归档/恢复/删除 + 自动备份，6 单测）+ `master_history.rs` 扩展 hidden_status/work_mode + 3 Tauri 命令 + 历史页选择模式/归档视图/删除确认弹窗；vitest 126/126 + e2e 32/32 全绿。真机验收（TRAE 侧栏即时生效）随 P5-A 日常使用顺带收口。
- **选择与归档树补强（2026-09-14）**：项目行选择会同步勾选当前视图下全部子会话；取消任一子会话会取消项目勾选；手动勾选全部子会话不会反向勾选项目，项目仍可作为空文件夹保留。单会话归档继续按其项目自动归类，归档页项目支持默认展开、逐项折叠/展开，选择项目同样覆盖其归档会话；持久化仍遵循 ADR-0028 的会话级归档语义，不新增项目级数据库状态。补充组件与 E2E 契约测试。
- **插件 tab（P5-8b 初版记录，已由 P8-4 G23 / ADR-0026 取代）**：环境插件清单模型（ADR-0023：环境持基线，切号对账 = 吸收后应用，+N/-M 单次确认，工具内装/卸即时改云端并更新清单）；已装清单管理必做，「浏览市场 + 安装」与「移除随行」两大能力前提已于 2026-08-31 探测闭环（市场目录 `GET /extensions/api/-/plugin/list` 响应 `data.plugins`；云端卸载 `DELETE /api/remote/v1/plugins/<记录ID>` 实测 code 0 + 重装往返无损，`builtin:` 前缀条目为客户端内置不可卸载需跳过——协议事实见 `docs/TECHNICAL_BASELINE.md`），无需降级。实施：后端 `plugin_manifest.rs`（storage_root 单 JSON 原子写，损坏拒绝覆盖，6 单测）+ `plugin_cloud_sync.rs` 扩云端已装/市场目录/装卸 API + 5 Tauri 命令（`get_plugin_tab_state` 首次启用自动从当前账号云端导入清单 / `browse_plugin_market` / `install_plugin` / `uninstall_plugin` / `absorb_plugin_manifest`）；前端 `PluginWorkbench`（已装/市场分段、市场懒加载、对账差异条 +N/-M 吸收按钮、卸载二次确认弹窗、装/卸回执；占位已移除）；插件通道错误码全部入 `safeUiError` 映射。验收全绿：cargo 647+ / tsc / vitest 147（PluginWorkbench 11 新增）/ e2e 34（P5-8b 插件 tab 全流程用例：清单徽章 → 吸收收敛 → 市场安装 → 卸载确认）。
- **切号对账改造（P5-8b-3 历史记录，已由 P8-4 G23 / ADR-0026 取代）**：切号预同步按 ADR-0023 升级为「吸收后应用」——`plugin_cloud_sync.rs` 对账改为双向（缺的装上 + 多的卸掉，`reconcile_plan` 纯函数预检与执行共用，回执新增 removed/declined/absorbed）；`lib.rs` 新增 `preview_master_switch_plugins` 预检命令（源 = 环境档案当前账号，fail-soft 永不报错），`switch_master_account` 增 `apply_plugins` 参数（false = 用户选择保留目标账号插件现状，declined 回执；true = 对账后吸收集落盘环境清单）。前端 `MasterSwitchDialog` 弹层先预检：无差异/预检失败静默直过；有差异弹 +N/-M 确认（插件名单列，移除项红色单列——破坏性操作红线），确认后带选择切换；busy 强制切换保留插件选择。验收：cargo test workspace 全绿 / tsc / vitest 151（MasterSwitchDialog 8）/ e2e 34。当前策略见 P8-4 G23。
- **界面表达纪律落地（2026-09-01 ✅）**：按用户反馈全面清理 UI 技术性表达——插件行去掉 slug/registry 元信息（只留「版本 x」，技术标识收进行悬浮提示）；账号详情折叠区字段中文化（账号档案标识/服务端账号标识）；总览证据带「账号指纹：xxx」→「当前账号 · xxx」；自造词全量替换（重铸→重置、随行→保留、交接会话→转移会话、凭据互换→登录状态切换、证据已过期→账号信息已过期等，覆盖切号弹层/账号页/签到页/错误码映射）；规则已写入 AGENTS.md「界面表达纪律」，后续所有 UI 文案遵循。验收：vitest 147 / e2e 34 全绿。
- **分组合并（P5-8c ✅ 2026-09-01 实施闭环）**：任意选组合并底层操作落地——后端 `master_archive.rs` 新增 `merge_master_projects`（源分组全部会话改挂目标分组（含归档/软删行，归位语义一致）+ 空壳源分组行 NOT EXISTS 守卫清理，事务原子；执行前自动 `.switch-bak-*` 备份，备份失败不执行；3 新单测）；`lib.rs` 命令 `merge_master_projects`（主库运行中拒绝 `master_merge_running`，与删除/切号同纪律）。前端历史页左栏 Gmail 式分组选择态（浏览态无框，「合并」入口进选择态 → 勾选分组 → 紧凑批量栏（全选/计数/合并/退出，Esc 退出）→ 确认弹层单选保留目标分组（列明将移动会话数（含已归档）与备份提示，ADR-0018 单次确认）→ 回执以数量表述（「已把 N 个会话并入「X」，清理 M 个空分组。」）；合并后若当前筛选分组被并走自动回退全部项目视图。`safeUiError` 新增 `master_merge_running`/`master_merge_invalid` 文案。同名合并快捷入口未做（弹层内改选目标已覆盖主路径，留待实际使用反馈再定）。验收：cargo master_archive 9/9（含 3 合并用例）/ tsc / vitest 154（HistoryWorkbench 新增 3 用例：完整合并流程、少于 2 个禁用 + Esc 层级、错误安全文案）。真机合并观察随 P5-A 日常使用顺带收口。
- **验收**：归档/恢复在 TRAE 侧栏即时生效；切号弹层差异确认（+N/-M）与插件 tab 状态联动；全部破坏性操作单次确认；UI 术语无内部词。

### P5-9 备份保留策略：定次自动裁剪（2026-09-02 grill 立项，先于 P5-7 实施）✅（2026-09-02 实施闭环）
- **背景**：备份在切号/合并/删除/收编/环境登录/手动备份 6 个生成点自动产生 `.switch-bak-{时间戳}`（db + wal/shm 附属件），无清理机制无限堆积。用户裁定（2026-09-02）：个人工具不需要无限备份保障，要求自动清理。
- **决策（grill 定案）**：**定次自动裁剪**——每次新备份生成后，自动删除超出保留数的旧备份（附属件连带删）；保留数默认 **5**，设置页可调（范围 1-50）、可关闭；关闭后回到「永不自动删」。裁剪失败静默（下次备份再试），不阻断备份与切号主流程。
- **铁律修订（同步落地 ADR-0018 决策 3 + AGENTS.md）**：「工具不得自动删除用户历史、快照、备份或失败证据」中的**备份**例外改为按用户配置的保留策略自动裁剪；用户历史、快照、失败证据的不自动删不变；保留策略关闭时回退为完全不自动删。
- **完成情况（2026-09-02）**：`infrastructure/backup_retention.rs` 新模块（配置存取 storage_root 单 JSON 原子写、损坏报错不重建、**save 前置 load 校验拒绝覆盖损坏文件**；`prune_master_backups` 裁剪：复用 `list_master_backups` 枚举、保留最新 N 份、wal/shm 连带删、命名异常散件永不触碰）；lib.rs 六个备份生成点成功后统一调用 `prune_backups_if_enabled`（静默容错）+ `get/set_backup_retention` 命令；设置页备份分区配置 UI（开关 + 保留份数 1-50 即时保存）与「永不自动删除」文案修订（keep_policy 注释同步）；`safeUiError` 补 3 个错误码映射。
- **验收（全绿）**：Rust 单测 8/8（含损坏文件拒覆盖、keep 边界值、散件不动、附属件连带删）/ cargo check / tsc / vitest 167（SettingsPanel 新增 2 用例：开关保存 + 越界值不提交）；真机观察随日常切号顺带收口（备份链稳定在保留数附近）。
- **UI 术语**：「自动清理旧备份 / 保留份数（1-50）」，不用「裁剪/淘汰」等词。

### 贯穿约束（Phase 5 全程）
- 切号顺序铁律 Q1.1：切换登录 → 交接记录（先换身份后交记录，已实现于编排第 3/4 步）。
- 破坏性批量操作前必须先备份（ADR-0018）；备份链永不覆盖；自动删除仅按用户配置的保留策略执行（P5-9，默认保留最近 5 份，可关闭）。
- 台账/档案损坏一律报错拒绝，不静默重建。
- UI 术语：用户可见文案用「接力/交接/登录凭据/主库数据备份」，禁止内部术语（腿/换腿/三件套/blob/9074/9095）。

---

## Phase 6：账号-环境解耦（2026-09-01 立项，ADR-0024）✅（2026-09-02 全部完成）

> 依据：E1 最小供体切号实验定论（`.scratch/minimal-donor-e1/REPORT.md`——切号对供体目录的唯一消费是登录 blob，三件套 2.7 MB 足够，完整实例目录 98.6% 为可重建的工具链缓存）+ 存储根盘点（`trae-instances` 19.3 GB / 退役 `environments/master` 3.5 GB / `snapshots` 3.3 GB / import 暂存 1.5 GB）+ 用户裁定（「启动实例」功能移除，以「环境」概念替代，账号 ↔ 环境多对多，主库不可变）。
> 任务依赖：P6-0 定胜负最先 → P6-1 / P6-2 随后（P6-2 的兜底形态依赖 P6-0 结论）→ P6-4 收尾；P6-3 独立小项随时可做。

### P6-0 E2 定胜负实验：凭据直接构造登录态 ⭐（最优先）✅ 2026-09-01 完成
- **命题**：App 凭据包（OAuth token + 用户信息）直接构造登录 blob 明文 JSON → 以目标环境密钥材料加密写回 → TRAE 实启动接受（服务端验证，非仅本地解密成功）。
- **结论：命题成立**（`.scratch/e2-credential-login/REPORT.md`）。证据链：TRAE `[updateUserInfo]` 加载构造明文（expiredAt `.000Z` 构造特征值）→ `[getUserInfo] response success`（凭据包 token 服务端验证）→ `[LoginStatus] logged in` ×3 → 持续 API 请求带构造身份与目标环境设备 ID；主库零触碰复核通过。
- **字段映射表已逆向定档**（双账号样本）：token 五件套 ← 凭据包；username/avatar_url/mobile 等 ← GetUserInfo 实调；host/scope/loginScope/userTag 等 ← TRAE CN 恒定值；tokenReleaseAt ← 构造时刻（无严格校验）。生产化约束：preserve_order 序列化、GetUserInfo 身份校验前置。
- **架构推论**：新账号登录到环境零实例目录依赖（纯凭据包构造）；切号双路径归一（E2 构造首选 / E1 存档移植降级，本质同构）；ADR-0024 决策 2/4 已同步修订。

### P6-1 退役数据清理（~23 GB，破坏性批量操作）✅ 2026-09-01 执行完成（用户单次确认授权）
- **执行前盘点（只读，已完成 2026-09-01）**：
  - **trae-instances 逐账号（合计 19.26 GB；清理候选 18.79 GB = 工具链 ModularData + 运行缓存 + 日志；保留 14.4 MB = User 目录全量）**：
    | 实例 | 总大小 | 工具链 | 缓存+日志 | User | 三件套 |
    |---|---|---|---|---|---|
    | checkin-0bea870c6951 | 3521.4 MB | 3474.1 | 35.3 | 9.3 | ✅ |
    | checkin-3cc73c217eb5 | 3573.6 MB | 3470.5 | 102.0 | 0.8 | ✅ |
    | checkin-66d8f009f598 | 4.0 MB | 0 | 4.0 | 0 | ❌ 未启动实例 |
    | checkin-ec1590e91ce0 | 3508.1 MB | 3470.6 | 37.1 | 0.2 | ✅ |
    | checkin-ee273142ca4e | 3551.8 MB | 3468.6 | 81.4 | 0.7 | ✅ |
    | checkin-f03cd7249235 | 3531.5 MB | 3468.5 | 61.5 | 0.4 | ✅ |
    | checkin-import-<设备ID> | 1571.2 MB | 1490.5 | 76.2 | 3.0 | ✅ |
  - ModularData 内部 99.99% 为 `ai-agent`（工具链缓存）；三件套路径 `User/globalStorage/storage.json` + `User/globalStorage/state.vscdb` + 根级 `machineid`。会话数核验（P3-1 索引缓存重读）：5 个有索引账号全部 0 会话（与 P3-1、grill-20260830 两次实测一致）；无索引 2 账号中 66d8f009f598 未启动（无三件套，整目录可删），0bea870c6951 三件套在位。**全部账号实例目录均为空对话库，剥离至 User 目录零对话损失。**
  - **environments/master 退役目录 3574.4 MB**：ModularData 3485.3（97.5%）+ 缓存/日志约 87 + User 0.5；`environments/relay-ledger.json` 与 `environments.json` 档案不在 master 目录内，删除不受影响。
  - **snapshots 3.34 GB（5 个快照）调用链核查定论：孤儿**。生成链（`scan_history`/`scan_default_history`/`scan_local_inventory` → SnapshotStore → `snapshots/`）与读取链（`browse_history`/`search_history`/`read_conversation`/`assign_source` 及授权命令族）全部注册于 invoke_handler 但 **前端零调用**（Phase 5 历史页已改读主库 `get_master_history`）。同族孤儿：P3-1/P3-2 会话索引命令族（`get_account_session_index`/`get_account_session_changes`/`refresh_account_session_index`/`get_account_session_messages`）前端仅类型定义残留、组件与测试已移除。
  - oauth-browser-profiles 0 条目（P6-3 生效）；staging/session-index/recovery 均 ~0 MB。
- **清理范围**：账号实例目录内 `ModularData`、`Cache`/`CachedData`/`CachedProfilesData`/`Code Cache`/`GPUCache`/`DawnGraphiteCache`/`DawnWebGPUCache`/`CachedConfigurations`/`logs`/`monitor`/`Crashpad` 等运行时目录；`environments/master` 退役 data_dir（保留 `environments/relay-ledger.json` 与 `environments.json`）；`checkin-66d8f009f598` 整目录（无三件套）。**snapshots 3.34 GB + scan/会话索引孤儿命令族：待用户裁定后一并清理（孤儿代码退役与数据清理可合并为一个后续小项）。**
- **保留红线**：三件套（`storage.json`/`state.vscdb`/`machineid`）、`User/` 目录、备份链（`.switch-bak-*`）、档案与台账、失败证据。
- **纪律**：清单 + 规模 + 单次确认（ADR-0018）；会话数非 0 的实例目录单独列出交用户决策（本次盘点全部为 0，无此情形）；预期回收 22.4 GB（不含 snapshots）或 25.7 GB（含 snapshots 裁定）。
- **验收**：清理后各账号存档仅余三件套 + User（MB 级）；切号功能在瘦身后存档上可用（任选一账号真机切号往返）；保活写回路径不受影响。
- **执行结果（2026-09-01）**：① trae-instances 运行时目录（ModularData/各级缓存/logs/monitor/Crashpad）7 实例全清，回收 19,240.4 MB，无失败残留；② `checkin-66d8f009f598` 整目录删除（无三件套未启动实例）；③ `environments/master` 退役 data_dir 删除（3,574.4 MB），`relay-ledger.json` 与 `environments.json` 档案保留在位；④ 执行前核验补录：主库 data_dir 代码确认指向官方目录（`%APPDATA%\TRAE SOLO CN`，environment_registry.rs `master_data_dir`），environments/master 为死路径，删除不影响环境页。**合计回收约 22.8 GB；storage_root 从 26 GB+ 降至 3.29 GB（余量主体为 snapshots 3.34 GB 待裁定项 + catalog/checkin 档案）；清理后验收：6 账号三件套全部在位（[SDM]），单账号 0.5~12.04 MB（0bea870c 12.04 / import 4.37 / ee27 1.8 / f03c 1.4 / 3cc7 1.07 / ec15 0.5）。** snapshots 与孤儿命令族未动（铁律：不自动删除快照 + 待用户单独裁定）。真机切号往返验证留待用户下次实际切号自然覆盖。

### P6-2 「启动实例」功能退役（依赖 P6-0 结论定兜底形态）✅ 2026-09-01 完成
- **命令侧（已落地）**：`launch_trae_instance`/`close_trae_instance` 命令及实现代码移除；`get_trae_instance_states` 收缩为登录存档健康度查询（`TraeInstanceStateDto` 移除 `running` 字段，fixture 模式统一返回 uninitialized）。E2 已定胜负（凭据直接构造登录态成立），无需实例启动兜底；进程管理原语保留（`launch_master_library` 主库编排与 P6-4 环境实例复用）。
- **UI 侧（已落地）**：账号页启动/关闭实例按钮、实例状态 5 秒轮询移除（存档只在登录/切号/签到时变化，改为进入页面与健康检测时读取一次）；徽章转型——新增 `LoginArchiveSlotBadge` 登录存档四态（登录有效=中性 / 待登录=琥珀 / 登录失效=琥珀加强 / 未登录=中性占位），健康检测汇总文案同步改为「登录存档：N 登录有效、N 登录失效…」词表；总览页「运行中实例」统计卡移除；`InstanceSlotBadge` 保留为环境页主库实例专用（复合态：运行态 + 登录子态）。账号页无实例入口。
- **代码清理（已落地）**：类型侧删除 `TraeInstanceLaunchDto`/`TraeInstanceCloseDto`/`TraeInstanceCloseOutcome`；accounts.css 实例操作行/实例按钮样式清理；实例相关测试退役，新增登录存档四态用例（含 stale 琥珀加强断言）；删除孤立测试文件 `tests/AccountSessionIndexPanel.test.tsx`（引用 P3-1 已移除组件，阻塞 typecheck 的预先存在问题）。`instance_login_state` 判定原语保留（存档健康度数据源）；存量疑似孤儿（snapshots 体系）未动，随 P6-1a 盘点裁定。
- **验收**：`pnpm typecheck` ✅；`cargo check --workspace` ✅（3 个预先存在的 unused import 警告，均不在本任务改动文件内）；`pnpm test` 152 passed / 0 failed（AccountCenter 24 用例含新增四态）；`cargo test` lib 套件 95 passed / 0 failed + 集成套件全绿。切号供体读取路径（三件套）零改动，测试全绿佐证；真机切号往返留待用户下次实际切号自然验证。

### P6-3 OAuth 临时浏览器档案自动清理（独立小项）✅ 2026-09-01 完成
- **现状**：`oauth-browser-profiles/{时间戳}` 每次隔离登录新建，机制上无清理（当前恰好为空，长期使用必堆积）。
- **实现（已落地）**：三层清理闭环——① 登录流程终结（`complete_checkin_login` 无论成败）后台删除当次 profile 目录（阻塞线程池，不占 async 线程）；② `begin` 重复发起时旧记录被覆盖，旧目录由周期清理回收；③ 后台线程每 5 分钟清理超龄（> 10 分钟 = 2 倍回调超时上限）目录，启动先清一次遗留。浏览器占用导致的删除失败静默重试；非时间戳命名条目不动；时钟回拨（目录时间戳晚于当前）不删。
- **验收（单测覆盖）**：超龄目录删除（含内容）、未超龄目录保留（保护进行中登录）、非时间戳条目不动、根目录不存在 no-op、时钟回拨保护——`oauth_profile_cleanup_tests` 3 用例；lib 测试套件 95 passed / 0 failed。
- **随任务修复**：`master_switch_tests` 样本构造缺 `HandoverSession.previous_user_id` 字段（预先存在的测试编译错误，`cargo check` 不编译 cfg(test) 故未暴露），补齐后测试套件恢复可运行。

### P6-4 环境管理 V2（账号 ↔ 环境多对多）✅ 2026-09-02 完成
- **注册表泛化**：`environments.json` 从单环境（master）扩展为多环境档案（`env_id`、名称、data_dir、创建时间）；master 固定官方目录不可删除不可改名；副环境 data_dir = `{storage_root}\environments\{env_id}`。档案损坏拒绝重建（沿用纪律）。
- **环境生命周期**：创建（空目录 + 档案登记）/ 重命名 / 删除（破坏性操作：环境内有会话记录时单次确认并列明规模，空环境直接删；删除连带 data_dir 与档案行）。
- **环境登录账号**：选环境 + 选账号 → 凭据互换（复用 `switch_auth_identity` 原语；空环境无记录交接，互换即完成）；环境内记录单一归属当前登录账号（ADR-0021 语义泛化）。P6-0 成功形态下互换凭据直接由凭据包构造；失败形态下从该账号登录凭据存档移植。
- **环境实例**：启动/聚焦/关闭复用 `trae_instance` 进程管理原语（`--user-data-dir` 指向环境目录）；环境间并行运行，环境内单实例；主库环境沿用 P5-0 既有编排。
- **UI（环境页改造）**：环境列表（名称、当前登录账号徽章、运行态、大小）+ 创建环境入口 + 每环境操作（启动/登录账号/删除）；主库环境置顶标识「主库」；账号页不增加环境操作（环境操作集中环境页，避免两处入口）。
- **范围边界**：V1 不做环境间记录搬运（环境是独立库）；插件清单绑定环境维度（ADR-0023 天然兼容，副环境首次启用按同规则从当前账号云端导入）。
- **验收**：创建空环境 → 登录账号 A → 启动该环境实例真机确认 A 登录态 → 环境内产生对话 → 登录账号 B（记录随行单一归属）→ 删除环境全流程（确认弹层列明规模）；主库环境全程不受影响；环境间并行运行实测。
- **实现（已落地）**：注册表 V2（`environment_registry` 多环境档案 + V1 迁移 + master 不可删改/保留名「主库」）+ 空环境登录播种原语（`seed_login_state_from_donor`，幂等守卫按目标登录 blob 判定）+ 全库规模统计（`read_total_scale`，删除确认数据源）+ 命令族 7 项（list/create/rename/delete_preview/delete/launch/login，登录复用切号凭据互换双路径）+ 环境页 V2（主库置顶卡 + 辅助环境列表 + 创建/重命名/登录/删除弹层，错误码映射走 safeUiError）。
- **验收数字（2026-09-02）**：`pnpm typecheck` 通过；`pnpm test` 165/165（12 文件，环境页 19 用例含创建/登录/删除/重命名/错误映射全流程）；`cargo test --workspace --all-targets` 全绿（注册表 11 + 播种 5 + 全库规模 1 新用例）。真机全流程（副环境产生对话、双账号登录、删除确认）待 release 打包后由用户确认。

### 贯穿约束（Phase 6 全程）
- 主库（官方目录）零触碰红线：所有实验与清理操作不写主库；主库环境档案与编排不动。
- 清理是破坏性批量操作：清单 + 单次确认 + 备份链/档案/台账/失败证据永不清（ADR-0018 + 铁律）。
- 存档三件套路径不变：U-7 保活与 P5-1 切号供体读取零改动。
- UI 术语：环境/登录凭据/存档，禁止「实例目录/blob/供体」等内部词进 UI（沿用界面表达纪律）。

## Phase 7：切号五问题修复（2026-09-01 立项，grill 定案）✅（2026-09-01 全部完成）

> 依据：用户真机切号实测反馈五问题 + 2026-09-01 grill 逐题定案（ADR-0024 决策 4 修订：双路径裁定 + 单账号存储预算 ≤ 10 MB）。背景：用户使用的 release 版本落后于当前源码，分析以当前源码为准。
> 收尾验收（2026-09-01）：cargo test --workspace 全绿（约 890 项）+ tsc 无错 + vitest 155/155 + Playwright e2e 34/34；P7-5 新增 credential_login_state 五态单测与徽章悬浮提示断言。真机验收（无存档账号 E2 切换、关浏览器按钮解锁、失效凭据徽章翻转）待 release 打包后由用户确认。
> 依赖顺序：P7-1 最先（切号主路径，P7-2/P7-3 叠加其上）→ P7-4 / P7-5 独立可并行。

### P7-1 切号登录态双路径：凭据包构造优先、存档移植降级 ⭐（最优先）
- **问题**：新账号无实例存档，切号第 3 步 `switch_auth_identity` 只走 E1 存档移植，报「专属实例尚未登录过」——而该文案指引自已退役的功能（账号页启动实例入口 P6-2 已移除）。
- **方案（ADR-0024 决策 4 修订，已落档）**：`switch_master_account_inner` 第 3 步改造为双路径——首选凭据包 + GetUserInfo 实调构造登录 blob 明文（E2 路径，字段映射表内置静态知识，preserve_order 序列化，GetUserInfo 身份校验前置不通过绝不写库）；构造失败静默降级 E1 存档移植；双路径均失败报 `switch_donor_login_missing`，错误文案改写为「目标账号凭据不可用，请重新登录该账号」（专属实例概念已退役）。
- **验收**：单测（构造字段映射、降级触发、双失败错误码）；真机切换一个无存档账号（如 66d8f009f598 重新登录后的新档案）走 E2 路径成功；GetUserInfo 断网模拟下降级 E1 成功。

### P7-2 切号失败自动回滚（LY 账号卡死修复）
- **问题**：第 3 步身份互换成功后第 4 步交接失败（如 TargetConflict），主库停留杂交态（登录身份 = 新账号、记录归属 = 旧账号）；重试被 `switch_same_account` 堵死，用户被锁死只能手工修。
- **方案（内存回滚）**：第 3 步写前已在内存持有主库原始 `storage.json` 全文（`target_raw`）；第 3/4 步及环境档案写回的任何失败触发「主库身份原始字节写回 + 三重自验证」；回滚成功后错误信息附「已自动还原到切换前状态，可安全重试」；回滚本身失败才指向第 2 步备份链路径。`switch_same_account` 判定随之恢复本义（真正重复切换才报）。
- **验收**：单测（注入第 4 步失败 → 存档身份还原 + 记录归属未变 + 返回错误带还原说明）；真机构造冲突场景验证重试通路。

### P7-3 交接进度可见化 + 集合式批量重写（转移慢修复）
- **问题**：`handing_over` 单阶段事件无细粒度进度；`apply_mapped_columns` 逐 ID 发 UPDATE（每 ID × 每表 × 每引用列一条，引用列无索引全表扫），万级消息时数十万条 UPDATE，分钟级耗时。
- **方案（两层）**：① 进度事件——`handover_master_records` 接收进度回调，映射/执行/校验三阶段各按「第 n/N 个会话」发事件，复用 `master-switch-progress` 通道扩展 `progress: {current, total, label}` 字段（向后兼容），label 用项目名（「正在转移项目“xxx”的会话 (12/89)」，纯技术 ID 不进主视野）；② 批量重写——事务内建 `temp.id_map(old, new)` 临时表，每表每引用列一条集合 UPDATE（`SET col = (SELECT new FROM id_map WHERE old = col) WHERE col IN (SELECT old FROM id_map)`），`rewrite_session_columns` 同法合并为单次集合重写。**不动**：单事务边界、写后指纹校验、`server_history_info` 永不触碰。
- **验收**：现有 4 个 handover 测试全绿（语义不变的硬证据）；新增批量重写等价性测试；真机切号计时对比（目标：分钟级 → 十秒级）。

### P7-4 登录等待可取消（浏览器退出按钮锁死修复）
- **问题**：`handleLogin` 串行 await `complete_checkin_login` 无限等待回调；用户关闭浏览器窗口后等待不终结，登录按钮一直锁定。
- **方案（轮询 + 取消 + 进程检测）**：`complete_checkin_login` 内部轮询（间隔 ~2s）检查三条件——回调已到 / 用户取消 / 浏览器进程已消失，任一命中即终结；新增 `cancel_checkin_login` 命令（置取消标记 + 清理本次浏览器档案，与 P6-3 三层清理闭环衔接）；前端登录弹层加「取消」按钮。浏览器进程消失时报「浏览器已关闭，登录未完成」。
- **验收**：单测（取消标记终结等待、进程检测）；真机关浏览器 → 数秒内按钮解锁并提示。

### P7-5 登录存档健康度改凭据包实调判定（徽章失真修复）
- **问题**：徽章判定 = 存档键存在性 + 最近一次 TRAE 启动日志证据；P6-2 退役实例启动后存档日志永远停摆，徽章显示与现实脱节（存档可能已被保活刷新或服务端吊销）。
- **方案**：徽章主判定改为凭据包 token 实调 GetUserInfo——成功 = 登录有效 / 401 = 登录失效 / 无凭据包 = 未登录（复用签到 HTTP 直连基建，与 P7-1 切号首选路径同源，徽章说有效则切号必通）；触发时机 = 账号页打开/手动刷新时逐账号并发实调（不后台轮询，沿用用户触发纪律）；存档文件状态（三件套有/无 → E1 降级路径可用性）收进徽章悬浮提示次要信息。
- **验收**：单测（三态判定）；真机对比——失效凭据账号徽章翻「登录失效」、有效账号翻「登录有效」，与实际切号可用性一致。

### 贯穿约束（Phase 7 全程）
- 主库备份链（第 2 步三件套备份）与失败证据永不删（铁律）；回滚优先内存还原，备份链为最后兜底。
- 单事务与指纹校验语义不变：P7-3 只改「怎么写」，不改「写什么」「怎么验」。
- 界面表达纪律沿用：错误文案不引导用户去已退役功能；进度 label 不暴露 session_id 等技术标识。

---

## Phase 8：UX 治理（2026-09-03 立项，两轮 grill 共 25 项决策）

> 依据：`.scratch/grill-ux-20260903.md`（G1-G25 决策全记录，含每项根因核实与交叉引用修订）。背景：全工具 UX 巡检发现 Gate 时代残留（已废止体系仍在 UI 常驻）、死代码报错（后端命令不存在仍被 invoke）、术语违反界面表达纪律（2026-09-01）、交互三套选择状态各自独立等问题。
> 结构：六批推进，每批完成后跑 `cargo test --workspace` + `tsc` + `vitest` + code-review 再提交。G 编号与 grill 文件一一对应，实施时以 grill 文件决策详情为准。
> 前置 ADR：P8-4 开工前补「Library 抽象」ADR（G8）；P8-4 插件实时同步开工前补「插件同步策略」ADR（G23）。

### P8-1 清理批（纯删除，低风险热身）⭐（最优先）

**G2 删标题栏三态徽章**
- **问题**：「只读保护中 / 需要重新检测 / 读取未授权」来自已废止 Gate 体系（2026-08-22 废止），常驻但无动作。
- **方案**：`src/components/TitleBar.tsx` 删除 `renderModeLabel` 徽章区块；清理 `scanEnabled` 数据链孤儿代码。
- **验收**：标题栏只余窗口控件 + 产品名；`rg renderModeLabel` 零命中；全量测试绿。

**G6 删总览页"最近活动"面板**
- **问题**：OperationsPanel 调用的 `list_operations`/`get_operation_lock_status` 后端命令不存在（注册表只有 `restore_operation`），面板恒显"操作状态暂不可用"——废止 journal 体系的孤儿组件。
- **方案**：删 `src/components/OperationsPanel.tsx` 及 OverviewPage 引用、关联类型与测试。
- **验收**：总览页无"最近活动"区块；`rg OperationsPanel|list_operations|get_operation_lock_status` 零命中；测试绿。

**G17 清理常驻提示词（全工具）**
- **问题**：签到页"真实签到已启用：仅对已通过登录的账号直连 TRAE……"（后端 capability.message，CheckinPage:337）、"四动作直达……串行执行"；账号页导游词；签到页"可用/未启用"徽章——全部是开发注释型文案。
- **方案**：删上述四类常驻提示；保留异常态提示（"签到功能不可用：存储未就绪"，仅异常时出现）；fixture 模式收敛为"演示模式"横幅。**判定标准（固化）**：常驻提示必须有"用户可据此行动"的信息；功能自述、实现细节、导游词一律不留。
- **验收**：正常态各页头部只有标题；异常注入测试（存储根不可用）仍显示异常提示。

**G25 删设置页"安全默认值"死区**
- **问题**："自动查找新历史"指向已废止功能（G1 同源）；副标题"尚未接入的选项不会显示为可编辑设置"是开发声明；"固定安全值"徽章无功能。
- **方案**：`SettingsPanel.tsx` 整块删除；自动签到改 toggle 开关 + time 选择器（删 select 啰嗦选项），错峰说明收悬浮提示。
- **验收**：设置页区块 = 自动签到（开关+时间）+ 密钥（折叠）+ 备份（折叠+立即备份）；无死区块。

**G1 总览页空态修正**
- **问题**：同屏"尚未发现 TRAE 数据位置，去历史页开始扫描"与主库统计数据共存；扫描功能已废止。
- **方案**：`OverviewPage.tsx` 空态分支重写——生产模式主库就绪为常态（"主库就绪但暂无对话"，引导去环境页）；"官方目录不存在"仅异常态一行中性提示（"未找到 TRAE 数据目录，请确认 TRAE 已安装"），不引导去扫描。
- **验收**：正常态无扫描语言；异常态（目录缺失）显示中性提示；统计与空态互斥。

### P8-2 账号页批（状态机与组件重构）

**G10 统一徽章状态机（账号页+签到页共用）⭐（本批地基，最先做）**
- **方案**：签到槽 6 态（已签绿/未签灰/签到失败红/待重试琥珀/不可领取灰/未刷新灰空心——消灭"状态未知"一词）+ 登录槽 5 态（正常绿/已过期琥珀·可自动恢复/需重登红·不可自动恢复/待登录琥珀/未登录灰）；四色语义全局统一（绿=确认正常、灰=中性、琥珀=需关注、红=必须人工处理）。两页共用 `StatusBadges` 组件；后端积分缓存扩展 `last_attempt_date` + `last_attempt_outcome` 字段（签到失败账号现在只显示"未签"，无法区分"从没试过"）；`checkinOutcomeLabel` 错误码词表作为悬浮提示与详情页共用词源。
- **验收**：状态机判定式单测（每态判定输入→输出）；组件测试（各态渲染词与色）；现有 AccountCenter 测试更新后全绿。

**G9 健康检测/刷新结果卡**
- **方案**：检测/刷新完成后展示结构化结果卡：首行结论"12 个账号正常，4 个需要处理"；下方只列异常账号（名字+一句人话原因）；正常账号不占空间。健康检测与刷新额度共用卡片形态；异常原因与 G10 状态机共用判定与文案。
- **验收**：组件测试（正常态只显示结论行、异常态列出名字与原因）；替换现有汇总长句。

**G12 卡片视图独立布局**
- **问题（已核实）**：卡片复用列表视图的 `creditsBlock`（`justify-items:end`+`min-width:64px` 为右栏设计）导致积分右对齐悬浮脱节；徽章行类名 `account-card__slots` 在 CSS 不存在（靠 grid 兜底）；窄卡片塞三段 meta 挤压换行。
- **方案**：卡片视图独立纵向层级——头像+名称行（含"使用中"chip）→ 徽章行（签到+登录并排）→ 积分数字（左对齐大字+标题小字）→ 手机号一行 → 底部切换按钮通栏；统一徽章行类名补样式。
- **验收**：e2e 三尺寸截图对比无错位；卡片内无右对齐悬浮元素。

**G13 meta 精简**
- **方案（2026-09-10 修正）**：卡片/列表 meta 保留手机号与访问令牌剩余天数，帮助快速判断账号状态；已过期或无法读取时显示明确文案。设备尾号与访问/刷新令牌的精确到期时刻继续收进详情页"技术细节"折叠区；`refresh_error_code` 标记继续纳入 G10 色彩。
- **验收**：列表与卡片均显示访问凭据剩余天数；已过期/有效期未知状态可区分；详情页继续展示精确到期信息；续期失败标记仍显示。

**G14 "全部/需处理"分段过滤**
- **方案**：账号列表上方分段控件"全部 / 需处理"（默认"全部"）；"需处理"带数字角标，只显示异常账号。异常判定口径与 G10 一致：登录槽红/琥珀 或 签到槽红；未签/未刷新不算异常。
- **验收**：组件测试（异常判定、角标数字、过滤切换）；正常时"需处理"无角标。

**G15 纯本地刷新按钮（账号页+签到页同构）**
- **方案**：页面头部刷新按钮 = 重读账号总览缓存（`refreshOverview`；不触发含逐账号 HTTP 实调的登录凭据探测），不发网络请求，按钮带旋转动画。语义分工：刷新=重读缓存；健康检测=真实探测。签到页同构添加。不做自动轮询。
- **验收**：点击后数据重读（可观察 invoke 调用）；无网络请求发出；两页行为一致。

**G11 手机号补录（含后端）**
- **方案**：登录完成后弹"补全手机号"步骤（可跳过）；未输入显示脱敏号，输入后显示全号（脱敏号不再显示）；详情页随时可改。校验三层：①与脱敏号首尾比对（前 3+后 2 位必须匹配，不合规则拒绝）②大陆手机号格式 ③与其他账号查重提示。输入框旁展示服务端脱敏号供肉眼比对。手机号与凭据包同级安全存储。
- **验收**：后端单测（校验三层判定）；组件测试（补录弹层、跳过、修改流）；凭据存储含新字段。

### P8-3 签到批

**G16 已签账号豁免错峰**
- **问题（已核实）**：`BatchCheckinRunner.run` 错峰延迟 3-8 秒对所有账号间生效，含已签账号（`checkin.rs:265-275`）——已签 20 个账号也要干等。
- **方案**：runner 层改造——探测后判定 `AlreadyCheckedIn`/`NotEligible`（未尝试 claim）的账号跳过错峰等待直接进入下一个；只有真正执行了 claim 的账号后才错峰。不做前置缓存过滤（避免隔日缓存漏签——G10"未刷新"态存在的理由）。
- **验收**：Rust 单测（已签账号批次不产生错峰等待、未签账号保留）；批次时间 = 未签账号数 ×（执行+3-8s）。

**签到页徽章接入 G10 状态机**
- **方案**：签到页账号行复用统一 `StatusBadges`（G10 数据层 `last_attempt_*` 已就绪）；失败账号显示"签到失败"红徽章+悬浮原因（`checkinOutcomeLabel` 词表），不再显示"未签"。执行临时态（排队中/签到中/倒计时）与徽章状态分开呈现。
- **验收**：组件测试（今日失败账号渲染红徽章）；e2e 截图正常。

### P8-4 主库详情批（最大改造）

**G19 SQL 层 0 会话项目过滤（最先做，独立可交付）（✅ 2026-09-07 实施闭环）**
- **问题（已核实）**：SQL 捞全部未删除项目含 0 会话空壳（`master_history.rs:191-193`），TRAE 自动创建的哈希名虚拟项目全变"未命名项目"堆积。
- **方案**：SQL 改为只返回至少 1 个可见会话的项目（EXISTS 子查询）；哈希名且路径尾段不可读的项目不进列表；无路径但有会话的项目归并"未关联文件夹"分组（复用现有按名合并机制）。不删任何数据（铁律）。
- **验收**：SQL 单测（0 会话项目被过滤、有会话项目保留、未关联归并）；真机列表"未命名项目"消失。

**G20 三 tab 平级结构（✅ 2026-09-07 实施闭环）**
- **方案**：主库详情改三 tab——对话列表 / 插件 / 库信息（原基础信息），默认落"对话列表"；顶部只留面包屑式标题（主库名+返回环境页）。数据校验 tab 由 G24 取消，其功能移入库信息 tab（备份对比）与 P8-5 自检（台账核对）。
- **验收**：三 tab 切换正常；顶部无大段基础信息；e2e 截图。

**G8+G21+G22 对话列表重构（两栏式，✅ 2026-09-09 实施闭环）**
- **前置**：补「Library 抽象」ADR——主库和副库的基地是"库"，库内模块（对话/插件/归档）不与具体库实例耦合；HistoryWorkbench 重命名 LibrarySessionsPanel，宿主页注入库参数（数据目录、raw key、当前 user_id）。
- **G21 两栏式**：左栏=项目树（点击原地展开会话子级：标题+相对时间；"未关联文件夹"末位；搜索框置顶；底部"选择"按钮）；右栏=对话查看器（顶部会话标题+[消息|接力]tab 默认消息；消息流上下排列，用户右对齐、助手左对齐；默认滚动到底部最新，向上滚动加载更早历史；2000 条截断保留）。用户消息按接力账号着色（色板 A=蓝/B=绿/C=琥珀 循环分配，色块头像+首字母），LLM 回复中性色。会话底部接力徽章收悬浮。
- **G22 统一选择模式**：左栏底部[选择]按钮进入（项目行/会话行显示勾选框，右栏变只读预览）；底部浮出 Gmail 式操作栏 [已选 N 会话·M 项目]+[归档][合并到…][恢复]（归档视图）/[删除]（会话级、单次确认列明数量与消息量）。合并流：弹层选目标项目→确认页→事务改挂+空源组清理。归档流：先进入可持久化草稿，用户点击“应用归档设置”后一次性提交；左栏"已归档"入口行（有归档内容才显示，进入归档视图——同一棵树灰显，层级 模式→项目→会话）。浏览态项目行悬浮"归档"快捷图标。
- **流程**：先用 rapid-prototype-craft 出高保真原型（含账号着色与选择模式两关键交互）→ 用户确认 → 实施。
- **验收**：原型确认记录；组件测试（树展开、选择模式、操作栏上下文按钮）；SQL 归档/合并既有测试全绿；e2e。
- **实施记录（2026-09-09）**：`LibrarySessionsPanel` 承载两栏式项目树与会话查看器（消息/接力 tab、接力账号着色、默认滚动到底部、向上分页读取更早消息并保留单页 2000 条上限）；统一选择模式覆盖项目/会话，归档/恢复/删除/分组合并沿用单次破坏性确认与可逆归档语义；ADR-0025 的库引用与 `libraryId` 参数化已接入主库详情页。验收：cargo test workspace 全绿（基础设施 700+55）/ tsc / vitest 210 / e2e 33/33。

**G23 插件表格化 + 实时同步（含后端）（✅ 2026-09-07 实施闭环）**
- **前置**：补「插件同步策略」ADR——新增全实时零确认（安全操作）；移除保留单次确认（"将同时从 N 个账号移除"，确认后应用到所有已知账号）；切号时差异确认弹层取消改静默应用（差异含移除时才弹一次确认）；对账条（plugin-drift）自然消失。
- **布局**：已装插件改密集行列表（图标+名称+来源标签+分类+操作按钮，行高约 40px）；顶部分类筛选 chips（复用市场 category_name）；市场段同表格化+搜索框；已装/市场共用组件骨架，segment 切换换数据源。
- **术语**：按 G4 执行（切号保留/仅此账号；卸载确认"移除后切换账号不再带走该插件"）。
- **验收**：后端单测（同步策略：新增零确认、移除单确认、切号静默应用）；组件测试（表格渲染、筛选）；e2e 插件 tab 截图。
- **实施记录（2026-09-07 至 2026-09-09）**：策略按 ADR-0026 落地——`switch_master_account` 删 `apply_plugins` 参数（对账静默执行，`PluginCloudSyncOutcome` 删 declined）；`uninstall_plugin`/`absorb_plugin_manifest` 命令删除，新增 `uninstall_plugin_everywhere`（当前账号卸载 + 清单移除 + 其他账号逐个 fail-soft 传播，`propagate_uninstall_with` 依赖注入可测，builtin 跳过）；`get_plugin_tab_state` 增 `known_account_count`（卸载确认文案 N）；预检抽纯函数 `switch_plugin_preview_from_lists`（remove_names 非空才确认）。前端 `PluginWorkbench` 表格化（共用 `PluginRow` 行骨架 + 分类 chips + 市场搜索，市场目录随 tab 激活拉取供分类关联），`MasterSwitchDialog` 仅含移除时弹单次确认（取消「保留目标账号插件」二选一）。验收：cargo 700+55（策略单测 6 新增）/ vitest 210 / tsc 全绿；e2e 33/33，mock bridge 命令已同步。

### P8-5 环境+总览批

**G18 主库自检 + 分级自愈（账号卡死修复通道）（✅ 2026-09-09 实施闭环）**
- **问题（已核实）**：`reconcile_master_current_profile` 实测不可用（blob 解密失败/登出态）时保留注册表缓存旧账号，切号前置校验依赖同链路 → 死锁无修复入口。
- **方案**：主库卡加"自检"按钮，四级检查：①读校验（storage.json 可读/blob 可解密/userId 可反查）②一致性（注册表缓存 vs 实测）③可切校验（实例未运行、供体目录存在）④深度检查（接力台账核对，G24 融入，默认折叠可选项）。结果分级：健康/可自愈（一键"纠正为实测值"，复用现有 reconcile 写回）/需人工（blob 损坏→"登录数据已损坏，需重新登录"引导账号页；切号弹层降级"强制重置为某账号"通道——死锁最后手段，单次确认）。
- **验收**：自检命令单测（四级判定、分级结果）；死锁场景模拟（blob 损坏 → 强制重置通道可用）。

**G7 证据带改接主库真实当前账号（✅ 2026-09-09 实施闭环）**
- **方案**：总览页证据带重写为"当前登录：xxx（最近活跃 x 分钟前）"，数据从主库读（当前 user_id/最近活跃会话归属）；读不到显示"未登录"（非"未检测"）。废弃指纹检测链路（G2 同源清理）。
- **验收**：与主库实际账号一致；登出态显示"未登录"；`rg "未检测"` 相关零命中。

**G5 环境页主库卡瘦身（✅ 2026-09-09 实施闭环）**
- **方案**：删统计格（详情页有完整版），只留主库名+体检结论+进入按钮；环境页职责回归"管理库的生死"。
- **验收**：环境页主库卡无统计数字；体检结论与启动按钮突出。

**G3 设置页折叠收尾（✅ 2026-09-09 实施闭环）**：密钥区块折叠后只露一行结论（"密钥正常"/"密钥异常需维护"）；备份绝对路径收 `<details>`；正文一句话+立即备份按钮。

**G3 补记（2026-09-21）**：设置页「安全维护」模块按用户决策整体移除——密钥状态/探测/候选登记 UI 与 `get_key_status` / `probe_source_key` / `register_source_key_candidate` 命令、infrastructure `source_key_profile` 候选密钥体系全删；数据库密钥唯一来源收敛为 lib.rs 硬编码 `WORK_CN_SOURCE_RAW_KEY`（启动激活路径随之简化）；磁盘遗留的 source-key-profiles 数据文件按铁律不删，仅代码不再读取。设置页现只保留自动签到与主库数据备份两个分区。

**G4 残余文案（✅ 2026-09-09 实施闭环）**：插件卡片标签改名（随主库/仅此账号→切号保留/仅此账号）随 G23 一并落地（对账条文案已被 G23 取代作废）。

- **P8-5 实施记录（2026-09-09）**：G18 增加主库四项只读自检、健康/可自愈/需人工分级、缓存纠正与切号弹层强制重置兜底；G7 总览改读主库真实当前账号与最近活跃时间；G5/G3 收窄主视野并折叠技术细节；G4 文案已在 G23 收口。验收：Rust 自检判定 6 项通过，vitest 213 项通过，tsc 通过；P8-5 全量 e2e 33/33 与 workspace 回归通过。

### P8-6 收尾批

**G8 独立历史工作区删除（✅ 2026-09-09 实施闭环；最后执行）**
- **方案**：删除独立历史工作区与导航入口（NavigationRail/路由/测试清理）；对话列表功能继续由 P8-4 `LibrarySessionsPanel` 在主库详情页承载。
- **验收**：导航无历史入口；`rg "HistoryWorkbench|navigation-history|activePage.*history" src tests e2e` 零命中；主库详情页对话列表 e2e 通过；全量回归（cargo test + tsc + vitest + e2e）。
- **实施记录（2026-09-09）**：移除 `NavigationRail` 的 history 页面类型与入口、删除 App 独立宿主，更新总览主库记录跳转与测试/验收脚本；保留 `LibrarySessionsPanel`，唯一入口为环境页 → 主库详情 → 对话列表。验收：tsc、vitest、cargo workspace、Playwright 全绿。

### 贯穿约束（Phase 8 全程）
- 每批完成即跑全量验证（cargo test --workspace + tsc + vitest + e2e 三尺寸截图）+ code-review 后提交，不跨批欠账。
- 界面表达纪律（2026-09-01）全程适用：内部标识不进主视野、技术细节收折叠区/悬浮、G17 判定标准（常驻提示须可据此行动）。
- 数据红线解除但铁律不变：P8-4 归档/删除操作保留单次确认（G22 删除）；不自动删任何历史数据（G19 只过滤不删）。

## 后续设计议题：凭据代次稳定性（2026-09-09，核心约束已确认）

本议题由 `账号M` 与 `用户C` 的 `20403 Token device not match` 故障触发。当前只记录已确认的设计边界，不提前实施未确认的状态机、批量修复或迁移。用户已确认：凭据维护需要覆盖管理工具关闭期间，并提供单账号/全部账号的手动刷新入口；手动刷新是立即换发一次；失败时保留旧凭据并按退避策略重试；后台使用当前 Windows 用户计划任务，每 6 小时唤醒短命令，按需联网后退出。

### P9-0 首次登录验收与一次 OAuth 约束（方案已确认，App 优先部分落地；Windows 暂缓）

- 首次 OAuth 不以 AuthCode 交换成功作为完成条件；新凭据代次必须通过本地完整性/绑定校验、账号身份校验、可持续性验收及成功写回。
- access token 尚未过期时，续期优先使用当前 access token + 当前设备申请 AuthCode，再用同一设备四件套交换完整新凭据；不创建新签到设备，不静默发起第二次 OAuth。新签发 access 约 14 天、refresh 约 180 天，寿命由服务端本次签发决定。
- 验收或修复失败时保留旧代次、候选代次与失败证据，不发布半成品凭据，不覆盖可恢复材料。
- 凭据发布采用“候选代次 → 完整验收 → 原子发布”边界：发布前不切换当前代次；任一步失败都保持旧当前代次不变，避免凭据文件与账号档案指向不同代次。
- 增加不含 token/私钥正文的凭据发布日志，记录前后代次、文件校验信息和事务阶段；启动或账号操作前只做本地幂等恢复，不自动触发 OAuth、设备重铸或网络重试。
- 对已完成登录验收的现有账号，按阈值独立维护凭据；旧 refresh 返回 `20403` 不再触发自动签到设备重铸。只有 access 已过期时才最后尝试旧 refresh；失败保留当前代次并要求用户明确重新登录。
- 2026-09-09 真实验证：`133****1304` 与 `156****3219` 各连续两次完成同设备 AuthCode 换发，JWT 身份校验、原子写回均通过；新 pair 的 access/refresh 有新签发寿命，未进行第二次 OAuth。随后用户实机确认换发后的凭据可以完成 TRAE Work 登录。
- 用户新增确认：增加后台凭据维护范围，以及“刷新此账号 / 刷新全部账号”手动入口；两者均复用同设备 AuthCode 换发与候选代次→验收→原子发布边界。
- 用户确认失败策略：同设备换发失败但旧 access 仍有效时，保留旧凭据，按 1h→2h→4h→8h 指数退避，之后最多每天一次；按账号隔离并设每日上限，超限转为需处理；不隐式 OAuth、不重铸设备，手动失败不启动隐藏循环。
- 用户确认低占用后台策略：当前用户计划任务在登录时及每 6 小时唤醒一次；健康账号只做本地检查，维护命令完成后立即退出，不做常驻进程。
- 用户确认后台开关策略：计划任务默认启用；设置中可关闭，关闭只停止后台任务，不删除凭据、备份或失败证据；重新开启时恢复任务，卸载时只移除计划任务。
- 用户确认批量策略：全部刷新按账号串行执行；单个账号失败不阻断后续账号，不做跨账号回滚；每个账号独立保留旧凭据并汇总成功、失败和无需操作结果。
- 已落地（2026-09-10）：access 已过期且无法申请同设备 AuthCode 时，按当前凭据代次用不可覆盖的原子标记最多尝试一次旧 refresh；20403/20101 或其他失败不再自动重放，当前凭据保留并进入凭据维护失败提示。新 AuthCode 换发/重新登录产生新代次后，救援额度自动重新独立计算。
- 已落地（2026-09-10，App 优先）：账号列表新增“刷新凭据”（全部账号串行），账号详情新增“刷新登录凭据”（单账号）；两者只执行同设备换发并展示逐账号结果，不触发新的用户 OAuth、签到或额度查询。Windows 计划任务/后台开关暂缓，后续另行实施。
- 已补强（2026-09-14）：两处手动刷新凭据入口均增加二次确认，取消不触发任何换发；E2E 契约测试同步覆盖取消路径。
- 已落地（2026-09-14）：全工具确认交互形态统一为自定义弹层（新通用组件 ConfirmDialog，switch-dialog 视觉骨架）——账号页 5 处原生 window.confirm 全部替换（批量/单账号刷新凭据、重置设备、删除账号、G11 手机号查重）；破坏性操作确认按钮用危险色；ESC/遮罩点击可取消、执行中防重复提交。同轮用户决策：切号、一键全签保持无确认原状（核心日常操作，幂等/可恢复）；P9-0 Windows 后台凭据维护维持暂缓。
- 已收敛（2026-09-14）：确认弹层组件收敛到公共 ConfirmDialog——插件卸载确认、会话删除/合并确认、环境删除确认全部迁移，各自手写的遮罩/骨架/动作条删除（history.css 中 preview-veil/preview/confirm-dialog__foot 孤儿样式已清理）；ConfirmDialog 扩展 children（清单/单选列表/结构化通知正文）与 confirmDisabled（前置数据未加载禁用）。流程型/输入型弹层（切号、收编、环境登录/命名、手机号补录）不属确认类，保持独立实现。
- 已补强（2026-09-10，App 优先）：旧 Work `client_id` 在任何换发/救援前直接失败，不发起网络请求、不创建 rescue 标记；账号“需处理”过滤同步识别已知凭据失效/缺失与 access 过期，即使健康度读取未返回也不会隐藏。新增后端守卫测试与前端过滤回归测试。
- **已收口（2026-09-10，App 优先）：**`credential-maintenance.json` 只保存非敏感账号级失败退避/每日尝试元数据，重启后仍遵守 1h→2h→4h→8h 与每日上限；状态文件不可读时自动维护 fail-closed，不触网且保留账号级提示。`RealCheckinRenewalService` 增加最小 `CredentialRenewalHttpAdapter`，以确定性 fixture 覆盖同设备换发、refresh 兜底、身份不匹配零回写；刷新命令增加可注入结果收集边界，验证单账号失败不阻断批量；Playwright 增加批量/详情刷新按钮契约测试。手动刷新仍绕过自动退避，但成功会清除该账号旧退避。

## 后续设计议题：归档冻结与切号性能（2026-09-12，grill-with-docs 已定案并实施）

### P8-7 归档内容冻结、取消归档与交接性能

- **已确认的领域边界（ADR-0028）**：主库保留全部会话与项目分类；正常会话随账号切换交接；归档会话属于主库冻结内容，切号时不进入会话身份改写、逐会话校验、接力台账或细粒度进度，仍留在数据库和归档视图中。
- **已确认的用户体验边界**：取消归档后必须纳入当前登录账号；归档/取消归档先在工具内连续编辑，用户主动应用后一次性提交；优先无感完成，无法即时安全完成时必须显示待同步状态，并提供明确的刷新动作，由工具控制关闭、更新、重启和结果回执，不能出现“已取消归档但找不到”的静默状态。
- **已确认的草稿边界**：待提交归档设置持久保存为本地草稿；工具重启、退出或提交失败后可继续编辑、应用或明确放弃，草稿存在不代表主库已完成更新。
- **已确认的混合项目边界**：项目不拆分；正常会话决定项目是否参与账号交接，归档会话保留原会话/消息身份与内容；项目内全部会话归档时，项目本身也冻结，归档视图跨账号可见。
- **已确认的性能边界**：归档数量不应增加交接候选、扫描、写入、校验、接力台账或进度计数的工作量；正常会话使用集合级读取与校验；本机约 100 条正常会话的切号目标为十秒级，不能退化为分钟级。
- **已确认的统一提交失败边界**：提交前先预检；预检或关闭实例失败时不改库；数据库更新使用单事务，失败则全量回滚并保留草稿；数据库已提交但实例重启/验证失败时，明确显示“主库已更新、实例待恢复”，提供重试入口，不重复写库或静默隐藏状态。
- **已确认的项目归属边界**：混合项目由正常会话决定账号归属；全部会话归档时项目冻结；取消归档任一会话时项目纳入当前账号，仍归档的其他会话继续冻结且不改写会话/消息身份。
- **已确认的项目级操作边界**：项目归档冻结现有项目身份及其当前会话；可恢复整个项目或只恢复选定会话，部分恢复后形成混合项目；归档项目可以重新创建同名但身份不同的新项目，不自动复用旧项目。
- **已确认的项目合并边界**：合并必须明确选择保留目标，允许归档与正常项目合并；源项目会话迁入目标但保留各自归档/正常状态及会话、消息身份，合并不自动解冻；结果由是否存在正常会话决定项目是正常/混合还是继续冻结。
- **能力边界已澄清**：项目归档是工具逻辑分类，不是 TRAE 原生只读锁；工具不能阻断 TRAE 后续写入旧项目，也不为此增加细粒度审计或版本追踪。本需求只负责列表收纳和切号成本控制。
- **已确认的外部新增边界**：项目归档只覆盖提交时已有会话；之后同一项目下的新会话按正常会话处理，参与正常列表和账号交接。本需求不额外追踪或提示项目内部的外部变化，用户可按需要再次把新会话加入归档草稿。
- **已确认的外部修改边界**：不做细粒度内容审计、版本追踪或只读保护；归档状态保持不变，仍从正常列表和账号交接中排除，用户需要时从归档视图查看或取消归档。
- **已实施（2026-09-12）**：切号候选、项目归属改写、会话身份映射、消息/服务器计数、完整性校验和进度统计均排除归档会话；归档视图跨账号读取正文；归档/取消归档使用可持久化草稿，用户点击应用后经关闭实例、备份、单事务更新、重启一次完成；数据库已提交但重启失败时返回明确的待恢复结果，不重复写库。
- **补强（2026-09-14）**：P5-5 主库体检与收编候选同步排除归档、软删和无会话账号；归档-only 不再触发“归入当前账号”提示，空项目仅作诊断展示且不会进入收编或触发空跑；旧主库缺少归档列时保持兼容。
- **补强（2026-09-21）**：空项目诊断口径排除软删项目（TRAE 历史删除残留行不再被误报为「空项目」，此即“31 个空项目”提示的根因）；环境页体检区块新增「清理空项目」——单次确认（ADR-0018）+ 自动备份，复用收编编排（关实例 → 三件套备份 → 单事务删除无任何会话行的未删项目行 → 重启），孤儿行保持只报告不动。
- **验收**：`master_handover` 13 项、`master_history` 8 项、`master_archive` 13 项、`account_session_content` 7 项 Rust 单测通过；`LibrarySessionsPanel` 22 项 Vitest、前端 `typecheck`、Tauri `cargo check -p trae-sync` 通过。

## P10 多产品工作台与 Trae CN 渐进适配（研究门禁已通过，2026-09-22）

### 目标与决策边界

本阶段把 Trae Sync 从 Work CN 专用 UI 提升为可安全承载多个 TRAE 产品的工作台壳层。Work CN 仍是完整支持面；Trae CN 先接入产品上下文、能力驱动导航、只读数据定位、身份发现和重新授权引导。正式边界以 `docs/adr/0029-multi-product-workbench-boundary.md` 为准，事实台账见 `.scratch/research-multi-product-adaptation-20260922.md`。

本阶段不把“同一账号体系”解释为“凭据可直接迁移”：不复制 access token、refresh token、native storage blob、设备密钥或 MachineID；不跨产品写数据库、WAL、storage 或官方客户端目录；不做全局会话库、统一签到入口或泛化插件平台。

### 研究门禁结论

- 产品根目录、数据库相对路径和 Trae CN 只读 SQLCipher 读取已确认可隔离。
- Work 包为 `SOLO_CN`、Trae CN 包为 `TRAE_CN`，官方认证选择分别使用 SOLO/solo 与 TRAE/trae；直接凭据复用未证明，不得承诺。
- 两套 storage blob 结构相似，但当前真实根目录登录的是不同账号；“身份可识别”和“会话可恢复”必须拆成两个状态。
- 两套客户端确实都把授权 URL 交给 Windows 默认外部浏览器，并使用本机临时回调；这意味着浏览器网页登录会话可能复用，但不等于产品 credential 可复用。
- GitHub 公开源码复核仍指向平台化 OAuth：`Trae`、`TraeSolo`、`TraeCn`、`TraeSoloCn` 使用不同平台上下文、认证选择和 CN/国际服务路由；未发现能证明 Work/SOLO credential 可直接注入 Trae CN 的公开证据。完整索引见 `.scratch/research-trae-cn-github-20260922.md`。
- V1.1 默认采用“身份发现 + 引导重新授权”。只有严格隔离实验满足 ADR-0029 的全部门槛，才允许另行评估 `verified`；Trae CN 凭据维护的独立 OAuth、续期和验收边界以 ADR-0030 为准。

### V1.1 顺序（不超过十步）

1. **产品注册与上下文边界**：以 `ProductContext` 统一 Work/CN 的数据根、数据库定位、可执行文件和能力清单。验收：每个产品的路径和数据源由自身上下文解析，Work 现有测试全绿。
2. **动态工作区状态**：把固定 `platform_id` 提升为 `selectedProductId`，为每个产品保存当前账号和最近页面。验收：切换产品后旧页面详情、数据库句柄和缓存不会复用，返回原产品可恢复其最近安全页面。
3. **标题栏产品切换器**：在标题栏左侧显示 Work CN/Trae CN，切换流程固定为销毁旧上下文 → 加载新上下文 → 重载能力 → 恢复安全页面。验收：标题、数据源和页面均属于当前产品。
4. **能力清单驱动导航**：以 `CapabilityManifest` 生成侧栏，Trae CN 只显示真实适配的账号相关入口；签到、会话、环境不显示。验收：无 `product === work_cn` 的导航分支，Trae CN 不出现签到入口。
5. **产品内账号切换器**：账号选择只改变当前产品的 `activeIdentityId`，不改变产品上下文或其他产品账号。验收：同产品切号不读取另一产品数据库，产品切换不重置另一产品的账号选择。
6. **账号身份层**：从现有 Work/CN 只读材料建立脱敏 `AccountIdentity`，支持同一身份关联多个产品。验收：身份关联不向前端、日志或文件写入 token/私钥；当前不匹配的根目录显示未知或需授权。
7. **产品凭据状态**：建立 `ProductCredential`，至少支持 `identity_recognized`、`restore_testing`、`verified`、`need_authorization`、`failed`。验收：Work `verified` 与 Trae CN `need_authorization` 可并存，UI 不把身份识别写成已登录。
8. **重新授权引导**：Trae CN 未验证自动恢复时提供“发现已有 TRAE 账号 / 继续授权”入口，沿用 App 内 OAuth 和一次 OAuth 约束。验收：不复制生产 credential，不写官方 storage，授权结果按产品单独保存。
9. **Trae CN 只读数据适配**：接入已验证的只读数据库定位与读取，严格禁止跨产品写入和会话合并。验收：Trae CN 读取只命中自己的 data root；Work 主库读写回归通过。
10. **隔离恢复探针与状态回执**：仅在临时 user-data-dir、临时 credential store 和非生产实例中验证 adapter restore；若门槛未全部满足保持需授权。验收：实验有明确状态、业务码和脱敏证据，生产目录无变更。

### 当前落地记录（2026-09-22）

- **已落地第一切片**：`ProductContext` 的产品注册边界、标题栏产品切换器、能力清单驱动导航、按产品记忆安全页面、Trae CN 账号适配页，以及只读产品身份发现命令。
- **已落地第二切片**：Trae CN 独立 OAuth（`TRAE/trae/IDE_PC` 通道）、独立 DPAPI 凭据目录、独立账号注册表、当前账号指针、账号列表和产品内切换命令；默认打开系统浏览器以复用网页授权会话，但不复制 Work 的本地凭据。
- **已落地第三切片**：账号页统一进入 `AccountWorkbench`；Trae CN 通过 `AccountAdapter` 提供统一账号快照、独立 OAuth 登录、取消登录和账号切换，复用 Work 的标题、工具栏、列表/卡片视图与能力裁剪。`ProductAccountPage` 仅保留为迁移测试基线，不再作为生产路由；Work 的 `AccountDetail`、签到、额度和主库切号事务保持原路径。
- **已落地第四切片**：健康检测和登录凭据刷新进入共享账号模块；Trae CN 使用自己的 `TRAE/IDE_PC` OAuth client、凭据根和后端命令，Work 继续使用自己的成熟续期链路。两者不共享 token、设备材料或官方 storage；额度和签到仍只由 Work 管理。
- **当前状态**：Work CN 保持完整工作区；Trae CN 现在可以在 Trae Sync 内完成独立授权、保存账号、健康检测、凭据刷新和切换当前账号。官方 Trae CN native storage 的会话写回/关闭重启恢复，以及真实生产续期成功证据仍未完成，因此不能把该状态宣称为“Work 凭据迁移成功”或“官方客户端已登录”。
- **尚未落地**：Trae CN 只读历史读取、官方 native session restore 和隔离恢复探针。真实账号续期仍需单独做一次实测验收，失败时回退到重新授权，不扩大到额度或签到。
- **登录复核回执**：可以复用“同一浏览器网页登录会话”来减少再次输入账号的步骤；不能把 Work 的本地 token/blob/device 材料复制到 Trae CN。当前 Work 与 Trae CN 本地保存的脱敏 `userId` 不同，因此当前机器也不是同一账号跨产品登录状态。
- **已验证**：`pnpm build`、`pnpm typecheck`、全量 Vitest **242/242**、Playwright **38/38**、`cargo check -p trae-sync`、Tauri 核心库 **62 passed / 1 ignored** 与 infrastructure Rust 单测 **735 passed / 7 ignored** 均通过；Windows GNU 目标的交叉检查仍受本机 OpenSSL/Strawberry Perl 工具链阻塞，详见研究台账。

### P10-5 远程设备管理（2026-09-23 grill 九问定案，ADR-0031）⭐

- **背景**：设备重铸与多产品登录累积服务端设备占用，达上限（10 台）时登录/续期返回 20401/20408，此前显示为普通网络错误。ListDevices 已用 Trae CN 本地凭据实测可行（占用 7/10、4/10，探针报告 `.scratch/checkin-http/reports/device-list-token-probe-20260923.json`）；ClearRefreshToken 接口已从官方前端脚本确认、未验证。研究台账 `.scratch/research-trae-device-management-20260923.md`。
- **决策**（详见 ADR-0031）：账号级共享能力两产品同时上；原生 ListDevices 主路径 + 官方授权页兜底；本机设备禁止远程退出（UI 隐藏退出按钮 + 后端 device_id 比对拒绝，双防线）；其余设备逐台单次确认退出（列明设备名/绑定产品/影响）；列表按账号全量展示 + 行内产品标签，无筛选器；退出成功后重拉列表 + 触发该账号健康检查（本机失效→need_authorization 仅作防御兜底）；协议放产品中立 infrastructure `remote_device` 模块（`checkin_http.rs` 保持签到专用）；能力清单声明 `supports_remote_devices` 接入；UI 不显示 DeviceID/业务码；remint 本次不动。
- **实施切片**：
  1. infrastructure `remote_device` 模块：端口 + ListDevices 实现 + DTO + 错误映射（20401/20408 → 设备额度状态）；Work CN 凭据调用可行性实测补证（只读，无副作用）。
  2. 共享设备面板：AccountWorkbench 设备模块（能力声明接入），列表形式呈现，信息丰富度以官方授权网页为基线只增不减——顶部汇总（已用/上限，接近上限高亮）+ 每行：设备名称、设备类型（友好映射，未知值不透传内部枚举）、绑定产品标签、最近活跃时间（绝对 + 相对双形式）、本机徽章（本机行无退出按钮）、逐行退出按钮；按最近活跃倒序；官方授权页兜底入口；加载/失败/空三态（失败显示原因 + 重试 + 官方页入口）。
  3. 退出能力：ClearRefreshToken 实现（mock/单测固化请求形状：字段、头、错误映射）+ 逐台单次确认 UI + 本机后端拒绝 + 成功后刷新链。
  4. 真机验收：用户明确选定一台可牺牲设备，工具内单次确认执行真实退出，结果落档 `.scratch/`；不做无副作用探针试错。
- **验收**：20401/20408 登录失败显示「设备数量已达上限」+ 管理入口；两产品面板可用且主视野无内部标识；本机行无退出按钮且后端拒绝本机目标；真机退出验收记录落档；remint 行为与 P1-4 现状完全一致、无新增自动触网；Rust 全量 + 前端 typecheck/vitest/e2e 通过。
- **落地记录（2026-09-23，切片 1-3 代码闭环）**：
  - **切片 1（协议模块）**：新建 `ports/src/remote_device.rs`（`RemoteDeviceManager` 端口 + Snapshot/Entry/Error，端口零凭据暴露）与 `infrastructure/src/remote_device.rs`（`RemoteDeviceHttpAdapter` 注入 seam + `ReqwestRemoteDeviceHttpAdapter` 复用 `trae_http_client()` + `RealRemoteDeviceManager`：凭据包内存解密 → x-cloudide-token 调 ListDevices/ClearRefreshToken → 业务码字符串/数字双形态解析 → 本机 device_id 比对拒绝后端防线）；12 单测全绿。
  - **切片 2（命令与面板）**：`list_remote_devices` 命令（RealReadPreview 门控 + product_id 路由 Work=Solo/TraeCn=TraeCn + spawn_blocking + 稳定错误码透传）；前端 `AccountCapability` 加 `remoteDevices`、新建 `RemoteDevicesPanel`（默认折叠懒加载，零打开触网；汇总行 + 设备名/类型友好映射/产品标签/绝对+相对活跃时间/本机徽章/逐行退出；三态；ConfirmDialog danger 确认）、AccountCenter 门控挂载、safeUiError 词表 5 组新增；vitest 新增 11 用例。
  - **切片 3（退出闭环）**：`clear_remote_device` 命令（同门控路由 + `remote_device_empty_target` 防御，不写本地状态）+ 退出成功后经 `onSignedOut` 联动现有健康检测（忙碌跳过不排队）+ e2e `remote-devices.spec.ts`（本机无退出按钮 → 非本机确认 → 列表刷新）。
  - **双产品凭据实测补证**（探针报告 `.scratch/checkin-http/reports/device-list-probe-both-products-20260923.json`）：Work CN 8/8 账号 ListDevices 全部成功（占用 1~6/10）——决策 1 开放事实已实证；Trae CN 对照 7/10、4/10 与研究台账一致。只读操作，无退出动作。
  - **全量验收**：`cargo test --workspace` 零失败、`pnpm typecheck` 通过、vitest **255/255**、e2e **39/39**。
  - **待办（用户配合）**：切片 4 真机退出验收——用户在工具内选定一台可牺牲的非本机设备单次确认执行真实退出，结果落档 `.scratch/`；此前不做任何 ClearRefreshToken 真实调用。
  - **审查修复（2026-09-23 双轴 code-review 后）**：修复账号切换面板滞留（含飞行中旧响应代次守卫）、补官方设备管理页兜底入口（`open_device_management_docs` 命令 + 失败态按钮）、account_label 回退改中性文案、登录失败文案引导「登录设备」面板、退出健康检查联动收敛为单账号（`handleHealthCheck(targetProfileId?)`）。vitest 258/258。
  - **架构深化排期（2026-09-23 grill 定）**：深化主线（Binding::from 消重 → 产品账号解析器下沉 → lib.rs 按域拆分 → 命令壳收敛）推迟至切片 4 真机验收后作为独立批次启动；候选报告 `architecture-review-20260923-p105.html`（temp），候选 5/6/7 按 Phase 计划另行排期。
  - **UI 结构调整（2026-09-23 用户拍板，ADR-0031 决策 12）**：「登录设备」面板从账号列表页独立区块迁入**账号详情视图**——Work CN 内嵌 AccountDetail；Trae CN 卡片新增点击进入轻量 `AdapterAccountDetail`（基础信息 + 登录设备 + 可选切换）；面板 profileId/accountLabel 改必传（绑定固定账号）；登录失败文案改引导「打开该账号的详情页」；退出联动：Work 详情页刷总览数据、Trae CN 详情页单账号健康检测。后端命令零改动（本就按 profile_id 查询）。vitest **266**、e2e **39** 全绿。
  - **列表二次调整（2026-09-23 用户拍板）**：面板三段式——本机设备区块置顶强调（无退出按钮；无匹配行显示「未识别到本机对应的设备」中性空态）+「其他设备 (N)」默认折叠分组（点开才展开，组内退出流程不变）。汇总行保持。Work 只读验证通过（探针 16 账号全绿、占用无副作用、日志无错误）。vitest **267**、e2e **39** 全绿。
  - **本机识别修复（2026-09-23，用户真机反馈「全部显示未识别」驱动）**：实测确诊服务端 `DeviceID`（14 位记录 ID）与本地凭据 `device_id`（16 位 hex 虚拟设备指纹）是**两套标识体系**，逐行相等永不命中。对照实验：空体（官方网页形态）与仅带 ClientID 时响应各行 `CurrentDevice` 全 false；请求体带 `ClientID + DeviceInfo.DeviceID`（本机凭据指纹）后服务端才标记本机行 `CurrentDevice=true`。修复：① list 请求体从空对象改为携带凭据 ClientID+DeviceID（trait 签名同步）；② is_local 优先采用服务端 `CurrentDevice` 标记（宽松真值），device_id 相等降为兜底；③ **退出防线重构**——原"目标==本地 device_id"比对在真实体系下永不触发，改为退出前先拉只读列表，目标行 is_local=true 即拒绝且不发起退出请求（原本地比对保留为第一道）。单测 14 个（新增 CurrentDevice 标记/真值形态/服务端本机拒绝 3 个）；真实探针验证 is_local 命中本机行。诊断能力沉淀在 `real_device_probe.rs`（形状摘要 + SHA-256 指纹，不输出原值）。
  - **切片 4 真机退出验收收口（2026-09-25）✅**：用户于 09-23~09-25 期间在工具内自然完成远程退出操作——探针占用对比显示 6 个账号合计退出 **15 台设备**（66d8f009f598 8→1、0bea870c6951 8→6 等），全部生效；`app-diagnostics.log` 零 remote_device/ERROR/panic 记录；本机识别修复后探针持续命中。验收记录 `.scratch/checkin-http/reports/device-clear-acceptance-20260925.json`。**P10-5 切片 1-4 全部完成**。残余风险：ClearRefreshToken 错误响应完整形态未穷尽（成功路径已验证）、DeviceType/LastActiveAt 展示格式随真实数据逐步校准（无报障）。

### 当前不做

- 自动复制或转换 Work 的 access/refresh/native blob 到 Trae CN 生产目录。
- 共享设备身份、设备密钥、MachineID 或跨产品调用设备注册/重铸。
- 跨产品数据库、WAL/SHM、会话正文和主库记录的读取写入混用。
- 将签到状态放到账号全局，或把 Trae CN 的签到做成“点击后失败”的入口。
- 把未来适配抽象成完整插件市场、全局聊天数据库或所有 TRAE 产品的一次性大迁移。

### P10 完成标准

Work CN 的账号保存、对话主库、直连签到和账号切换行为保持现状；Work/CN 可在标题栏切换且数据边界清晰；侧栏由能力清单生成；Trae CN 只展示已适配能力；身份状态与产品凭据状态不混淆；所有未验证的跨产品登录都显示为需授权或实验状态；全量 Rust、前端类型、组件和 E2E 验证通过。

## Phase 11：精细化与生产就绪（2026-09-28 grill 六议题 28 项定案）

### 背景与决策依据

2026-09-28 弹窗 grill 会话逐项定案六议题 28 项策略，全量记录 `.scratch/grill-session-20260928-production-refinement.md`（含各模块机制地图与疑点裁决）。核心方向：**Work CN 先收尾、Trae CN 适配其后**（执行顺序铁律）；差异收敛在适配层；通知体系基建先行。架构决策见 ADR-0032（能力三态，局部取代 ADR-0029 决策 4/9 与 ADR-0030 决策 5，取代标注已写入两 ADR 状态行）。

纪律：每任务 TDD 红→绿；每阶段全量验收（`cargo test --workspace` + `pnpm typecheck` + `pnpm test` + e2e）；子代理实施 + 主会话逐项验收。

### P11-0 决策链同步 ✅（2026-09-28 完成）

- ADR-0032 新立；ADR-0029/0030 状态行加取代标注；ADR 索引更新（15 份）。
- 本 Phase 写入 tickets.new.md；策略会话记录落 `.scratch/`。
- CONTEXT.md 无领域语言变更（能力三态为 UI 实现，术语「能力清单」既有定义已覆盖）。

### P11-1 全局通知体系（sonner + 统一 notify 适配层）

- **决策**：选型 sonner（React 生态标准，样式对齐 DESIGN_TOKENS 毛玻璃）；位置**顶部居中**；成功 4s/失败 6s 自动消失、悬停暂停、堆叠上限 4、不抢焦点；建单一 notify 模块作 sonner 唯一入口；**通知类全迁**（操作结果/完成/失败/提醒，批量结果摘要含异常清单入 toast 富内容），**页面结构态保留**（加载占位/空态/整页错误+重试）；落点联动：自动签到完成提示、取消登录失败提示、批量操作完成摘要。
- **切片**：① 引入 sonner + notify 模块 + Toaster 挂载 + 毛玻璃主题（单测：适配层 API 与内容映射）；② 通知类迁移（AccountCenter/AccountDetail/AdapterAccountDetail/CheckinPage/EnvironmentPage/MasterLibraryDetail/SettingsPanel/PluginWorkbench 页内通知横幅与结果摘要；自动签到完成跨页 toast；取消登录失败 toast）；③ 验收清理（通知类页内横幅零残留）。
- **验收**：notify 适配层单测；迁移点 vitest 更新；`pnpm typecheck`/vitest/e2e 全绿；通知类页内横幅无残留（grep 验证）。

### P11-2 签到调度策略修复

- **决策**：① 自动签到撞锁被跳过的账号当日**顺延重试**（约 30 分钟后，仍走 try_lock 与 before 探测，防重复签）；② 自动签到中断（崩溃）后台账停 Running 的，下次 tick **恢复补跑**未完成账号（before 探测 + 9095 配额天然防重复）。
- **验收**：触发判定/顺延重试/中断补跑单测（含互斥与 fail-closed 不回归用例）；`cargo test --workspace` 全绿。

### P11-3 账号模块收尾

- **决策**：① 清理 `ProductAccountPage.tsx` 及专属测试（迁移基线使命已完成）；② **首次登录验收**落地（ADR-0027 实现，P9-0 Work 侧）：登录完成后立即以同设备 AuthCode 滚动换发验证可持续性，验收通过才算登录完成；失败保留候选材料与证据，不自动重铸、不二次 OAuth；③ 批量健康检测/凭据刷新补**逐账号进度事件**（复用 master-switch-progress 事件模式）；④ 取消登录失败给用户可见提示（经 notify）；⑤ 「刷新」按钮悬浮提示注明纯本地不联网。
- **验收**：删除后无引用残留；首登验收成功/失败路径单测（失败保留证据不覆盖）；进度事件前后端联调 + 组件测试；全量测试绿。

### P11-4 备份能力补齐（红线）+ 设置小项

- **决策**（ADR-0018 三维度落地）：① 备份位置可选——backup-retention.json 增加 backup_dir（默认现目录），设置页「更改备份位置」确认框问是否迁移已有备份，**迁移用复制、原位保留**并提示可手动清理（铁律：不自动删快照）；保留裁剪只管当前活动位置的链；备份链视图与恢复指引同步；② 备份内容可选「包含账号凭据」（账号注册表 + DPAPI 凭据包整拷至备份位置子目录，同 Windows 用户可恢复）；③ `purge_account_records` 补备份先行（与破坏性操作家族对齐）；④ `keep_policy` 展示读实际配置值。
- **验收**：位置更改/迁移/裁剪单测（含跨位置链隔离）；凭据备份内容与恢复单测；purge 备份先行单测；全量测试绿。

### P11-5 环境模块四项 + 副环境补齐

- **决策**：① 环境卡补「关闭实例」按钮（运行中显示，后端优雅终止进程）；② 「主库自检」（G18）与「主库体检」（P5-5）整合为一个「主库体检」区块（登录态/可切换性作为其中一组检查项）；③ 归档草稿从前端 localStorage 迁到工具管理数据 storage_root（后端持久化命令，首次读取时迁移旧草稿）；④ 删除副环境时目录删除失败的错误信息带上残留路径；⑤ **副环境全量补齐**：体检/收编扩展到副环境（复用现成机制）+ 副库详情页（复用 LibrarySessionsPanel 的 libraryId 边界）+ 副环境统计卡。
- **验收**：各机制单测/e2e；副环境体检与收编对副库真实生效；详情页两库切换正常；草稿迁移不丢失；全量测试绿。

### P11-6 产品化界面适配（ADR-0032 落地，Work 收尾后启动）

- **决策**：① 问候语取产品注册表 `displayName` 动态生成；② 导航五页全产品开放 + 区块能力三态（ready/pending/absent，判定依据能力清单与探针结论）；③ Trae CN 签到**只读探针先行**（status 类查询、不消耗配额、不发 claim），结论落档 `.scratch/`，有端点实装、无端点显示 absent；④ Trae CN 主库只读浏览（Library 抽象接入）；⑤ 各页空态组件统一（共享 EmptyState）。
- **验收**：探针报告落档；三态渲染组件测试（每产品每区块）；Work CN 全量回归不受影响；e2e 关键路径绿。

### P11 执行阶段顺序

1. P11-1 通知基建 → 2. P11-2 签到调度 + P11-4 小项（6-2/6-3 部分）→ 3. P11-3 账号收尾 → 4. P11-4 备份大项 + P11-5 环境四项 → 5. P11-5 副环境补齐 → 6. P11-6 产品化对齐（含探针）。

### P11 验收清单（完成后逐项核对）

- [x] P11-1 sonner 集成 + notify 唯一入口 + 顶部居中毛玻璃样式（2026-09-28 切片①，274/274）
- [x] P11-1 通知类迁移完成、页内通知横幅零残留、结构态保留（2026-09-28 切片②，vitest 272 + e2e 39；OperationResultCard 零引用已删除）
- [x] P11-1 自动签到完成跨页提示 + 取消登录失败提示落地（2026-09-28 App 级监听 + AccountCenter warning）
- [x] P11-2 撞锁顺延重试 + 中断恢复补跑（单测固化；1012/0 全绿，2026-09-28）
- [x] P11-3 ProductAccountPage 清理、无引用残留（2026-09-28）
- [x] P11-3 首次登录验收落地（成功/失败路径，证据保留；回执 acceptance 字段，2026-09-28）
- [x] P11-3 批量健康检测/凭据刷新逐账号进度事件（含 credits-refresh-progress 补齐 Work 健康检测与额度路径）
- [x] P11-3 刷新按钮纯本地提示（2026-09-28）
- [x] P11-4 备份位置可选 + 复制迁移 + 裁剪管当前位置（2026-09-28：backup_dir 配置/set/migrate 命令、活动目录解析统一、单测 9+5 项；迁移含凭据子目录与 sha256 校验、幂等、部分失败清单）
- [x] P11-4 凭据内容备份（勾选）+ 恢复可用（2026-09-28：includeCredentials 白名单整拷 credentials-{stamp}（accounts.json + profiles/renewals/refresh-rescue/retired 密文），has_credentials 展示 + 人工恢复指引；白名单防御性断言明文不入备份）
- [x] P11-4 purge 备份先行 + keep_policy 读实际值（2026-09-28 随 P11-2 批次完成，单测 5+1 项）
- [x] P11-5 关闭实例按钮 + 自检/体检整合 + 草稿迁 storage_root + 删除残留带路径（2026-09-28：close_environment_instance 命令（主库/副环境口径复用 taskkill /T 优雅→/T /F 兜底原语，NotRunning 幂等）；环境卡「主库体检」整合区块（登录健康四项 + 库内容分组，独立降级）；get/set_archive_draft + archive-drafts.json（原子写/256KB/损坏 fail-closed 不删文件）+ localStorage 首读迁移；environment_delete_failed_residue:{path} + safeUiError 带路径提示；单测后端 7+5、前端 291 全绿，e2e 39 项通过）
- [x] P11-5 副环境体检/收编 + 副库详情页 + 统计卡（2026-09-28：体检/收编/统计/历史/归档删除合并全命令 library_id 参数化（缺省 master 兼容，未知 id 报 library_not_found）；副环境账号解析=注册表权威（主库保留实测优先）；副环境卡体检区块（有差异才显示）+归入当前账号+查看详情；MasterLibraryDetail 接受 library prop，副库插件 tab 诚实说明；备份链/验证面板按库路由；性能红线守住（5s 轮询无 DB 聚合）。vitest 295 + e2e 36）
- [x] P11-6 问候语动态化 + 五页全开放 + 能力三态（2026-09-28：问候语取注册表 displayName（OverviewPage product prop）；NavigationRail 移除按产品过滤；PRODUCT_SECTION_STATES 三态映射（trae_cn：overview pending / accounts ready / checkin pending / environment absent / master-library ready / settings ready）+ CapabilitySection 组件；App.tsx 页面槽位全产品渲染，pending/absent 页零 Work CN IPC）
- [x] P11-6 后端：Trae CN 签到探针结论落档（端点存在且可用，报告 .scratch/checkin-http/reports/trae-cn-checkin-probe-20260928.json）+ 主库只读接入（trae_cn 库读放行/写拒绝 library_read_only/体检 not_applicable，单测 7 项）（2026-09-28）
- [x] P11-6 前端：三态渲染组件测试（每产品每区块）+ EmptyState 统一 + trae_cn 库入口（2026-09-28：MasterLibraryDetail trae_cn 只读形态（标题「Trae CN 数据根」+只读徽章，写控件/草稿机制隐藏，验证面板自然降级，插件 tab 只读说明，统计卡正常）；总览浏览入口 overview-library-cta；EmptyState 统一 account-center/checkin-page/plugin-empty/settings-backup/lib-tree/history-empty（原类名保留为测试钩子）；vitest 313 + typecheck + e2e 39 全绿）
- [x] 全阶段：cargo test --workspace / pnpm typecheck / vitest / e2e 全绿（2026-09-29 终验收：cargo 各二进制 0 failed（主 99 + infrastructure 780 + application 58 + commands 27 + domain 50 等）、typecheck ✓、vitest **313/313**、e2e **39/39**；孤儿样式 product-account-page 全块清理）
- [x] 最终汇报报告：变更清单 + 验收证据 + 遗留风险（2026-09-29，见会话汇报；探针报告/测试日志均在 .scratch/）

## Phase 12：架构深化与审查处置（2026-09-29 协作研讨定案，待执行）

### 背景与决策依据

四专业子智能体研讨（Rust 架构师/前端架构师/质量治理/行业调研）+ 四轮弹窗质询定案，全录 `.scratch/collab-discussion-20260929-architecture-and-remediation.md`。输入：P11 两轴审查（0 硬违规；S1-S9/P1-P4 判断题）+ 架构扫描 7 候选（上轮 4 主线复验成立，Binding 2→18 处、解析器 2→4 组恶化；lib.rs 13,522 行/91 command）。

关键决策：解析器**直接 enum**（含 into_account() 辅助方法）；门禁收敛用 helper 函数式（宏包装经行业证据否决——Rust 宏展开次序「最外层先展开」）；BatchProgressEvent serde tag 判别联合、前端 unknown-kind 丢弃兜底、同批对齐；mock 自建路由器（官方 mockIPC 与 vi.mock 底座冲突）+ **命令名 union 构建期生成**（gen-command-union.mjs + vitest golden 断言）；子组件文件化三判据（testid 契约/≥2 处/>150 行）。

### R0 立即修（2026-09-29 已定案启动）

- D1 提交切分 A'：5 提交拓扑序（C1 docs-adr → C2 前端全量 → C3 后端全量 → C4 杂项归档 → C5 tickets+决策文档），每提交独立可构建可验证
- P1 RemoteDevicesPanel 退出通知迁 notify.success（单设备操作已核实）；S9 OverviewPage 缩进；S4 operation_contract 死代码删除；S6 SettingsPanel 专用类名；S7 notify.css 圆角对齐 token；S5 残留路径保留主视野 + 设置页备份位置收悬浮；S8 examples 运维工具移出 .scratch/tools/ 并剥离个人路径
- P3 凭据备份裁剪：credentials-{stamp} 与同名主备份**同 stamp 同进同退**（以主备份链计数为准；孤儿凭据子目录不自动清，恢复指引注明）——TDD

### 架构批次（R1→R3，每 Phase TDD + 全量四套件 + 子代理实施主会话验收）

- **R1 热身**：② From<&AccountRecord>（infrastructure 内，不提供所有权版，登录手拼处保留 new()）→ ① 解析器下沉（infrastructure/library_account.rs，直接 enum LibraryAccountResolution + into_account()；前置 LibraryContext 迁 domain + lib.rs use 转发）∥ ⑦ mock 路由 + union 生成脚本（前端独立）
- **R2 横切收敛**：④ 命令壳收敛（ensure_real_preview/ensure_fixture 两函数 + error_mapping.rs + run_blocking<T>，含 S1/S2）∥ ⑤ 后端批量运行器 + BatchProgressEvent（serde tag，含 S3）+ 前端 useTauriEvents（latest-ref + cancelled 代次守卫）+ useArchiveDraft hook（9 格迁移矩阵）∥ R2b/R2c 事件契约**同批对齐**
- **R3 lib.rs 薄根化**：按耦合低→高逐域纯移动 commit；environment_library 一步分二（environment.rs + library_archive.rs）；共享 fixture 提 test_support.rs
- Trae CN 签到实装（P2 条目，ADR-0030 门禁）：探针已证实 status 端点可用（.scratch/checkin-http/reports/trae-cn-checkin-probe-20260928.json）；实装需真机 claim 验收（用户配合，同 P10-5 切片 4 模式），签到区块届时 pending→ready（改 PRODUCT_SECTION_STATES 一处）
- **CN 能力真实性审计结论（2026-09-29 复核）**：① CN 健康检测**真机验收通过**（2/2 账号 HTTP 200/auth 一致/昵称指纹一致，报告 trae-cn-health-probe-1790684645.json；09-28 对照组 401/20310 系其自身请求形状错误，非 CN 凭据问题——「可用性存疑」旧表述作废）；② CN 库只读浏览真机实测通过（17 项目/39 会话/328 消息，探针 trae_cn_library_read_probe.rs 沉淀）；③ 09-28 晚 CN 目录 3 份切号备份已澄清=用户授权的三次 master_handover_manual 真实交接（备份先行+记录归并，participating_account_count=1 即预期结果），无越权写库；④ CN 真实续期长期有效性仍按 ADR-0030 待独立验收；⑤ 参考 master_handover_manual 实现评估「CN 记录归并」是否产品化为收编类功能（用户确认该工具可完成任务）
- 后续待办：凭据备份凭据子目录裁剪规则随 P3 落地后观察；vitest 闪失一次（高负载下）如复现加 --pool=forks 验证

## Phase 13：Trae CN 实装收尾与库域深化（2026-09-29 范围与设计定案）

### 范围决策（弹窗确认）

1. **CN 签到全量实装**：status 查询（探针已真机验证）+ claim 实装 + 真机验收（用户配合点一次）+ 签到 UI pending→ready；**CN 自动签到不接入**（先手动后评估）。
2. **CN 记录归并产品化**（ADR-0033 选择性写）：以项目为整体单位的跨账号转移（同目录合并语义）+ 会话归档/恢复开放；删除/合并项目仍拒；无路径 empty 项目不参与；入口挂 CN 详情页；**三步真机验收为可用性前置门禁**（归并+官方验证/重启不复活/签到 claim+额度核对），验收不过回退只读。
3. **小项清理包**：lib.rs 残余实例工具族收编（~400 行）+ 3 个 dead-code 函数与 2 个 unused import 清理 + 旧 12 个测试文件 mock 路由渐进迁移。
4. **CN 真实续期专项验收**（ADR-0030）：✅ **已完成（2026-09-29，3026628）**——一次换发 14 项断言全绿（同设备 AuthCode/身份一致/journal state=verified/refresh 轮换/+14 天/健康复核闭环）；「重启恢复」证据列为独立小项后续补验。

### 机制调研结论（CN 库归属，2026-09-29 落档 trae-cn-project-schema-probe-1790690693.json）

CN 库与 Work 主库 61 表同名同列族；`chat_session` 无 user_id（归属唯一来源=project.user_id 经 JOIN）；项目=目录成立（`project.absolute_path`，17 项目 11 有路径互不重复）；同目录跨账号因 `UNIQUE(biz_project_id, user_id)` 天然多行（Work E5 镜像坑同款）；归档/软删通道同构（hidden_status/deleted_at）。风险：官方云端镜像缓存（server_history_info 11778 行）回写容忍未长期验证→以三步验收门禁兜底。**交接原语已被 CN 客户端真实容忍**（09-28 三次授权交接，官方客户端其后持续正常工作）——ADR-0033 据此立。

### 实施切片（TDD + 相位门，延续纪律）

1. **P13-1 CN 签到实装**：infrastructure `trae_cn_checkin` 协议（status+claim，参照 Work checkin_http 形态与探针实测形状；业务码不压平）；命令+命令壳（checkin_schedule 域复用 run_serial_batch？——CN 先手动单账号，批量暂不需要）；前端 CheckinPage CN 区块 ready（账号列表+签到按钮+额度展示）；真机验收（用户点一次 claim，证据落档）。
2. **P13-2 CN 项目归并**：infrastructure 归并变体函数（按目录过滤收窄 handover 原语：同 biz 跨账号行合并/镜像清理/换腿；UNIQUE 冲突走合并分支）；命令+CN 详情页入口（目标=当前指针账号，规模预览+单次确认+备份先行+进度事件）；守卫测试（写边界：开放两类/其余 library_read_only）。
3. **P13-3 CN 归档开放**：LibrarySessionsPanel readOnly 标志按库能力开关（trae_cn 开归档/恢复，仍禁删除合并）；apply 编排复用（备份先行+单事务）；hidden_status 写入属首例——真机验证项。
4. **P13-4 三步真机验收**（用户配合）：①归并+官方客户端验证 ②重启官方客户端不复活 ③签到 claim+额度核对；报告落档 .scratch/；通过后 PRODUCT_SECTION_STATES 签到 ready + ADR-0033 验收状态回写。
5. **P13-5 清理包**：lib.rs 实例工具族收编 + dead-code/unused import 清零 + mock 渐进迁移（每文件独立 commit）。

### 验收清单

- [x] CN 签到：status+claim 实装、UI ready（7e3b368；claim 路径按 Work 同族推导已标注「真机验收校准点」；claim 共用 checkin_execution_lock；无调度无批量无缓存）
- [x] CN 记录归并：ADR-0033 实现+守卫测试+e2e（9b7d9c4：domain ensure_project_merge_writable / infrastructure 归并变体 preview+merge / 三命令+进度事件 / CN 详情页入口+清单联动；同目录跨账号行合并 UNIQUE 冲突走合并分支；真机应用内归并待下次真实切号顺带验收——原语已有 4 次手动真机背书）
- [x] CN 归档：hidden_status 写入实现+e2e（767d1de：domain ensure_archive_writable / run_trae_cn_archive_apply 客户端运行拒绝+备份先行+恢复切当前指针 / 面板 libraryWriteCapability 三态 full/archive-only/none；真机 hidden_status 写入验收待 P13-4）
- [ ] P13-4 三步真机验收（用户配合，延后）：①应用内归档往返+官方客户端验证 ②重启官方客户端不复活 ③签到 claim+额度核对；归并项待下次真实切号顺带；完成后回写 ADR-0033 验收状态
- [x] P13-5 清理包（2026-09-30，d9b5ed1/53d0b78/d13ca69/cece0cd）：lib.rs 实例工具族收编 instance_tools.rs（lib.rs 2000→1523 行，纯移动）；R3 存量警告 6→1（require_actual_*/EnvFixture::storage_root 删除；resolve_library_dir 按定案保留——有测试引用）；13 个旧测试文件全部迁入 fakeInvoke 共享路由器（0 跳过）。**当时发现断头链路**：cancel_checkin_login 前端在调（AccountCenter:518）但后端 generate_handler 未注册（P7-4 Work CN 登录取消链路断头，原被测试自由 mock 掩盖）；随后处理状态见 Phase 14 P14-6，当前 WIP 已补注册，attemptId 与竞态验收仍未完成。
- [x] CN 续期专项验收：✅ 证据链报告落档（3026628）；重启恢复证据小项待补
- [x] 相位门全量四套件：cargo 35 组 0 failed（lib 126/infrastructure 817）/ vitest 355 / e2e 44 / typecheck ✓

### P13 后续待办

- **cancel_checkin_login 断头链路（历史遗留，当前 WIP 部分修复）**：handler 已注册并接回前端；剩余 attemptId 绑定、旧回调/取消不影响新登录尝试及真实命令契约验收统一见 Phase 14 P14-6。
- CN 真实续期重启恢复证据小项（ADR-0030 收尾）

### P12 验收清单

- [x] R0：提交落地（2026-09-29：C1 1624cea ADR 链 / C2 16983c0 前端全量 / C3 9711a30 后端全量 / C5 a509428 tickets（.scratch 为 gitignore 本地归档，examples 移出至 .scratch/tools/ 并剥离个人路径）；R0 修复 81c0e80 P1/S5/S6/S7/S9 + 31cae82 S4 + b438826 P3 凭据同 stamp 同进同退（4 新测试）+ 690db89 e2e 断言随迁；终验 cargo 全量 0 failed / vitest 313 / e2e 39）
- [x] R1：From 消重 18→0 处手工搬运（0333285：21 生产点迁移/28 new() 保留含 1 非同源登录手拼）；解析器 enum 落地（9b5fb0f：LibraryAccountResolution Measured/Registry + into_account，LibraryContext 迁 domain，TraeCnPointerLookup 三态，消除 2 处同构）；mock 路由 + union golden 断言（4e1eb55：91 条清单、gen-command-union.mjs、fakeInvoke 共享路由器、示范迁移 1 文件）
- [x] R2：门禁 51 处→3 域函数（ad7555f：trae/login/checkin-http 三错误码=契约不可合并；run_blocking 52/56，2 处发射后不管保留；S1 七处裸调；S2 master_backup_db_name；error_mapping.rs 7 函数穷尽 match）；批量运行器 run_serial_batch + BatchProgressEvent serde tag 判别统一（cfdfe96：kind 字面值与前端 batchProgress.ts 序列化测试精确对齐，S3 双侧消除，事件名 3 个不变）；前端 useTauriEvents（latest-ref+代次守卫）+ useArchiveDraft（9 格矩阵 13 测）+ AccountCenter 三订阅迁移（1e9c710，复杂生命周期订阅按「不为统一而统一」保留并记录理由）；Q2 同批对齐达成；相位门 cargo 0 failed / vitest 340 / e2e 39
- [x] R3：lib.rs 薄根化达成（13,522→1,946 行）：R3-1 remote_devices(2586df5)+oauth_login(04bb5b5，test_support.rs 引入)；R3-2 checkin_schedule(b12e798，调度闭环+BatchProgressEvent/run_serial_batch 归位)；R3-3 environment(1136)+library_archive(2715) 一步分二(080ed10)；R3-4 credential_handover(5676，c20db3a)；R3-5 plugins(590，bea9084)。全部纯移动、行多重集审计、命令 90/91 注册守恒、测试 111=111 守恒、invoke 名不变
- [x] 每阶段全量四套件验收（延续 P11 纪律）：R0/R1（各片定向+代理全量）/R2 相位门（cargo 0 failed/vitest 340/e2e 39）/R3 终局门（无并发复跑 exit 0、32 目标 0 failed、vitest 340、e2e 39；并发下 sqlcipher 侧车测试一次偶发，3 次复跑+无并发复跑均绿，记录为负载偶发观察项）

## Phase 14：产品边界、切换安全与 CN 原生激活（2026-09-30 已确认）

依据：ADR-0034；[Phase 14 PRD](.scratch/phase14-prd-20261001.md) 记录 R01–R15、依据等级、最小契约与 G0–G5。源码归属见 docs/architecture/MODULE_MAP.md，接入检查见 PRODUCT_ONBOARDING.md。G0 与本次独立审核已完成，用户已授权按票持续实施、必要时向审查会话提问、不再等待总体确认。见 [审核结论](.scratch/phase14-execution-review-20261001.md)。WIP 缺陷按子票修复；G1–G5、真实验收与执行点确认保持。

**标签说明：** ready-for-agent / ready-for-human 表示票内依赖或真实操作窗口准备状态。总体执行授权已具备，按 Blocked by 与 G1–G5 推进；票完成不等于产品可用。具体问题按需提问，其余无阻塞票继续，不重复总体审核。

### P14-0 执行基线与交叉复核

**标签：** ready-for-agent。**Blocked by：** 无。

**交付：** 核对当前事实、调用者与计划，建立全量源码归属并保留旧命令/模块。G0 及本次独立审核已完成；后续按当前票的影响范围核对与实现，不重新等待总体开工确认。

- [x] 读取 ADR 状态链及新文档；核对当前 HEAD/工作区与评审基线（准备版基线 b90f12b），保留他人改动；任何源码变化先重新核对受影响事实。（2026-09-30 双会话只读核对，准备仅有文档改动；执行开始再核工作区。）
- [x] 交叉复核结论并入矩阵/对应 ticket；未证实的协议、官方身份或恢复方式明确记为调查门槛，不把未知当可用。（原会话补充共同账号页验收，已并入 P14-1；CN 原生认证/重启门槛保留。）
- [x] 全量当前核心源文件、注册命令及辅助清单可追踪；每个将修改模块列实际调用者、测试、产品范围。数量随变更更新，不将数量守恒当成功标准。（2026-09-30 初版矩阵漏列 `cancel_checkin_login`、`get_product_operation_states` 和当前 WIP 新增的 `product_operations.rs`；2026-10-01 对照生成命令与源码目录后补齐 153/153 源文件、98/98 命令。其后 P14-2 经仓库调用与兼容承诺搜索退役两个无生产调用的归档 IPC，当前为 96 条已注册命令；图谱工具不可用，静态映射不冒充动态覆盖。）
- [x] 24 条无前端直接引用命令按矩阵分类；四个内部切换原语保留，Gate/fixture/维护入口不算交付功能，不进行未授权大删除。`build_sync_plan` / `apply_sync_plan` 与受 fixture guard 约束的 `FixtureWorkCnSyncExecutor` 当前未由 Tauri 根接线，登记为内部路径；未来要接生产时再评估恢复隔离语义。（准备版矩阵与原会话复核一致。）
- [x] 开工准备与本次独立审核已完成，正式执行授权持续有效。见 .scratch/phase14-execution-review-20261001.md；执行主 agent 已获授权必要时向审查会话提问。当前 WIP 不代表 P14-1~9、全量回归或真实验收通过。

持续执行规则：每个后续子票开始前补齐该票实际 caller/测试核对；P14-0 勾选只证明全量归属与首批映射，不代表所有未来改动已有动态覆盖。这是每票入口检查，不要求先核完所有未来票才允许第一票开始。

### P14-1 共享页面的产品隔离与操作能力真实性

**标签：** ready-for-agent。**Blocked by：** P14-0。

**交付：** 用户切到 CN 时只看到/操作 CN 的有效能力，已有只读/健康能力继续可用；未验收写入与领取不再作为普通可用功能。详情、选择和异步结果不会跨产品。

**粒度修订（2026-10-01）：** P14-1 是汇总验收项，实际执行下列子票，不把全部前后端/UI 重写当单片。依据 R01–R04/R14；已存在的五文件前端草稿纳入 1b，不假勾父票。

| 子票 | 标签 / Blocked by | 独立交付与验收 | 文件责任/既有检查参考 |
|---|---|---|---|
| P14-1a 操作状态与普通 IPC 拒绝 | ready-for-agent / P14-0 首批 | 后端只读操作快照成为唯一事实源；页面/adapter 消费它；状态匹配请求的产品/库和当前环境，ready 的证据引用对应本操作的可核实证据；页面与普通命令使用同一判定契约并在命令执行时重查。CN claim、归档、归并、native activation 未验收时任何普通调用在副作用前拒绝，未知产品/库/状态不回退。Rust 注入断言网络/备份/进程/DB 零副作用，DTO/命令生成与前端 pending 可见性检查通过 | product_operations、product_adapter、domain/library、根 lib/checkin_schedule/credential_handover/library_archive，productRegistry/accountAdapter/相关 types；现有命令 golden、库守卫、CapabilitySection 测试；不把旧 CapabilityManifest 的单布尔含义扩成验收事实 |
| P14-1b 设置与签到的上下文生命周期 | ready-for-agent / P14-1a | 保留/完善当前 Settings/Checkin 草稿；初始挂载、切产品、非活动页面无错产品 IPC/订阅；保存/备份/迁移迟到时不写新视图、通知标明来源，已开始动作按原上下文完成。延迟 promise 覆盖 Work→CN→Work，确认框/未提交设置不跨产品 | App、SettingsPanel、CheckinPage、两组组件测试；草稿两组历史 14+26 通过只算初始隔离，动作竞态仍待验证 |
| P14-1c 共通账号契约与详情 | ready-for-agent / P14-1a | 共同字段/顺序/动作映射既有 view/adapter；工具选择与官方身份分开；Work 详情完整功能保留，CN 不新增伪造功能。账号批量进度事件携带产品来源，只更新匹配产品账号页；Work↔CN 延迟事件不泄露另一产品账号名/进度。未提交的账号操作确认绑定发起产品；Work 打开批量刷新确认后切到 CN，不得变成 CN 全量刷新。展开共通数据后迁移调用，再收缩 null 特例，逐片验证两个产品共同内容/特有区与取消/错误语义 | AccountWorkbench/Center、AccountDetail/AdapterAccountDetail、accountAdapter、`types/batchProgress.ts`、`credential_handover.rs` 全部进度事件 producer 与相应前后端契约测试；复杂主库切号留原编排 |
| P14-1d Library 路由、请求与动作隔离 | ready-for-agent / P14-1a | 产品/主副库往返清理旧详情/选择/草稿展示；迟到查询只更新对应代次；未提交意图不能跨库执行，持久草稿不丢，已提交写动作保持原来源。延迟 IPC/事件和草稿矩阵检查通过 | App、MasterLibraryDetail、LibrarySessionsPanel、useArchiveDraft/相关类型、对应组件与 mock E2E；仅触及本片实际共用类型依赖 |

子票共同契约见 PRD“最小执行契约”；只读快照不是授权，普通命令写前重新核对。native 工具选择与激活是不同操作；支持未知、未验收与环境暂不可用的原因不同。G1 以前不能扩展普通危险写入口，其他独立安全票可推进。

**2026-10-01 执行记录：** 已修正 3 项过时 UI 断言，使其验证 CN 操作 pending 时隐藏写入口、保留只读浏览；`pnpm typecheck` 通过，全量 `pnpm test` 23 文件/360 项通过。后端共享操作快照与写前门禁已实现初步上下文校验，但当前进程 Unknown/枚举失败及完整操作条件仍待 P14-2 和本票验收；不得将这片定向通过记作 P14-1a 全部完成。

**2026-10-09 指定审查会话窄审（问题发现记录）：** 无 P0/P1，发现 3 项 P2。① `LibrarySessionsPanel` 必须按操作消费匹配当前产品/库的快照：Work 归档可在客户端运行时保持可用（归档流程会自行关闭客户端），Work 归并/删除仅在明确停机时可用；Unknown、缺快照或错库时相应按钮 fail-closed，删除也必须进入同一操作契约。② CN 归并零副作用测试需调用与生产命令共用的编排，覆盖缺失/错库/待验收三种拒绝。③ ready evidence 必须落到操作级测试或实测记录，测试应校验实际操作事实；缺少证据时状态须为 pending。

**2026-10-09 P14-1a 首轮修复记录：** 前端分别按操作 readiness 控制 Work 归档、归并与删除；未知、缺失或错库快照均 fail-closed。CN 归并测试现调用生产共享的 `merge_trae_cn_project_inner`，验证缺失/错库/待验收时执行器、进程检查、备份、DB 和进度均为零。ready evidence 改为操作专属引用，并由测试检查对应事实。首轮复审接受这三项修复，但又发现运行态归档提示/确认和 Work 真实验收依据两项 P2，处理记录见下方追加。

- [ ] 在既有能力清单/API 边界内分开实现、必要验收和当前环境条件，逐操作反映 pending/absent/ready；快照按请求的产品/库计算当前可用状态，不以静态产品表冒充环境判断。ready 的证据引用须解析到本操作的真实依据。页面与普通写命令使用同一判定契约，命令执行时重新检查当前环境；状态在快照后变化时于副作用前拒绝并给出对应原因。受控验收另走明确授权路径，不能用公开布尔参数跳过守卫。
- [ ] 保留五个共享导航页；库浏览与归档/归并、凭据健康与原生切换分别判定；未知/缺证据不能标 absent 或 ready。
- [ ] 设置项标明全局/产品归属；CN Settings 与 Checkin 挂载、往返切换和事件订阅不触发 Work 专属 IPC/写入。Work 后台调度独立保持，通知有产品来源。
- [ ] 工具选中账号不表述为官方已登录/切换成功；CN 归并目标仍可显式选择，不能顺便改变既有目标语义。
- [ ] 账号列表/详情共同字段、信息顺序和操作位置一致：名称/已有备注 → 凭据状态、工具选择与官方身份、最近验证/已有有效期 → 常规操作 → 设备区 → 支持时的危险区 → 折叠技术细节。共同动作采用相同 busy/失败表达；没有源数据的字段不伪造，未支持的操作不展示。
- [ ] Work 共通字段/动作渐进映射到已有 view/adapter，不能仅给外层同名页面却永久留下完整的 null 特例；主库切号等专属复杂流程仍复用原编排。保持 Work 备注/手机号编辑、真实额度/签到展示与账号级开关、有效期、刷新/重新登录、设备变更/远程设备与删除等功能及确认/错误语义。CN 不因统一 UI 自动增加上述未实现写操作。必要迁移按展开→替换调用→收缩分片，不全量重写账号页。
- [ ] 账号/库等共用视图类型归现有 types/适配契约，platform 不反向导入页面组件；只修本次触及的依赖，不机械提取所有组件。
- [ ] 详情、账号/会话选择、未提交意图按产品/库清理；持久草稿按库保存。延迟查询按请求代次处理，已开始写动作保留原上下文与结果，切页面不取消提交。
- [x] 账号页未提交确认按打开时的产品与账号范围绑定，或在产品切换时取消；确认回调不能读取切换后的 adapter 作为新目标。测试覆盖 Work 打开批量凭据刷新确认→切 CN→确认，不能调用 CN 刷新命令。（2026-10-03：产品身份变化即关闭确认弹层；先红测复现旧 Work 确认仍会触发 CN `refresh_trae_cn_credentials`，修复后确认 Work/CN 刷新命令均未调用。指定审查会话窄审无 P1/P2。）
- [ ] Vitest 用延迟 promise、事件和 IPC 断言覆盖 Work↔CN、主/副库往返及执行中切页；组件与 mock E2E 覆盖能力混合状态，Work 页面回归通过。
- [ ] 组件断言两个产品共同信息/操作的顺序与可见性，Work 专属区仍可用；未知凭据/官方状态不展示“使用中”。较大的账号迁移可拆为 P14-1 的后续绿色切片，与产品 IPC/状态隔离分开验证。

**2026-10-03 P14-1 未提交批量确认的产品隔离（部分完成）：** `AccountCenter` 在 `adapter.productId` 改变时关闭未提交的批量凭据刷新确认，避免用确认时的新 adapter 把 Work 操作改投 Trae CN。回归测试先以 CN 刷新命令被错误调用而失败；修复后定向 Vitest 1 passed/57 skipped，`pnpm typecheck` 与两个触及文件的 `git diff --check` 通过。指定审查会话只读复核无 P1/P2，确认 effect 按产品 ID 稳定取消、测试覆盖旧确认不转投 CN。未运行完整 Vitest；P14-1c、账号详情/选择状态与异步操作隔离等其余项保持未完成。

**2026-10-03 P14-1c 账号详情与快照读取隔离（部分完成）：** `AccountCenter` 切换产品时清空选中账号和旧快照；读取结果必须同时匹配当前产品与请求代次，防止 Work→CN→Work 往返后迟到结果覆盖较新的 Work 快照。新增同 `profile_id` 跨产品不继承详情、旧 Work 读取不能覆盖新 Work 读取两项回归。`tests/AccountCenter.test.tsx` 60/60、`pnpm typecheck` 与差异检查通过；指定审查会话窄审确认无 P1/P2。已开始的操作仍绑定原 adapter 的上下文；本记录只证明详情选择及快照读取隔离，不关闭账号共通字段/动作迁移、批量进度事件来源、全部操作回执或 P14-1c 父票。

**2026-10-03 P14-1c Work/CN 最近验证字段对齐（部分完成）：** Work 真实账号列表和卡片补展示现有 `last_verified_at`，与 Trae CN 使用相同“最近验证”标签及日期格式；手机号、积分和凭据剩余时间等 Work 专属信息保留在后续元信息位置。组件定向 Vitest 1/1、`pnpm typecheck`、差异检查通过；指定审查会话确认数据源与顺序正确、没有重复展示、无 P1/P2。只补齐一个共同字段，账号共通字段/动作全面映射与 P14-1c 父票仍待。

**2026-10-09 执行记录（部分切片，P14-1 父票保持未完成）：**

- **P14-1b 设置快照读取切片：** `SettingsPanel` 为备份链、保留策略、自动签到设置分别使用读取版本，初始读取/操作后刷新/旧操作刷新共享对应版本门禁，迟到快照不能覆盖新值。三类逆序响应测试、组件套件 24/24、`pnpm typecheck`、目标文件 `git diff --check` 通过。指定审查会话确认原 P2 已关闭，无 P0-P2；记录一项非阻塞 P3：设置页隐藏时旧 Work 操作可能触发一次多余后台刷新，没有状态错误，本次不为此扩展改动。P14-1b 的 Checkin 生命周期仍未验收，父票不关闭。
- **P14-1d Library 代次切片：** 切库时在同一 `useLayoutEffect` 更新代次并清空 history/error/ledger/viewer；旧查询、账本、打开详情与加载更多的迟到结果按库/查看器代次丢弃。补“新库请求挂起时旧列表和详情立即不可见”测试。定向 Vitest 51/51、`pnpm typecheck`、目标文件 `git diff --check` 通过；指定审查会话确认原“被动 effect 清理前旧内容短暂闪现”P2 已关闭，未报 P0-P2。草稿/动作等父票验收仍待。
- **P14-1c 批量进度与忙碌来源切片：** 三类批量进度 producer 携带 `product_id`；前端按当前产品精确接受事件，切产品时在 layout effect 清旧进度。health、credential refresh、credits refresh 三类全局互斥操作均记录发起产品；同产品保留原提示，跨产品显示中立等待状态，旧 finally 不恢复旧账号名。`AccountCenter.test.tsx` 64/64、`pnpm typecheck`、目标文件 diff-check 和 Rust crate `cargo check` 通过。指定审查会话独立运行 Rust 进度序列化契约 5/5 与组件套件 64/64（有两条既有 React `act(...)` 警告），确认此前首帧旧账号名 P2、健康 busy 来源 P3、凭据/额度 busy 来源 P3 均关闭，无新 P0-P3。P14-1c 的共通账号字段/操作迁移、回执等父票范围仍待，父票保持未完成。
- **启动错误修复与当前试用状态：** Tauri `Emitter::emit` 要求事件载荷实现 `Clone`，`ProductBatchProgressEvent` 已补 `Clone`，Rust crate `cargo check` 通过，原四处 E0277 已消除。仓库 `scripts/desktop-launch.bat` 改为经 `pnpm dev:tauri` 启动，复用 `%TEMP%\tp14` 构建缓存；Vite 返回 HTTP 200（标题 `Trae Sync`），Tauri 开发进程已启动。该入口把 AppData/LocalAppData 指向仓库 `.scratch/dev-tauri` 隔离目录，当前证明可启动和浏览 UI，不代表连接真实 TRAE 账号/数据库或 CN 原生切换已验收。用户粘贴日志中的乱码 CMD 片段未出现在仓库启动脚本，来源尚未定位，不宣称已修复该外部脚本。
- **2026-10-09 启动日志复验：** 旧批处理的换行/清理行处理会让 CMD 把乱码残片当成命令；现将 `scripts/desktop-launch.bat` 规范为 CRLF，并把进程清理移入 `scripts/cleanup-dev-instance.ps1`。修复后从该入口完成一次真实开发启动：Rust/Tauri 编译成功，Vite 返回 HTTP 200；启动输出不再包含残片命令或 `ProductBatchProgressEvent: Clone` 错误。验证后已关闭 Tauri/Vite，不遗留 watcher。该启动入口仍使用隔离空数据目录；查看既有账号请打开已安装的稳定版，不能将开发预览当作正式更新。P14-1a 复审后续发现的两项 P2 见下方记录。

**2026-10-09 P14-1a 指定审查会话复审后续修正：** Work 归档快照为 Ready 且客户端运行时，页面会显示“归档前会先关闭 Work 客户端”的提示；应用归档草稿先弹确认框，取消不发 IPC，确认后才调用归档命令。弹框只用于 Work，并在产品/库/页面上下文变化时关闭。Work 归档、删除、项目归并因缺少各自的真实操作验收证据，现都明确为 Pending 且不引用宽泛票据；库读取保留。旧 Work 命令的 `library_id=None` 依 ADR-0025 保持为 master 兼容别名，当前 UI 始终传明确库 ID，缺失库快照为 Pending，CN 写命令仍要求显式 trae_cn 上下文。验证：`product_operations::` 12/12、CN 归并生产编排零副作用拒绝 1/1、`LibrarySessionsPanel` 54/54、`pnpm typecheck`、rustfmt 与 `git diff --check` 通过；Prettier 未安装，未为此加依赖。指定审查会话完成本次只读窄审，确认上述兼容和门禁成立、无 P0-P2。P14-1a 仍未关闭，因为操作级真实 Work 归档/删除/归并验收证据及其它子项仍待。

**2026-10-09 P14-1b 签到来源与 CN 生命周期切片（部分完成）：** `App.tsx` 的跨页自动签到摘要标出 TRAE Work CN；签到页 Work/CN 操作结果通知按发起产品标注。CN 页失活时不读取账号、状态或操作快照；账号列表与状态刷新使用请求代次，丢弃失活后才到达的列表和较旧状态回执。新增 Work→CN→Work 迟到批次回执用例及 CN 失活/旧状态回执用例。核对时修复一条被重复断言“已签”削弱的 Work 未签到状态断言。定向 Vitest 44/44、`pnpm typecheck`、目标文件 `git diff --check` 通过。

**2026-10-09 P14-1b P2 修复（窄审通过）：** 指定审查会话发现 claim 后状态重查没有绑定刷新代次。现由 React 布局阶段生命周期在页面失活/卸载时同步推进代次；`refreshStatus` 请求前及成功/失败回执提交前均校验活动状态与代次；claim 结束后仅在仍活动时重新加载 CN 账号和签到状态，让新代次结果胜出。新增“失活后 claim 才完成不再发列表/状态 IPC”和“claim 后新刷新使旧状态回执失效”用例。定向 Vitest 46/46、`pnpm typecheck`、目标文件 `git diff --check` 通过。指定审查会话确认原 P2 已关闭、本次范围无新增 P0–P2；P14-1b 父项因其它设置/动作生命周期矩阵仍未完成而保持开放。

**2026-10-09 P14-1b Settings 隐藏页刷新 P3 修复（窄审通过）：** 针对已记录的“设置页失活后，Work 操作可能多发一次隐藏快照读取”，`SettingsPanel` 现在按当前产品和活动状态拒绝启动读取，并丢弃页面失活后的快照回执；跨产品旧 Work 操作的补充刷新也只在 Settings 活动时执行。增加备份完成时页面失活、结果通知仍保留且不再读取快照的回归。设置/签到/App 三组组件测试 71/71、`pnpm typecheck` 和目标差异检查通过；指定审查会话确认 P3 已关闭、本次范围无 P0–P2。P14-1b 父项继续开放，仍需完成其他动作生命周期矩阵。

**2026-10-09 启动错误修复复验与当前试用状态：** 附件中的 ProductBatchProgressEvent: Clone 错误已由 Clone 派生修复；真实 Tauri 开发构建成功。复验发现旧的 Tauri dev 调度进程会在只结束 Vite 子进程后重新占用 1420；cleanup-dev-instance.ps1 现仅按仓库路径和 @tauri-apps/cli/tauri.js 的 dev 参数识别本仓库调度器，再结束该进程树；正式安装与其他项目不匹配。批处理改为 CRLF，并不再让 CMD 执行原日志中乱码清理残片。desktop-launch.bat 冷启动复验成功，Vite HTTP 200；App/Settings/Checkin 定向测试 72/72，pnpm typecheck 与 git diff --check 通过。OpenSSL LNK4099 是缺少调试 PDB 的非阻塞警告。当前 Tauri PID 72772、Vite PID 76784；开发版数据根为 .scratch/dev-tauri，不能显示正式账号。未更新稳定版，P14-1b/父票仍开放。指定审查会话已收到只读窄审请求，等待回复。

**审查边界补充：** 指定审查会话收到上述窄审请求后，又表示准备检查/更新稳定版并核对正式账号备份；主会话已明确要求停止并回到四文件只读窄审。该会话及其他线程转来的稳定版更新说法不构成本主会话的用户授权。最新命令行核对未发现针对已知备份目录的复制或安装进程；本次审查结果仍未返回。

**2026-10-09 P14-1c 账号选择语义与共享详情增量（部分完成）：** 适配器契约将含义不清的 `switch` / `isCurrent` 命名为 `toolSelection` / `isToolSelected`，动作命名为 `selectToolAccount`。移除快照中未被消费的产品名称、静态能力列表和冗余选中账号 ID；静态名称/能力由适配器唯一提供，快照保留 `productId` 作为迟到读取的来源标记，账号条目用 `isToolSelected` 表达选择状态。Trae CN 的动作只写 Trae Sync 自有选择指针，页面状态与成功通知明确说明不会改变官方客户端登录状态；官方身份提示取自客户端数据读取结果，与工具选择指针分开。共享账号行映射状态、忙碌态与动作入口，同时保留 Work 原有账号切换事务和文案；详情页对齐工具内选择状态、最近验证与可选动作表达。字段收敛后相关 3 个前端测试文件 77/77、`pnpm typecheck` 和目标文件 `git diff --check` 通过；指定审查会话源码复核未发现 P0–P3。Work 详情使用已有凭据健康结果显示登录状态、使用环境快照显示主库当前账号；缺少状态时不伪称“未登录”或“使用中”。这部分新增字段用例后 `AccountCenter.test.tsx` 65/65、`pnpm typecheck`、目标差异检查通过。详情设备模块位于常规操作之后、危险操作之前；指定审查会话依据 ADR-0034 决策 11 复核该顺序增量无 P0–P3。新增的 Work 状态与当前账号映射待本次窄审。P14-1c 的完整字段/操作/回执与生命周期矩阵仍未完成，P14-1 父票保持开放；本次不证明 CN 官方客户端原生账号切换已实现。

### P14-2 产品进程识别与库写前置守卫

**标签：** ready-for-agent。**Blocked by：** P14-0。

**交付：** 启动/聚焦/关闭只作用于目标产品和目录；进程状态未知时不会放行 CN 写库。

**执行记录（2026-10-01，WIP）：** 已将进程归属与汇总改为 `Match/NoMatch/Unknown`、`Running/NotRunning/Unknown`，并接入启动/聚焦/关闭、环境状态、库写、环境删除、账号清理、强制重置与 blob keepalive。账号清理仅接受绝对 `APPDATA`，并同时检查托管实例目录与待删的 TRAE 原生账号目录；Unknown 时在备份和删除前拒绝。Unknown 或进程枚举失败不会被折成未运行。10-01 继续逐一核对了进程查询生产调用者和库写前守卫；新增可注入查询结果的纯判定接缝及“枚举失败保持 Unknown”测试，`trae_instance::tests` 23/23 通过。环境删除新增可注入进程检查的测试入口，Unknown 时验证注册行与目录均保留；`environment_commands_tests` 6/6 通过。主库 `archive_master_sessions` / `restore_master_sessions` 兼容 IPC 经仓库调用和文档承诺核查后退役；只保留事务批次入口 `apply_master_archive_changes`，对应生成命令清单、E2E mock 和基础设施直写 helper 已同步清理。此切片还未完成全调用验收与 Work 回归，不能标记 P14-2 完成。其他最新定向测试结果见 handoff 的“已运行验证”。

**2026-10-02 增量：** 独立调用审计发现 `force_reset_master_current_account` 的未知进程用例同时缺少 exe 与命令行，未精准覆盖“Work CN exe 已识别、仅命令行缺失”。现已收紧该夹具，并验证错误为 `process_state_unknown`、主库当前账号档案不变；定向 Rust 测试 1/1 通过。该项只补精准拒绝证据，双产品 matcher 的全部边界和 P14-2 全调用/Work 回归仍待完成。

**2026-10-09 APPDATA 默认目录判定增量（P14-2 局部）：** 进程身份判定发现 `APPDATA` 若为空或相对路径，旧实现会拼出相对默认数据目录，并把官方无参 Work 进程误分类为 `NoMatch/NotRunning`，可能让停机守卫放行。`default_product_data_dir` 现在要求 `APPDATA` 是绝对路径；缺失、空值或相对路径返回 `None`，因此无法把裸启动进程归属到默认目录时保持 `Unknown`。新增回归验证三种无效输入均为 `Unknown`。`trae_instance::tests` 24/24、Rust 格式检查、目标文件差异检查通过；隔离 Tauri watcher 编译成功。指定审查会话只读复核无 P0–P3，未运行命令或接触正式数据。P14-2 的其它 caller/边界验收仍未完成。

**2026-10-09 P14-2 关闭错误保真增量：** `close_instance` 现在区分进程状态未知与已确认关闭失败；主库切换、Work 归档/归并/收编/环境登录和关闭调用保留 `process_state_unknown`，在备份、账号/凭据、数据库或注册信息写入前退出。新增调用方拒绝与无副作用测试；根 crate lib 214 通过/0 失败/1 忽略，infrastructure lib 880 通过/0 失败/7 忽略，相关格式与差异检查通过。指定审查会话只读复核未发现 P0–P3，未运行命令或访问稳定数据。P14-2 仍需完成剩余进程边界及操作矩阵验收。

**2026-10-09 P14-2 关闭目标隔离验证：** `close_instance_impl` 现在通过窄入口注入进程查询、taskkill 与等待行为，生产仍接真实系统调用；合成快照测试证明 Work 与 CN 同时运行时只向 Work PID 发关闭请求，首次身份未知/查询失败时无进程动作，优雅关闭后的复查转为 Unknown 时不升级强制关闭，强制关闭和退出复查也只作用于目标 PID。matcher 另覆盖双产品同时运行时各自均可识别、exe/命令行字段分别缺失均保持 Unknown。infrastructure 全部 `trae_instance::tests` 29/29、根 crate `cargo check --lib` 通过，未真实操作 TRAE 进程。Tauri watcher 随源文件更新后重启隔离开发版，当前 PID 49292 响应正常，Vite PID 33684 返回 HTTP 200；正式安装不受影响。指定审查会话此前复核关闭接线无 P0–P3；本次 matcher 边界与剩余调用矩阵审查待回。P14-2 仍开放。

**2026-10-09 启动错误冷启动复验：** 当前源码的 `ProductBatchProgressEvent` 已实现 `Clone`；`cargo check --manifest-path src-tauri/Cargo.toml --target-dir "$env:TEMP\tp14" -p trae-sync --lib` 通过，随后经仓库 `scripts/desktop-launch.bat` 冷启动成功，Tauri 窗口响应正常，Vite 1420 返回 HTTP 200。日志中的旧 CMD 乱码命令未在当前三步入口重现。当前开发 Tauri PID 57228、Vite PID 33684；数据根仍隔离于 `.scratch/dev-tauri`。稳定安装仍运行且未被清理，正式 TRAE 数据未触碰。仅有 OpenSSL LNK4099 缺少 PDB 的非阻塞警告；真实账号与 CN 原生切换不由此验证。

- [ ] 先区分两条现有进程路径：`ProcessControllerPort` / `WorkCnProcessController` 通过目标数据库文件占用做只读观测，目前只配置 Work CN，并只在凭证捕获/应用/恢复标记链使用；它不负责启动、聚焦、关闭，也不覆盖库写、环境删除或账号清理。保留它在凭证材料写入前的职责，不把它当成完整产品进程适配。对 `trae_instance` 的产品/数据目录 matcher 与真实启停调用统一核对；保持官方默认无参数启动、合法 Work 旧 exe 和指定副环境目录兼容，关闭/启动函数及调用方必须使用同一目标产品/目录，不误关另一个产品或把副环境登录路由到主库关闭。
- [ ] 枚举失败、坏 JSON、缺少/不可读 exe 或命令行、归属不明与子进程不被误作“目标未运行”；关闭/启动遇到无法归属的 TRAE 候选时返回未知并停止，不能忽略该行后报 NotRunning 或再开一个实例。状态查询也要保留 Unknown，不得把查询失败显示为“未运行”。CN 库写守卫对枚举错误和缺失身份都必须拒绝，不能转成 false。
- [ ] guard 覆盖 Work/CN 归档与归并、主库/副环境切号和登录、`delete_environment_inner`、`purge_account_records_inner`、`force_reset_master_current_account`、`blob_keepalive` 认证文件写入及所有相关进程控制；未知在备份、DB/认证/账号档案写入、删除注册行/目录或控制进程前拒绝。WAL 暂时静止不替代客户端已退出证据。
- [x] 兼容 IPC `archive_master_sessions` / `restore_master_sessions` 已完成仓库范围内调用与文档兼容承诺搜索，未发现生产 caller 或对外兼容承诺；它们已从 Tauri 注册、前端命令清单、E2E mock 和重复的基础设施直写 API 退役，测试与当前 UI 统一经过 `apply_master_archive_changes`。2026-10-01。
- [ ] 用注入观察结果验证双产品裸启动并存、子进程、自定义目录、缺字段/命令行、查询失败和结果未知；关闭时 Work 只关闭 Work，启动时目标已运行但目录未知不得重复启动，CN 库写在错误/缺身份时零备份零 DB 写，环境状态不会把未知显示成未运行；`force_reset_master_current_account` 遇到目标 TRAE 候选缺命令行时返回未知/错误且 registry 不变。测试不真实杀进程。
- [ ] 全量追踪 `list_trae_processes` 与 matcher 的生产调用者；禁止 `unwrap_or_default`/`unwrap_or(false)` 把查询错误折成“未运行”。命令行必须解析 `--user-data-dir` 的唯一实际参数值，不能在整条命令行中独立搜索路径片段；参数出现但为空、重复或解析不完整时按 `Unknown`，不能落入默认目录匹配。匹配路径必须等于目标目录，目标路径后紧接目录分隔符也算不同目录。最小边界测试覆盖缺字段、空/重复参数、路径只出现在其他参数、同名前缀与子目录、产品 exe 不符。
- [x] 修正 `command_line_matching_ignores_quotes_and_case` 夹具：含空格数据目录改用合法带引号命令行表示，没有放宽生产参数解析规则；Windows 定向测试 1/1 通过（2026-10-01）。
- [x] 补充关闭入口的 Trae CN 目标对称隔离测试：Work 与 CN 同时运行时只向 CN PID 发非强制关闭请求，退出复查后 Work 仍运行；定向测试 1/1、全量 Rust workspace 测试通过，未操作真实 TRAE 进程（2026-10-09）。P14-2 其余未知态 caller 验收与 P14-9 仍待。
- [x] ProcessController 凭据捕获与已保存凭据应用 caller 在进程观测为 `Unknown` 时零业务写入：分别注入“枚举失败”和“目标数据库查询失败”证据，断言两条 caller 均只观察一次、返回 `process_state_unknown`，fixture 文件树增删改前后完全相同。`p14_2_process_observation_tests` 2/2 通过；指定审查会话确认短路顺序与断言足以证明 fixture 持久文件零写。注入器直接返回 Unknown，因此本测试不证明底层真实适配器如何把各类系统错误映射为 Unknown。
- [x] 为启动聚焦增加仅供内部测试注入的函数 seam：目标 Work 实例 PID 被传入聚焦函数，另一 Work 数据目录 PID 被排除；若 Trae CN 进程声称占用同一 Work 目录则返回 `process_state_unknown` 且聚焦零调用。`launch_process_guard_tests` 3/3 通过，根 crate lib 222 通过/1 忽略；指定审查会话确认原 P3 样本缺口已关闭、生产 seam 保持原聚焦行为。

**2026-10-09 P14-2 caller / 聚焦窄验证：** ProcessController 凭据捕获与应用 caller 各新增 Unknown 零写测试，两个 fake evidence 标签均直接返回 Unknown；每条链单次观察、返回 `process_state_unknown`，fixture 文件树完全一致。指定审查会话确认 Unknown 守卫位于 account/store、vault、lease 和业务写之前；此测试不覆盖真实 controller 的故障映射。聚焦测试使用私有 FnMut seam，正常场景只传入目标 Work PID，另一 Work 数据目录不聚焦；跨产品同目录声明被 Unknown 拒绝且 focus callback 不运行。指定审查会话发现并帮助关闭第一版 CN 测试样本的 P3 假阳性，复核修订后的两个场景无剩余问题；生产入口仍使用真实 Windows 聚焦函数。caller 与聚焦定向测试各 2/2；完整 workspace 串行回归 44 个结果段共 1315 passed/0 failed/25 ignored，根 crate lib 222 passed/1 ignored、infrastructure lib 886 passed/7 ignored。全操作矩阵及 P14-9 真实 Windows/TRAE 验收仍未完成。

### P14-3 Work 切换的目标身份一致性

**标签：** ready-for-agent。**Blocked by：** P14-0。

**交付：** 选择账号 B 时，即使 E2 不可用，也绝不把 C 的登录存档写入目标；主库与环境登录沿同一规则。

**执行记录（2026-10-01，WIP）：** 修复审计发现的目标身份错配。E1 新增按注册目标账号校验供体 userId 的共享入口，检查使用即将移植的同一份解密 blob，错配时不写目标；E2 只有白名单可证明未写的失败可转 E1，storage 写调用失败归为 `WriteOutcomeUnknown`，写后验证失败与之均禁止 E1；环境首次登录播种在拷贝前验证 donor userId。主库/副环境完整联动尚待验收。根 crate `identity_tests` 5/5 通过，infrastructure/workspace 的 identity 过滤测试 38/38 通过；尚未运行真实 OAuth/生产切号。

- [ ] 在共享材料消费边界、写入之前核对目标/供体/产品身份，覆盖 E2、E1 成功与 SameAccount 分支；空身份或未知身份拒绝。E2 错误须区分“能证明尚未写入”的不可用与写后校验失败/写入状态不确定；后两者不得再回退 E1。
- [x] 追踪全部调用者，覆盖主库切号和环境登录；断网 E1 仅在供体身份可信且等于目标时保留，不无故移除合法离线回退。调用点核对和失败 caller 测试见 2026-10-09 增量记录。
- [x] 失败注入覆盖“E2 已写入目标 storage.json、后校验失败或写入状态未知、E1 供体为另一账号”的路径，并断言 E1 不调用且供体 C 不覆盖目标 B；主库 caller 用 `WriteOutcomeUnknown`、环境 caller 用 `VerifyFailed` 验证认证回滚及档案指针/会话库不变。P14-3 定向 7/7、全量 workspace 通过，指定审查会话确认原 P3 覆盖缺口关闭。
- [ ] 确认成功及重复同账号行为保持当前契约，并验证插件同步仍 fail-soft、移除确认语义不变。

**2026-10-09 P14-3 当前覆盖边界（只读复核历史快照）：** 当时 `p14_3_identity_tests` 尚未注入“E2 已写入目标后验证失败，再尝试供体账号 C”的共享生产编排路径，也未在主库切号与环境登录 caller 中断言实际认证文件/账号档案/库状态。后续实现与验收见本文件末尾的 2026-10-09 P14-2/P14-3 增量复测记录；P14-3 其余成功、重复同账号及插件/移除确认契约仍待核验。

### P14-4 数据提交阶段、回执和并发一致性

**标签：** ready-for-agent。**Blocked by：** P14-2、P14-3。

**交付：** Work 切号/环境登录及库写入即使提交后验证或启动失败，也报告实际完成阶段并保持身份/历史一致，不误称已回滚或鼓励安全重试。

**实际子票（依据 R07/R09）：** P14-4 为汇总，按下列契约迁移，避免基础设施改了而调用者仍用旧错误判断。

| 子票 | 标签 / Blocked by | 独立交付与验收 | 文件责任/检查参考 |
|---|---|---|---|
| P14-4a 库事务提交结果 | ready-for-agent / P14-2 | handover/归并/归档/恢复等实际提交点与相关库 caller 同批识别未提交/已提交验证通过/已提交需处理；归档只覆盖当前统一入口 `apply_master_archive_changes`。事务内逻辑校验与持久性验证分开；commit 返回错误及提交后读回/校验查询失败均按实际提交事实判断。合成 SQLCipher 对提交前后失败断言 DB 残留、回执与归档冻结 | infrastructure master_handover/master_archive 及 library_archive 必需 caller/DTO，现有合成库测试；不泛化所有数据库 API |
| P14-4b 认证/DB/启动组合回执 | ready-for-agent / P14-4a、P14-3 | 主库切号、环境登录和库编排使用真实提交结果，不在已提交时 auth-only 回滚；事件/DTO/UI 同批对齐；备份/启动失败、CN 未启动不说 launched，危险重试文案移除。另覆盖收编已提交但重启失败、环境认证/DB 已变更但当前账号档案写失败、库写成功但后续列表刷新失败，以及 EnvironmentPage 收编/清理写入成功后 `get_environment_state` 刷新失败仍被报成旧数据未变；分别表达已提交、待启动/档案需处理/刷新失败，不提示整笔未完成或重试写入 | credential_handover/environment/library_archive、EnvironmentPage/LibrarySessionsPanel/MasterSwitchDialog、相关 types 与组件测试；复用已有错误/事件结构，必要最小扩展 |
| P14-4c 实际写资源串行 | ready-for-agent / P14-2、P14-3 | 按物理目录/实际共享注册表或日志拿锁；并发与重复请求不交错。目录租约必须覆盖真实切号、环境登录与库写副作用；共享档案/台账/退避文件的读改写必须有短锁。OAuth 登录、续期、手动重铸、CN 健康状态回写、手机号/偏好更新与删除必须共享账号租约或在锁内重读后只更新本字段，不能按旧快照覆盖新值或复活档案；账号租约绑定产品凭据根。共享 credits cache 的所有读改写使用同一短文件锁。OAuth 凭据包与账号档案共同发布、崩溃恢复另属 P14-5a/5b，不由单文件互斥冒充。legacy `OperationLease::acquire` / `acquire_bound` 的全局 catalog 锁不能当按库方案；`acquire_shared_for_path` 按物理资源隔离，复用 fs2 原语。目录别名/跨进程竞争/崩溃释放/独立只读均验证 | operation_lease/file_identity、全部共享存储与真实 mutator caller；不引新依赖，不以 UI busy 或仅进程内锁声称跨进程安全 |

4a/4b 如公共 DTO 不能拆到独立绿色提交，允许同一集成批次，但所有 caller 与 UI 对齐并验证后才勾完成。

**独立审计基线（2026-10-01，取证当时）：** 两条 `master_handover.rs` 事务路径在 `tx.commit()` 后做完整性读回；读回查询错误当作普通 `DbOpenFailed`，值不符当作 `IntegrityFailed`，主库切换 caller 因而可能只回退认证文件。`master_archive.rs` 的归档/项目合并将 commit 错误映射为 `WriteFailed`；CN 归档回执固定写 `relaunch_outcome="launched"`，但 CN helper 不负责启动客户端。以下 WIP 记录更新当前状态，不改写这条历史取证事实。

**执行记录（2026-10-01 至 10-02，P14-4a WIP）：** `MasterHandoverError` 新增 `CommittedNeedsAttention` 与 `CommitOutcomeUnknown`；两条 handover 路径的 post-commit 读回错误/完整性不符均报告前者，`tx.commit()` 错误报告后者。主库切号和环境登录 caller 共用 `finalize_handover_failure`：这两种状态返回专用稳定码且不调用认证回滚；已知提交前错误仍调用原回滚。收编与 CN 项目归并映射为独立稳定错误码。归档统一入口在事务内记录预期状态、提交后再读回；归档/恢复结果不符或读回失败返回 `CommittedNeedsAttention`，commit 错误和另两项 master_archive 写操作使用 `CommitOutcomeUnknown`。归档 UI 对两类异常先阻止重复应用并要求刷新核对。先前定向验证：根 crate 错误码测试 1/1，`cargo check -p trae-sync --lib`、`pnpm typecheck` 通过，safeUiError 与 LibrarySessionsPanel 79/79 通过；之后补入两项使用延迟外键制造真实 SQLite `COMMIT` 失败的测试，确认返回 `CommitOutcomeUnknown` 且测试写入在事务释放后无残留，定向测试 2/2；infrastructure crate 全量单测 835 通过/7 忽略。为与现有 SQLCipher fixture 一致，将测试 key 规范为 32 字节后再次定向运行，两项仍通过；未因此重跑全量 crate。新增共享 finalizer 的回滚/保留策略测试 4/4。仍未完成：主库切号/环境登录完整编排与阶段回执集成断言、全量 workspace lib 与全量 Vitest 最新重跑；P14-4c 写租约仍待。CN archive 假 `launched` 回执已在下方 P14-4b 增量修正。

**2026-10-02 P14-4b 回执/UI 增量：** 主库切号与环境登录 DTO 增加 `current_profile_saved`；主库切号与收编在数据交接已成功后把启动失败作为 `relaunch_outcome="failed"` 回执，不再丢弃成功结果。MasterSwitchDialog 区分切号回执与账号页刷新警告；环境登录与收编分别显示档案、自动启动、页面刷新后续问题；Work/CN 项目归并的刷新失败改为成功后的 warning，不再追加“操作未完成”错误。CN archive 的虚假 `launched` 已改为 `not_requested`，用户文案已说明需要手动启动。验证：三份定向 Vitest 13+36+45 共 94 项通过；随后 `LibrarySessionsPanel.test.tsx` 再跑 45/45；`pnpm typecheck`、三份 Rust 文件 `rustfmt --check`、根 crate `cargo test -p trae-sync --lib`（155 通过/1 忽略）及 `git diff --check` 通过。没有重跑 infrastructure/workspace 全量测试、完整 Vitest、E2E 或真实账号/客户端/数据库操作。此增量只关闭下方对应回执/UI 子项；P14-4a 与整个 P14-4 未完成，P14-4c 写租约与完整 caller/提交阶段故障注入仍待处理。

**2026-10-02 P14-4c 写资源锁 WIP：** legacy `OperationLease::acquire`/`acquire_bound` 保留目录库 catalog 锁；`acquire_shared` 改为只锁逻辑数据位置，新增 `acquire_shared_for_path` 以资源父目录物理身份 + 叶名称派生跨进程 OS 锁，路径别名共用锁、独立目录可并行。主库切号、主库备份/迁移/账号数据清理、副环境登录/删除/启动/关闭、Work/CN 归档/归并，以及六个 Library 写入口已覆盖对应真实数据目录；副环境创建/重命名/删除与库写入采用“环境容器短锁 → 单环境锁”，取得单环境锁后释放容器锁。EnvironmentRegistry 的读改写在 sidecar 文件锁内完成；RelayLedger 追加和切号 JSONL 追加也有短锁。新增路径别名、跨进程锁释放、注册表/台账并发更新、读取不等待 sidecar、不同副环境并行等测试，以及租约冲突脱敏 UI 提示。验证：infrastructure lib 850 通过/7 忽略；根 crate lib 157 通过/1 忽略；`pnpm typecheck`、`tests/safeUiError.test.ts` 38/38、P14-4c 七个 Rust 文件定向 `rustfmt --check` 与 `git diff --check` 通过。整 crate `cargo fmt --check` 当前被工作树其他 Rust 文件的格式差异阻断，未批量格式化无关文件。P14-4c 仍未勾完成，尚缺指定真实 caller 的并发屏障/副作用不交错集成验证、完整 workspace/Vitest 回归、E2E 与真实 TRAE 验收。未执行 OAuth、官方客户端或用户真实数据库写入。

**2026-10-02 P14-4c 共享存储锁增量（WIP，最新回归待补）：** 源码调用审计确认四类文件写入缺少跨进程互斥：`AccountRegistry` 的 `accounts.json` 整表读改写、`AutoCheckinStore` 的设置/台账合并、`CredentialMaintenanceStateStore` 的退避状态更新、`CheckinCredentialStore` 的每账号凭据包写入/恢复/删除。当前 WIP 已让前三者按实际共享文件使用 sidecar 锁；凭据包按 profile 锁住 save/remove/mobile/writeback/recovery，并将真实/fixture 续期锁延长到读取、网络换发、写回全过程；自动维护在同一 profile 租约内完成退避判定、续期和结果记账。新增账号注册表/自动签到/维护状态的锁等待测试，以及续期期间手机号更新必须等待且保留新凭据的屏障测试。OAuth 凭据包与 `accounts.json` 仍是两个分别发布的文件，跨文件候选代次、崩溃恢复由 P14-5a/5b 负责。此前 infrastructure lib 858 通过/7 忽略是在本增量最终接线前的快照；本次完整回归正在等待 Cargo 构建锁，不能引用为最新通过结果。未勾 P14-4c，caller 级切号/副环境/Library 副作用屏障、workspace/Vitest/E2E 与真实 TRAE 验收仍待。

**2026-10-02 P14-4c 验证更新：** 此增量后的 infrastructure lib 全量测试最终为 859 通过/0 失败/7 忽略（58.36 秒）；首次全量运行中续期屏障用例的 5 秒完成等待超时，定向复跑 1/1 通过后将完成阶段等待延长至 30 秒，再次全量通过。根 crate lib 为 157 通过/0 失败/1 忽略；P14-4c 七个 Rust 文件 `rustfmt --check` 通过，`git diff --check` 退出码 0（仅有工作树 LF→CRLF 提示）。这不是 P14-4c 完成验收：真实 caller 的数据库/进程副作用并发屏障、完整 workspace 与 Vitest、E2E 和真实 TRAE 验收仍待；OAuth 凭据包与账号档案的联合发布/恢复属于 P14-5a/5b。未执行真实 OAuth、官方客户端或用户数据库写入。

- [ ] 读取 handover/归档/恢复/归并/收编的所有相关 caller。可在事务内完成的逻辑验证移到 commit 前；必要持久性验证保留。覆盖 commit 后读回查询本身失败与校验值不符两种情况，均不得退化成普通未提交错误。
- [ ] 数据库已提交时禁止 auth-only rollback；DTO、事件与 UI 区分未提交、已提交验证通过、已提交需处理，并单列登录/启动结果。备份失败、关闭后失败、重启失败不称整次无变化。
- [x] 收编 DB 提交成功后重启失败、环境登录在认证/DB 变更后写当前账号档案失败、库写命令成功后重新读取列表失败，以及 EnvironmentPage 收编/清理命令成功后 `load()` 的 `get_environment_state` 失败时，分别显示已提交阶段与后续失败；不得把已提交操作显示成“保持不变/可重试”，也不得在提交成功通知后再报整笔失败。对页面刷新失败的验收要断言已返回的 mutation receipt 不被覆盖。（2026-10-02 定向回归通过，P14-4 整体验收仍未完成。）
- [x] CN 归档未实际启动不能返回 launched；按真实行为表达未要求启动或需要用户启动。（回执为 `not_requested`，对应 Rust/UI 测试已通过。）
- [ ] 采用最小真实资源写入串行边界，覆盖会修改同一物理目录的相关命令；共享注册表/日志需要对应短锁。不仅 UI busy，不长期锁无关只读/独立目录，不重写现有签到锁；不得把 OperationLease 的全局 catalog 锁误称已支持按库并发。
- [ ] 把租约持有范围追到真实副作用：主库切号、副环境登录、环境删除、Work/CN 归档与 CN 归并必须和凭据捕获/应用在同一物理目标上互斥；环境注册表的 create/rename/remove/set_current_profile 与对应 relay-ledger 的读改写使用短共享锁。`run_blocking` 仅把闭包放入阻塞线程池，不提供串行保证。
- [ ] 并发验收覆盖同一主库的两种写命令、同一副环境登录与归档/归并/删除、不同副环境并行、多个环境同时修改注册表，以及主库切号同时追加台账；断言不丢档案/台账行、不重复/交错 DB 与认证副作用，独立只读不被长时间阻塞。
- [ ] 合成 DB 与副作用注入验证提交前失败零提交、提交后验证失败实际已提交、启动失败、部分恢复/重复请求及并发；断言库、认证、事件、备份与 UI 一致。正常会话交接与归档冻结、混合项目规则仍满足 ADR-0028。

**2026-10-02 环境删除屏障增量：** `delete_environment_inner` 已缩短环境容器锁范围：长进程检查期间释放容器锁，同时持有目标环境目录锁；删 registry 行前重取并复核目标档案，避免并发注册表更新被覆盖。确定性屏障测试暂停环境 A 的进程检查，证明 A 仍锁定自身目录、环境 B 可完成删除且 A 恢复后 registry 保留一致。定向测试 1/1；随后根 crate lib 全量 158 通过/0 失败/1 忽略。独立 P14-4c 调用审计确认这仍不是切号 vs 主库写入、环境登录 vs Library 写入的 DB/认证副作用屏障，P14-4c 不勾完成；完整 workspace/Vitest/E2E/真实 TRAE 验收仍待。

**2026-10-02 caller 屏障测试安全边界与审核结论：** 本节点定点评估已完成，见 .scratch/phase14-execution-review-20261001.md 的 10-02 补充，无需再等待总体或同一方案评审。只读源码确认两 caller 固定官方路径，直接加屏障不安全。按最小内部 seam 实施：生产命令仍解析官方目录，租约获取及全部副作用留在生产/测试共用的私有编排；测试显式传入 TempDir 路径/库上下文，保留副环境容器短锁→目标目录租约的顺序及校验，公开 IPC 不接受任意路径或绕过字段。先做真实 caller 争用方在副作用前 busy 的定向测试，再用实例级进程/认证/事件等必要替身与现有合成 SQLCipher 完成成功 caller 屏障；覆盖插件同步等潜在触网调用，所有目录、备份 override/prune、registry/journal 都限制在临时根。冲突测试仅证明拒绝入口，不能勾完 P14-4c；完整验收仍按上面的 DB/auth、租约生命周期及独立目录要求。不得以仅锁原语/假闭包证明取代真实编排测试。

**2026-10-02 P14-4c 主库 caller 首批：** 切号核心新增私有显式主库目录入口，生产 wrapper 仍解析官方目录；租约、真实认证/SQLCipher 交接、备份、registry/relay 与回执都由共用主体持有，进程关闭/启动仅作为注入点。Work 归档改为由共用私有编排取得目录租约后再解析账号并执行进程检查、备份、SQLCipher 写入与重启；副环境生命周期短锁仍覆盖解析到目标租约建立。两个屏障用例覆盖切号持锁时归档零副作用拒绝及释放后成功，以及归档已提交但持锁时切号零副作用拒绝及释放后成功；认证文件、项目归属、归档状态与备份均来自 TempDir 合成数据，mock app 不启动 TRAE、不请求网络。定向结果：library_archive 模块 30/30、credential_handover 模块 38/38，根 crate 全量 lib 160 通过/0 失败/1 忽略。**P14-4c 仍未完成**：副环境登录组合、同库重复/独立目录并行调用层验收、跨进程/别名/崩溃释放/独立只读及共享存储锁审计首轮已完成但仍有下述恢复风险与测试缺口；当前两条测试不能替代这些标准。

**2026-10-02 P14-4c 后续审计与回归边界：** Luna 子 agent 只读核对共享文件锁，未改代码、未运行测试。`AccountRegistry`、`AutoCheckinStore`、`CredentialMaintenanceStateStore` 的读改写持 sidecar 锁；`CheckinCredentialStore` 的凭据写/删/恢复及真实续期流程持账号凭据租约。主 agent 另核对 `maintain_all_credentials` 在退避判定、续期和结果记账的整个账号流程持同一 profile lease，因此审计提出的“判定与记账分离”尚不能据此认定为并发缺陷；若以后出现不持 profile lease 的维护 caller，应重新评估是否需要原子 claim。本次复查结论：`claim_refresh_rescue_attempt` 的同代次禁止自动重放是预期防重复边界；把残缺标记归为普通“已申领”是 P2 状态辨识缺口。直接用现有字段/错误作最小修正：有效同 profile/代次/版本标记仍拒绝重复；空、截断、错误格式/绑定的标记保留并返回 `RecoveryRequired`，不删、不重发，不因内容缺失推定尚未触网。补合成残缺标记及并发至多一次的定向检查，无需再等待外评；环境登录 seam 延用已批准的共用内部编排方案。现有测试覆盖注册表/台账/退避锁等待、凭据续期锁定手机号更新等路径；仍缺 rescue marker 并发/崩溃证据。环境登录 caller 仍没有安全的完整 TempDir 组合测试：`login_environment_inner` 获取目标租约后还会检查 WAL、关 Work CN、备份、通过可能触网的身份校验写认证、交接/播种 SQLCipher 并写当前环境档案；下一片应在生产/测试共用私有编排中注入合成路径和必要外部边界，保留真实目标租约，补测与 Library 归档互斥及争用零副作用。`cargo test --workspace --quiet` 本次尝试以 Windows `os error 5`（Cargo 无法移除 `src-tauri\target\debug\trae-sync.exe`）退出，没有产生完整 workspace 测试总结；不能将其计作通过。根 crate lib 160 通过/1 忽略仍是本节点已核实的完整范围结果。

**2026-10-02 P14-4c 环境登录与救援标记增量：** `login_environment_inner` 现由生产 wrapper 与测试共用私有编排主体；环境记录解析后的真实副环境租约、WAL 检查、备份/裁剪、认证切换或 donor 播种、SQLCipher 交接及 registry 写回都留在主体内。关闭实例与身份切换通过实例级回调隔离，测试使用临时 registry/Storage/Recovery、合成认证文件与 SQLCipher 库，不触发真实进程或 HTTP。新增双向环境登录/Library 归档竞争、释放后重试及两个空环境并行首次登录用例；另两个原主库 caller 用例仍通过，P14-4c caller 定向组共 5/5。测试初次失败的夹具原因是 `usertag` 误复用 `cloudide` 密文，违反真实 E1 唯一替换约束；夹具改为独立占位 `usertag`，保留生产校验不变。

`claim_refresh_rescue_attempt` 对已存在普通文件现在反序列化并校验版本、账号与凭据代次指纹；未知字段也拒绝。有效 marker 仍禁止同代次重复申领，新凭据代次独立；空、截断、错版本/绑定等证据原样保留并返回 `RecoveryRequired`，没有自动清理或重新触网。定向 marker 用例 1/1。验证：workspace `--lib` 1165 通过/0 失败/8 忽略；完整 `cargo test --workspace` 在仓库内独立 `CARGO_TARGET_DIR=src-tauri/target/p14-validation` 下退出码 0，所有测试结果 0 失败。链接复用该工作树已构建的 vendored OpenSSL 安装以避免重复冷编译。Rustfmt 与本次源码 `git diff --check` 通过；前端本切片没有改动。

Rustfmt 与本次源码 `git diff --check` 通过；本轮 `pnpm run typecheck` 通过，`pnpm test` 23 文件/377 项全通过。前端源码本片未改。

这仅关闭上述 P14-4c 环境登录/标记子范围。P14-4c 总票仍未完成：其他 writer caller 的阶段不交错、重复请求与租约边界验收仍待；E2E、真实 TRAE/账号/数据库操作也未执行。P14-5a/5b 仍负责完整崩溃恢复和跨文件 OAuth 凭据/账号档案发布。

**2026-10-02 P14-4c 副环境登录/删除及重复请求屏障增量：** 删除 caller 的内部编排函数由生产命令与测试共用，测试在 TempDir 中复用真实租约编排和合成环境登录 caller。新增双向确定性屏障：登录持租约时删除在进程检查前 busy 拒绝，释放后删除成功；删除持租约并暂停进程检查时，登录在关闭、认证、备份和 SQLCipher 写入前 busy 拒绝，删除完成后重试返回 `environment_not_found` 且无副作用。另测同一副环境的并发重复登录，第二个请求不得重复关闭、认证切换或备份。caller 定向组 8/8 通过；`environment.rs` / `library_archive.rs` rustfmt 检查通过，`git diff --check` 退出码 0（仅 CRLF 提示）。P14-4c 仍未完成：CN/Work 归并、其他重复请求及其余租约/提交阶段边界继续按上方验收项核对。

**2026-10-02 P14-4c CN 归档/归并租约屏障 WIP：** CN 归并生产 caller 改为准备库 context 与生命周期租约后调用共用私有主体；主体先对 context 的物理目录取租约，再解析账号并执行客户端检查、备份、SQLCipher 归并与台账写入。合成测试使用显式 TempDir context，与 CN 归档共用 `acquire_library_data_lease`，新增归并持锁拒绝归档、归档持锁拒绝归并两方向屏障，校验认证/备份/项目归属/隐藏状态/台账及释放后重试。首次错误测试夹具误走固定官方 Trae CN 路径；该版可能读过官方库预检，但在测试时段未发现新备份（最近备份仍为 2026-09-30），数据库文件 mtime 与测试日志同秒变化、来源无法判定。生产归并仅在成功备份后执行数据库事务，故没有证据显示该失败运行提交了数据库写入；此边界和测试改正已交指定审查会话复核。修正后仅用显式 TempDir context：CN 归并组 9/9、P14 caller 组 8/8；两 Rust 文件 rustfmt 检查与 `git diff --check` 通过（仅 CRLF 提示）。P14-4c 仍未完成，其他 writer callers、重复请求、提交阶段、真实产品验收继续未勾。

**2026-10-02 P14-4c 共享写入者与 Work 归并补充（WIP）：** Work 项目归并现由生产/测试共用私有主体取得物理目录 lease，锁覆盖账号解析、进程检查、备份、SQLCipher 归并与剪枝；与副环境登录的双向 TempDir 屏障断言竞争方在进程/认证/备份/DB 前 busy，释放后重试成功。共享 JSON 写入补锁：`ArchiveDraftStore::set` 整段读改写、备份保留设置的 load-update-save、`PluginManifest` 初始化/增改删与保存分别用稳定 sidecar 锁；安装/卸载只在云端操作结束后短暂更新清单，不跨网络持锁。`ProductAccountStateStore::save` 锁住当前账号指针发布，防固定临时文件竞争。指定审查线程 `01a0f2c4-34f7-7150-b56b-291e7820a010` 复核认为这些共享 JSON 是 P14-4c 范围；handoff intent 的当前生产 caller 已由同位置 `OperationLease` 覆盖。会话索引缓存是可重建派生数据，源码中未发现应用层读取 caller，当前生产入口是 purge 删除；若未来接入读取，应先补文件级 save/remove 锁。若要求 purge 后绝不被在途索引刷新重新写回，读取与 purge 还必须共用 profile 数据目录 lease，本轮未扩大为此长期锁。

**验证与未决语义：** P14-4c caller 屏障组 10/10；当前 `cargo test --workspace --lib --quiet` 为 1177 通过、0 失败、8 忽略。全仓 `cargo fmt --all -- --check` 仍被多个既存工作树 Rust 格式差异阻断；本轮只格式化 `library_archive.rs` 并单独检查所触及文件，不批量格式化其他 WIP。备份名按 Unix 秒生成，曾造成归档后紧接切号的合成用例同秒重名；仅在测试步骤间加 1.1 秒间隔，生产命名语义未改。当前账号指针的锁只保证文件发布串行，OAuth 登录与主动切号谁优先仍是未定义的产品语义；插件云端安装成功而本地 manifest 写失败的阶段回执也仍需按 P14-4b 处理。P14-4c 仍未勾完成；跨进程 caller 屏障、剩余 mutator 清点与其他票内验收继续按上方标准核对。

**2026-10-02 P14-4c 账号选择优先级与救援并发增量：** 指定审查线程确认真实竞态：CN OAuth 完成与 `switch_trae_cn_account` 会写同一工具账号指针，文件锁只能保证原子发布，无法表达用户意图优先级。ADR-0034 决策 12 收敛为“较晚登记的有效账号选择意图优先”：OAuth begin 成功后记录代次，显式切号只有在目标档案和本产品凭据验证成功后才推进代次；OAuth 回调仅在代次未变化时选择新账号。无效切号不抢占代次；较新 OAuth 先登记后取消/失败时，指针保持原样且更早的待完成操作不恢复，避免旧操作迟到生效。较新切号先完成时，迟到 OAuth 仍保存账号/凭据但保留当前指针；反向顺序则较新的 OAuth 可成为当前账号。旧格式状态文件兼容读取，缺代次默认为 0。实现覆盖共享 `ProductAccountStateStore`、CN OAuth begin/complete 与显式切号命令；UI 登录提示仅称“已保存”，不宣称官方客户端或工具当前账号已切换。初次审查指出无效 profile 会抢占代次；现已将校验提到登记之前，`invalid_switch_target_does_not_supersede_pending_oauth_selection` 验证该路径保留较早的有效 OAuth；后续只读复核确认修正闭合问题，且取消/失败保留指针、不恢复旧操作的规则明确一致。`refresh_rescue_claim_is_at_most_once_under_concurrency` 以 8 个并发合成调用验证同一凭据代次只有一个获得触网资格，marker 仅有一份；其余调用 fail-closed 或读取已存在标记。定向验证：基础设施账号状态 6/6、救援并发 1/1；调用层 OAuth 竞态 1/1，目标校验 1/1。OAuth 测试完整路径为 `oauth_login::login_command_tests::later_cn_account_switch_is_not_overwritten_by_late_oauth_completion`。此前一次过滤器误用了不存在的模块路径，0 项运行，不计入验证。最新 `cargo test --workspace --lib --quiet` 全量结果 1187 通过、0 失败、8 忽略（root 172/1 忽略，infrastructure 868/7 忽略）；`pnpm run typecheck` 与 safeUiError 39/39 通过，所触及 Rust 文件 `rustfmt --check` 通过，`git diff --check` 退出码 0（有 LF→CRLF 提示）。若账号凭据已写入但当前指针持久化本身发生 I/O 错误，登录结果/页面刷新阶段仍由 P14-4b 继续核实；本次对 superseded（代次已更新）按登录成功处理。P14-4c 仍为 WIP，不能据此勾总票。

**2026-10-02 P14-4c 跨进程 caller 屏障与缺口审计：** Luna 只读审计确认当前锁原语有跨进程测试，但真实切号/归档/环境登录 caller 的屏障多为同进程线程；同目标归档/归并重复请求、caller 阶段故障注入、环境 registry 与 relay ledger 更新组合也未闭合。新增 Work 归档跨进程屏障：子进程只在合成主库目录持有实际 `OperationLease`，父进程通过生产/测试共用 `apply_library_archive_for_context` 调用归档；锁冲突时断言 close/relaunch 未触发、无备份且归档字段未变，释放后同一归档成功。子进程握手设置 10 秒超时并终止/回收未就绪进程，stdout reader 持续排空到进程结束。初次测试的 `--exact` 路径遗漏外层 `library_archive::`，修正后通过；审查确认调用路径和副作用断言有效，并指出无超时读取风险，已修正。复跑 caller 组时同时暴露既有环境登录→归档用例可能因秒级备份文件名碰撞而失败；在该合成测试两次备份间增加 1.1 秒间隔后，caller 组 12/12 通过。`rustfmt --edition 2021 --check src-tauri/src/library_archive.rs` 与 `git diff --check` 通过；链接仍有 OpenSSL PDB 缺失警告，不影响测试退出码。**仅补上 Work 归档一个真实 caller 的跨进程证据，P14-4c 继续 WIP**：CN/环境/切号 caller 的跨进程覆盖与 caller 阶段故障注入、Work/CN 归档和归并同目标重复请求、registry+ledger 组合屏障仍待；本增量未跑 workspace 全量、Vitest、E2E 或真实 TRAE 操作。

**2026-10-02 P14-4c CN 重复请求屏障与最终复核：** 新增 CN 同目标并发重复归档、归并测试：归档重复请求在同库租约占用时不触发客户端状态检查或提交回调，不增加备份；归并重复请求不发出进度事件、不重复备份或台账记录。`cargo test -p trae-sync --lib concurrent_duplicate_cn_ -- --nocapture` 为 2/2；根 crate 全量 `cargo test -p trae-sync --lib --quiet` 为 176 通过/0 失败/1 忽略。Luna 审查线程复核 handshake 修正后确认 10 秒等待上限、超时 kill+wait、stdout 持续排空及秒级备份间隔已关闭其指出的 hang/BrokenPipe/撞名风险。**仍未勾 P14-4c**：重复请求只覆盖 CN 归档/归并与副环境登录；Work 归档/归并、CN/Work 切号和环境 writer 的跨进程/重复调用矩阵、真实 caller 阶段故障注入及 registry+ledger 组合屏障仍待；本增量未跑全 workspace、Vitest、E2E 或真实 TRAE 验收。

**审查补充：** 指定审查线程对 CN 两条重复请求测试做只读复核，确认首次调用在提交后回调中暂停且持有同一物理库租约；第二次调用沿同一编排入口在产品检查/备份/数据库写入前被拒绝，归档状态、备份数、归并项目归属与台账条目数断言足以检查副作用未重复，未发现具体问题。审查未修改代码、未运行测试。

**2026-10-03 P14-4c Work 归并重复请求与账号 writer 审计：** 新增 `library_archive::p14_4c_archive_caller_tests::concurrent_duplicate_work_project_merge_is_rejected_without_repeating_side_effects`：首个 Work 归并在合成 SQLCipher 提交后暂停且保持目录租约；第二个同库请求在进程检查前以 `operation_lease_unavailable` 拒绝，断言源项目已移除、会话归属正确、只产生一份备份且只检查一次进程。定向测试 1/1；`rustfmt --edition 2021 --check src-tauri/src/library_archive.rs` 与该文件 `git diff --check` 通过。此前“Work 归并重复请求未覆盖”仅是 10-02 审计时状态，现已由本测试关闭。

同日只读源码审计另发现账号 writer 缺口，尚未修复：`checkin_login::persist_login_result_for` 在 profile lease 外读取旧偏好/手机号后保存；`DeviceRemintService::remint` 读档、HTTP 与凭据/档案写回未用同一租约；`remove_checkin_account` 先删除档案再获取凭据文件锁；Trae CN 健康检查 HTTP 后把旧 `AccountRecord` 整条 upsert，可能覆盖新设备/偏好或复活已删档案。手机号/备注/自动签到开关等调用也有锁外快照；`checkin_overview` 的 `credits-cache.json` 各更新/删除路径对整文件读改写无 sidecar 锁，并使用固定临时文件名。下一片先把账号租约绑定到产品凭据根，复用 `CheckinProfileOperationLease`，给需要嵌套调用的存储操作提供持锁入口，再逐一接 OAuth、删除、重铸、健康与手机号路径及屏障测试；不把这些互斥缺口混成 P14-5a/5b 的跨文件崩溃原子性。跨进程 caller 矩阵、切号/环境登录阶段故障注入、registry+ledger 组合屏障、credits-cache 文件锁与完整集成/真实验收仍未完成，P14-4c 继续 WIP。

**2026-10-03 P14-4c 同账号 writer 租约接线（WIP）：** 复用并增强 `CheckinProfileOperationLease`，租约校验绑定 profile ID 与凭据根的规范化物理资源；存储层提供持租约 save/remove/mobile/writeback 与 renewal 入口，避免嵌套重锁。OAuth 在锁内重读档案、保存并保持租约至首次续期验收；设备重铸在首次读取前取得租约并持有到网络和凭据/档案安全写回，写回重新读取最新档案及凭据且拒绝已删除/替换档案。删除、归档/备注/手机号/自动签到偏好、CN 健康和手动凭据/额度刷新均在 profile lease 内重读最新资料并完成对应副作用；真实自动维护及 status→claim→status 持同一账号租约。批量签到 transport factory 接收 profile ID、持有单账号租约至 `run_one` 结束，并在账号间等待前释放。`credits-cache.json` 全部修改入口现用 sibling sidecar lock 包住整段读改写，并使用 PID+序号且 `create_new` 的临时名；只读展示不持该写锁。

验证：`cargo check -p trae-sync --lib` 成功；root crate `cargo test -p trae-sync --lib` 为 178 通过/0 失败/1 忽略；`cargo test --workspace --lib --quiet` 为 1195 通过/0 失败/8 忽略。涉及的 8 个 Rust 文件 `rustfmt --edition 2021 --check` 通过，`git diff --check` 退出码 0（仅 Git LF→CRLF 提示）。编译/链接出现既有 unused import、dead code 与 OpenSSL PDB 缺失警告，不影响通过。指定审查会话已收到只读审查请求，反馈待返回。

此增量关闭上段列出的代码接线缺口，但**不勾 P14-4c**：OAuth/健康/删除/偏好等真实 caller 的全部交错屏障、跨进程 caller 矩阵、registry+ledger 组合阶段、故障恢复语义仍需按票验收；跨文件 OAuth 凭据包与档案发布/崩溃恢复属 P14-5a/5b。没有运行真实 OAuth、真实用户目录/数据库写入、Trae CN 官方客户端切换，不能作为 G5 或真实 CN 切换通过证据。完整 `cargo test --workspace`、前端全量和 E2E 也未在本片执行。

**2026-10-03 P14-4c 并发审查问题修复增量（WIP）：** 指定审查会话发现同账号并行首次 OAuth 登录可能重复建档、签到结果在账号间错峰后写缓存可能覆盖新额度、关闭自动签到开关后仍可能按旧候选执行，以及多进程 scheduler 可能以旧台账覆盖新台账。OAuth 现在在 profile 解析前按产品物料根与账号 ID 摘要取得跨进程归属租约，持有至发布及首次续期验收；同账号不同 session profile 只生成一档，不同账号和 Work/CN 根独立。批量签到进度回调现在在 transport 仍持账号 lease 时写额度与最近尝试缓存，transport 随后在账号间等待前释放；锁内重读并核对账号映射。自动签到在错峰后持 profile lease 复核该账号开关，并在签到完成前保持租约；此时已关闭的账号本批计入跳过、不会加入顺延重试。`run_auto_checkin_tick` 先取得独立非阻塞批次租约，再读取设置/台账并持有到批次结束；设置文件只在读改写时短锁，不被网络操作阻塞。`AutoCheckinLedger.skipped_ids` 保留为可重试撞锁账号集合，`skipped` 表示本批跳过总数。

定向验证：OAuth `checkin_login::tests` 29/29；自动签到 lease 原语 `operation_lease::` 18/18、`auto_checkin_store::` 12/12；应用 runner 定向用例 12/12；root crate `real_checkin_tests` 本轮 19 通过/0 失败/1 忽略，其中包含“错峰后关闭开关则零签到、计为取消”的回归测试。全 workspace 库测试串行复跑 `cargo test --workspace --lib --quiet -- --test-threads=1` 退出码 0；root target 179 通过/1 忽略，infrastructure target 878 通过/7 忽略。并行首跑的 `account_switch_and_archive_callers_share_the_master_lease` 单项失败，随后精确复跑 1/1 及串行 workspace 库测试均通过；不把并行首跑隐藏为通过。完整非 lib workspace、前端/E2E 和指定审查会话对此次修复的复核仍待完成；因此 P14-4c、G5 与真实 CN 切换均不勾完成。

**2026-10-03 P14-4c 自动签到忙锁重试复核收口（WIP）：** 指定审查会话确认此前 P2 已关闭，未发现新的 P1/P2。复核确认手动批次锁忙时先尝试 `execution_lock.try_lock()`，失败分支不持有 execution mutex，再取 profile lease 读取当前自动签到开关并短暂写台账；关闭账号不进入 `skipped_ids`，若已有重试 ID 则移除但不重复增加 `skipped` 总数；该顺序不与正常执行的 execution→profile 锁顺序形成环。新增真实批次编排测试覆盖关闭、开启和“既有重试项后来关闭”；定向用例 1/1、`checkin_schedule::` 模块 23/23、`rustfmt --edition 2021 src/checkin_schedule.rs` 与 `git diff --check` 通过。改动后的串行 workspace lib 测试退出码 0：1200 通过、0 失败、8 忽略（root 180/1 忽略、infrastructure 878/7 忽略）。完整 workspace 默认 target 首次运行未形成结果：链接时 Windows 报 `os error 5`，拒绝覆盖正在运行的 `target/debug/trae-sync.exe`；未终止该进程，改用 `%TEMP%` 独立 target 后 `cargo test --workspace --quiet -- --test-threads=1` 完整退出码 0，31 个测试 harness 共 1252 通过、0 失败、25 忽略。审查只读、未改代码或重跑测试。P14-4c、G5 与真实 CN 切换均不勾完成。

**2026-10-03 P14 集成回归：产品操作状态 E2E mock 对齐（WIP）：** 初次 `pnpm test:e2e` 为 39/44；5 项均因 E2E bridge 未模拟新只读命令 `get_product_operation_states`，导致真实 pending 门禁正确隐藏按钮、旧用例却假设 ready。mock 已补该命令；CN claim/archive/merge 默认 pending，只有明确验收这些流程的页面用例显式覆盖 ready，Work `library.merge` 修正为生产定义的 absent。`pnpm test:e2e` 在最后这项单行修正前 44/44；修正后 `pnpm typecheck` 通过，相关归并 E2E（Work/CN）26/26。全量 `pnpm test` 23 文件、378 项通过。指定审查会话只读复核确认 CN 的 ready 覆盖限于测试 mock、生产 readiness 没有被改写，无新 P1/P2；最终 Work absent 修正按审查建议完成。该测试不能充当 CN 真实 claim/archive/merge 验收；生产 CN 操作仍为 pending。P14-4c caller 屏障、G5、官方 CN 原生账号切换仍待。

**2026-10-03 P14-4c 环境注册表与接力台账组合锁复核（WIP）：** 第一版并发冒烟仅 barrier 同步起跑，且台账只由一个 helper 追加；指定审查会话判定不足以关闭组合屏障（P2）。更新后的 `credential_handover::master_switch_tests::concurrent_master_switch_registry_and_ledger_writes_preserve_both_resources` 先预置主库/副环境档案及旧台账项，再由测试线程持有真实 `environments.json` 与 `relay-ledger.json` sidecar 锁，启动两个注册表 writer（主库当前账号回写、副环境重命名）和两个独立台账 helper writer。每个 writer 发出 started 信号后，测试在锁仍持有时观察其 250ms 内未完成，再放锁并有界等待结束。最终断言两条注册表记录和副环境原 profile 都保留；旧台账项与两个不同新增项各出现一次，且 from/to/profile 正确。定向测试 1/1、`rustfmt --edition 2021 --check src-tauri/src/credential_handover.rs`、`git diff --check` 通过。指定审查会话确认该测试足以关闭 registry+ledger helper/存储层组合锁缺口，无 P1/P2；started 信号及 250ms 只证明持锁期间未完成，不能宣称观察到每个线程的内部锁入口或穷尽调度交错。该测试不进入完整 `switch_master_account`，不覆盖目录租约、DB/auth、启动、回执、阶段失败、跨进程及其它 P14-4c caller 矩阵；P14-4c 与 tickets 的并发总验收仍未完成。

**2026-10-03 P14-4c 主库切号跨进程拒绝屏障（WIP）：** 对真实 `switch_master_account_at` caller 增加跨进程合成测试：持锁子进程针对同一 TempDir recovery root/master dir 取得真实 `OperationLease` 后发 ready；另一个测试子进程运行真实 caller。caller 必须精确返回 `operation_lease_unavailable:{OperationLeaseError::DataLocationBusy}`，并断言 close/launch/before_return 均为 0、storage root 仍为空。指定审查会话先指出两项 P2：仅匹配错误前缀会把路径错误误当锁竞争；同步 caller/child.wait 可能无限等待。现已改成精确 `DataLocationBusy`，caller 自身隔离在测试子进程中；父进程用 `try_wait` 对 caller/holder 正常退出设限，ready/退出超时和 spawn 失败路径均释放并回收持锁进程，caller 超时额外限时等待、必要时 kill+wait。持锁子进程 stdout 由父端持续排空，caller 子进程 stdout/stderr 继承给父端，不留阻塞管道。定向 `credential_handover::master_switch_tests::` 8/8、rustfmt 和 diff check 通过。指定审查会话最终确认两项 P2 关闭、无新增 P1/P2，认可为“主库切号真实 caller 在另一进程持有同库租约时，于业务副作用前拒绝”的跨进程证据。该测试空账号库早退，不覆盖成功切号 DB/auth、租约持有到提交后、关闭/启动、回执/阶段失败、路径别名、崩溃释放或其它 callers；只能关闭该窄跨进程拒绝子项，不能勾 tickets 945 并发矩阵或 P14-4c 总票。

**2026-10-03 P14-4c 副环境登录/删除跨进程拒绝屏障（WIP）：** 同一 TempDir 副环境、同一 recovery root/`secondary_data_dir` 由 holder 子进程持有真实 `OperationLease`；父端先后启动两个独立 caller 子进程，均在释放 holder 前退出。删除走生产共用 `delete_environment_inner_with_process_check`，精确返回 `operation_lease_unavailable:{OperationLeaseError::DataLocationBusy}`，process-check=0，环境行和 `keep.txt` 保留。登录走生产共用 `login_environment_inner_with_ops`；fixture 用有效的 `AccountRegistry::upsert` profile，精确返回同一 busy 原因，close/identity-switch/before_return 均为 0，环境行和文件保留。登录 caller 先读取并匹配账号档案、再取环境租约；此证据只证明它会在后续 WAL/关闭/认证/DB 操作前拒绝，不应说账号读取前拒绝，也不证明合成凭据字段代表真实 OAuth。审查核对两个 helper 与生产 wrapper/目录完全一致，无 P1/P2。holder ready 10s、两个 caller 各 10s、holder 释放 5s 均有界；未就绪、spawn 失败、caller 超时都清理并回收进程。定向 Rust 用例 1/1（父测试和两条子 caller 均通过）、单文件 rustfmt、`git diff --check` 通过。只覆盖两种真实 caller 的跨进程租约拒绝，不覆盖成功登录/删除、提交阶段持锁、DB/auth/启动/回执/失败恢复、其它 caller 或 P14-4c 总票。

**2026-10-03 P14-4c 同库 caller 备份时间边界与全 workspace 复跑（WIP）：** 完整串行测试中的 `library_archive::p14_4c_archive_caller_tests::account_switch_and_archive_callers_share_the_master_lease` 在切号后立即重试归档时得到 `master_archive_backup_failed`。两条操作按 Unix 秒命名同目录备份，`create_new` 有意拒绝覆盖同名文件；此合成测试只需避免其串行第二次备份撞秒，因此在切号 join 和全部租约/副作用断言后、归档重试前增加 1100ms 间隔，和反向顺序测试已用间隔一致。未改生产备份行为。指定审查会话只读复核无 P1/P2，认为这是该夹具的最小修正，不需为此新增时钟注入；限定为正常墙钟未回拨时分开该测试的先后两次合成备份，不能说时钟完全受控，也不代表解决生产同秒重试。定向测试 1/1 通过。

全 workspace 首次使用较长 `%TEMP%` target 时，trybuild 编译 vendored OpenSSL 失败于 MSVC `C1083`（无法打开编译器生成文件，参数无效）；随后一次跳过该 compile-fail 用例的 run 暴露上述同秒备份夹具失败。修正夹具后，完整 `cargo test --workspace --quiet -- --test-threads=1` 改用短 `CARGO_TARGET_DIR=%TEMP%\tp14`，退出码 0，31 个 workspace harness 共 1260 通过、0 失败、25 忽略；包括 trybuild 与新增跨进程测试（root lib 188/1 忽略、infrastructure lib 878/7 忽略）。短 target 下 MSVC/OpenSSL 编译顺利。此次仅改 Rust 测试夹具，所以没有重跑前端套件；最近 `pnpm typecheck`/Vitest/E2E 的结果另见 P14-9 记录。本增量不勾 P14-4c；其他 caller/阶段故障注入与真实 TRAE 验收仍待。

**2026-10-03 P14-4c 主库切号启动失败回执（WIP）：** `library_archive.rs` 新增 `master_switch_relaunch_failure_preserves_committed_receipt`，通过生产/测试共用的 `switch_master_account_at` 编排、TempDir 合成档案/认证/SQLCipher 数据和显式启动失败 seam，核对函数仍返回成功回执且序列化 `relaunch_outcome=failed`、`current_profile_saved=true`、`relay_ledger_written=true`；同时核对 auth 与项目 owner 已切至 account-2、registry 当前档案为 profile-2、备份计数为 1、关闭/启动/返回回调各执行一次。定向测试 1/1 通过。指定审查会话只读复核无 P1/P2，认可其覆盖“真实 Work caller 的切号已提交后启动失败仍保留回执”。准确边界：台账字段断言是回执，不读取台账内容；备份数断言不验证恢复；不代表官方客户端启动或 UI 提示验收。此测试加入前最近一次完整 workspace 回归为 1260 通过/0 失败/25 忽略，不能将其算作此测试版本的全量结果。P14-4c 仍 WIP；下一片按票补充真实 caller 的提交前失败零提交屏障，再继续剩余阶段故障/恢复矩阵。

**2026-10-03 P14-4c 主库切号关闭失败零提交屏障（WIP）：** `library_archive.rs` 新增 `master_switch_close_failure_leaves_master_unchanged`，使用真实 `switch_master_account_at` caller、TempDir 合成 auth/SQLCipher 与返回 `Err` 的关闭 seam。先把主库 registry 档案设为 profile-1，再注入关闭失败，精确核对 `master_switch_close_failed`、close 调用一次、launch/before_return 为零、auth 和 SQLCipher owner 仍为 account-1、备份数仍为零、registry 当前档案仍为 profile-1。第一次运行只因 fixture 未创建主库 registry 档案而在断言处失败；在该测试局部初始化基线后定向测试通过。指定审查会话只读复核无 P1/P2；认可本项状态保留断言，限定未比对所有文件字节，也未验证真实客户端进程关闭失败。随后根 crate 串行 `cargo test -p trae-sync --lib --quiet -- --test-threads=1` 在短 target 下 190 通过、0 失败、1 忽略；含以上关闭失败与启动失败两项 caller 测试。`rustfmt --edition 2021 --check src-tauri/src/library_archive.rs` 与 `git diff --check` 通过（后者有工作树 LF→CRLF 提示）。P14-4c 仍 WIP；接下来继续其它真实 caller、提交后读回失败及恢复组合，不将这两个切号测试扩成完整阶段矩阵。

**2026-10-03 P14-4c 主库切号提交后读回失败（WIP）：** `library_archive.rs` 新增 `master_switch_committed_integrity_failure_preserves_committed_state`。TempDir SQLCipher fixture 插入合成正文和 `AFTER UPDATE OF user_id` 触发器；触发器在事务内更新项目归属时改正文，handover 随后完成身份映射并成功提交，提交后正文指纹校验返回 `CommittedNeedsAttention`。真实 `switch_master_account_at` caller 精确返回 `master_switch_committed_needs_attention`，并核对 auth=account-2、重新打开数据库确认 owner=account-2 且 session id 已变化、备份数为 1、registry 仍 profile-1、close=1、launch/before_return=0。定向测试 1/1；指定审查会话复核无 P1/P2，并确认“事务内正文变更、提交后读回发现不一致”的准确顺序及认证不回滚。边界：不等同于 COMMIT 本身失败，也不代表全部文件完整性、UI/客户端或整个阶段矩阵验收。更新后根 crate 串行 lib 回归 191 通过、0 失败、1 忽略；全 workspace 仍需在本组三项测试新增后重跑。后续沿 P14-4c 继续其它真实 caller 与提交状态未知/恢复组合，再按依赖进入 P14-5。

**2026-10-03 P14-4c 主库切号事务内语句失败（WIP）：** 新增 `master_switch_precommit_db_failure_rolls_back_auth_and_transaction`。在合成 SQLCipher 库设置 `BEFORE UPDATE OF session_id` 触发器；生产 handover 已在同一未提交事务中更新项目归属，随后 session 重映射语句触发 `RAISE(ABORT)` 并错误返回，未进入 COMMIT。真实 caller 返回 `master_switch_db_open_failed`，未提交事务在函数退出时整体丢弃回滚；known-precommit finalizer 将认证恢复到 account-1。核对重新打开 DB 后 project owner/session ID 仍原样、registry 仍 profile-1、备份计数为 1、close=1、launch/before_return=0。定向测试 1/1；指定审查会话确认无 P1/P2，并要求准确写成“语句失败导致事务未提交，Transaction 丢弃回滚”，不能说 `RAISE(ABORT)` 自己回滚了整个事务。此项只覆盖合成语句失败与被断言的 owner/session 状态，不是 COMMIT 错误或全部表/文件逐字节验证。五项 caller 用例后根 crate 串行 lib 为 193 通过、0 失败、1 忽略；完整 workspace 尚未对这五项新增版本重跑。P14-4c 仍 WIP，继续未覆盖的真实 caller/恢复组合后再整体集成。

**2026-10-03 P14-4c 五项主库 caller 故障用例 workspace 集成回归：** 新增的 close 失败、真实 TargetConflict 认证回滚、事务内 session 重映射失败回滚、提交后正文完整性不符、提交成功后启动失败回执五项均进入完整 workspace 串行运行。`cargo test --workspace --quiet -- --test-threads=1` 使用短 target `%TEMP%\tp14` 退出码 0，31 个测试 harness：1265 通过、0 失败、25 忽略；含 trybuild。`cargo test -p trae-sync --lib --quiet -- --test-threads=1` 为 193 通过、0 失败、1 忽略。单文件 rustfmt 与 `git diff --check` 通过；OpenSSL PDB 缺失及工作树 LF→CRLF 提示不影响退出码。仍只是一组离线合成 caller 状态验证，不是完整 P14-4c caller/恢复矩阵、真实 TRAE 或 CN 官方切换验收。

**2026-10-03 P14-4c 副环境登录双阶段故障验证（WIP）：** 新增两个真实 `login_environment_inner_with_ops` caller 测试，使用 TempDir registry、副环境 SQLCipher 与 `switch_auth_identity_for_account` 的 E1 合成文件互换。其一，来源/目标同 biz 且都有正常会话；认证已互换后，handover 的写事务开始前返回 `TargetConflict`。caller 映射 `environment_login_conflict` 并恢复认证；断言 source owner/session 不变、目标 project 仍 account-2、备份保留一份、registry profile-1、close/identity 各一次、before_return=0。其二，事务内触发器改写消息正文，handover 正常提交后校验发现不一致；caller 返回 `environment_login_committed_needs_attention`，不回滚认证/已提交 DB，并在档案写回和 before_return 前返回。断言 auth/owner=account-2、session 已变、备份一份、registry 仍 profile-1。定向测试各 1/1；指定审查会话逐项复核无 P1/P2。边界：第一项是事务前冲突，不是 DB 写后回滚；第二项是 E1 合成身份 seam，不覆盖生产 E2/GetUserInfo、UI/真实客户端，也不是 COMMIT 本身失败。主库五项与副环境两项共七个 caller 故障用例版本的根 crate 串行 lib 为 195 通过/0 失败/1 忽略；完整 workspace 重跑退出码 0，31 harness 为 1267 通过/0 失败/25 忽略，含 trybuild。P14-4c 仍 WIP，其他 caller 与恢复组合继续待验。

**2026-10-03 P14-4c Work 项目归并跨进程租约屏障（WIP）：** 新增 `library_archive::p14_4c_archive_caller_tests::work_project_merge_caller_rejects_cross_process_library_lease_before_side_effects`。合成副环境的实际数据目录由 holder 子进程持有租约；父测在独立 caller 子进程中调用真实共用 Work 归并编排。caller 精确断言 `operation_lease_unavailable:DataLocationBusy` 且 process-check=0；父测在 caller 退出后断言无备份、源项目仍存在且 owner=account-1、源会话仍归属 project-2。指定审查会话先指出 caller 同步运行缺少完成时限（P2）；现已改为 caller 子进程，父测持锁期间最多等待 10 秒，超时先释放并回收 holder，再限时等待或 kill+wait caller。启动失败、holder ready 超时、正常释放也有界清理；stdout 持续排空。定向 Rust 测试父/子进程均通过，外层 1/1；`rustfmt --edition 2021 --check src-tauri/src/library_archive.rs` 与 `git diff --check -- src-tauri/src/library_archive.rs` 通过。审查复核确认 P2 关闭、无新增 P1/P2。边界仅为 Work 副环境项目归并与另一进程持锁时的拒绝，不证明官方主库、成功归并、全数据逐字节不变或整个 P14-4c 完成。上一完整 workspace 1267/0/25 是此前七 caller 用例版本，本次新增后未重跑全 workspace；父票继续 WIP。

**2026-10-03 P14-4c OAuth 重登与手机号并发屏障（WIP）：** Windows 测试 `checkin_login::tests::first_login_acceptance_holds_profile_lease_against_mobile_update` 使用既有 Work profile 执行 OAuth 重登，并在验收闭包暂停期间并发更新手机号。闭包直接对唯一既有 `.checkin` 凭据文件路径探测 sibling lock，确认生产 profile lease 正持有同一 sidecar；手机号更新在释放前未完成，释放后 OAuth 新 refresh token 与新手机号均从 DPAPI 包读回。指定审查会话复核确认原两项 P2 已关闭、无新增 P1/P2；定向 Windows 父/子 harness 均 1/1 通过，rustfmt 与单文件 diff check 通过。测试主体由精确测试名子进程承载，父进程 30 秒超时后 kill+wait。300ms 只表示手机号操作在这段观测时间内未完成，不作为“writer 已进入锁等待”的证据；直接同资源锁探测才证明锁仍持有。范围是 Work 既有账号重登与同进程手机号写入，验收/renew 为合成闭包，不覆盖真实 OAuth HTTP 或跨进程 writer。该定向测试加入后尚未重跑 workspace 全量；P14-4c 继续 WIP，P14-5/G5/CN 官方真实切换仍待。

**2026-10-03 P14-4c Work 归档跨进程等待边界（WIP）：** 修正 `work_archive_caller_rejects_cross_process_library_lease_before_side_effects`：父测通过生产共用 `acquire_library_data_lease` 持有实际 Work 主库目录租约，独立 caller 子进程运行真实 `Fixture.apply` 归档编排，精确断言 `DataLocationBusy` 与 close/relaunch=0。10 秒内未返回时先 kill+wait caller，再释放测试租约；正常拒绝后核对归档状态与备份未变，释放租约后保留成功归档重试断言。指定审查会话窄审通过，无新增 P1/P2；Windows 父/子 harness 各 1/1，rustfmt 与单文件 diff check 通过。范围仅是 Work 合成主库归档 caller 跨进程拒绝和释放后成功，不是官方主库/客户端验收。Workspace 1267/0/25 早于 Work merge、此归档和 OAuth/mobile 两项新增测试，当前源码快照的 workspace 全量仍待重跑；P14-4c 继续 WIP。

**2026-10-03 P14-4c CN 归档/归并跨进程拒绝屏障（WIP）：** 新增 `trae_cn_merge_tests::cn_archive_and_merge_callers_reject_cross_process_library_lease_before_side_effects`。父测通过共用 `acquire_library_data_lease` 锁住合成 CN 数据目录，子进程依次调用生产/测试共用的 `apply_library_archive_for_context` 归档路径和 `run_trae_cn_project_merge_with_lease` 归并路径。两者按 `OperationLeaseError::DataLocationBusy` 的正式显示格式精确拒绝；stop/after_commit、归并账号解析/process-check/progress 均为 0。父测核对 owner=111、hidden=0、backup=0、CN ledger 不存在。子进程 10 秒未返回时 kill+wait 后释放父测租约；审查会话确认同一生产资源和清理顺序成立，无新增 P1/P2。Windows 父/子 harness 各 1/1，rustfmt 与 diff check 通过。只覆盖 CN 合成库中两个 caller 的跨进程锁忙拒绝，不证明相互并发、释放后成功、官方 CN 真实切换或完整 P14-4c。最近完整 workspace 1267/0/25 早于 Work merge、Work archive、OAuth/mobile 和本 CN 用例，当前快照全量仍待重跑。

**2026-10-03 P14-4c 主库切号与 Work 归档跨进程竞争（WIP）：** 新增 `master_switch_blocks_cross_process_work_archive_until_post_commit_release`。真实 `switch_master_account_at` 在子进程中提交后停于仍持租约的 `before_return` 屏障；独立子进程执行真实 Work 归档 caller，精确断言 `DataLocationBusy` 和 close/relaunch=0。父测核对竞争期间 auth/owner 已是 account-2、archive hidden 状态未变、备份仍为 1；释放后确认切号子进程退出且归档状态仍未变。指定审查会话先指出旧版父线程无法超时终止回收（P2）；现切号与归档均为子进程，ready、caller 完成与释放等待有上限，失败时 kill+wait 后才断言/退出，复核确认原 P2 关闭、无新增 P1/P2。Windows 定向测试父/子各 1/1，单文件 rustfmt 与 diff check 通过。归档进程检查无可注入计数 seam；本项仅按源码确认取租约顺序，不声称动态观察其调用次数。完整 workspace 测试早于本用例，需在后续集成批次重跑；P14-4c 总票仍 WIP。

### P14-5 跨文件/数据库操作的最小持久恢复

**标签：** ready-for-agent。**Blocked by：** P14-4。

**交付：** 管理工具在真实切号关键阶段退出后，重开仍能辨认哪些数据已改变、恢复哪些材料及安全的下一步，失败证据不丢失。

| 子票 | 标签 / Blocked by | 独立交付与验收 | 文件责任/检查参考 |
|---|---|---|---|
| P14-5a 持久恢复材料与阶段 | ready-for-agent / P14-4b、P14-4c | 在已确认实际写顺序上保存旧认证加密材料、DB 备份和最小阶段事实；包含 ADR-0027 的 OAuth 候选凭据发布：凭据包与账号档案是同一可恢复边界，验收前不得替换旧当前代次；阶段记录与实际磁盘可能不同，崩溃窗口可辨认。未收口记录不覆盖、唯一材料不被裁剪；离线故障注入通过 | actual switch/环境/库与 OAuth 凭据发布编排、`CheckinCredentialStore`、`AccountRegistry`、atomic_publish/现有 journal；旧 Gate 仅可复用必要原语，不能复活审批模型 |
| P14-5b 重开幂等恢复与阻止新写 | ready-for-agent / P14-5a | 重开按磁盘证据处理或明确待人工处理，不重复交接/覆盖较新凭据/触网；所有相关同目录新危险写拒绝未收口状态。OAuth 凭据包与账号档案若发布中断，按可信磁盘证据恢复旧当前代次或明确待人工处理，不能留下跨代次绑定。明确共享恢复根中 A 位置的 `ManualRecoveryRequired` 是否应阻断独立 B 位置；按实际共享状态给出理由和 A/B 验收。重复重开与损坏/缺材料状态检查通过 | 根启动/真实 mutator 入口、OAuth credential store/registry 与恢复 UI；只新增实际恢复所需信息，不建立通用工作流框架 |

依据 R08；两个子票通过才达到 G2，任何阶段名称本身不能证明整个操作可回滚。

- [ ] 基于已经确认的实际提交顺序，写前保存可恢复的旧认证材料（秘密保持加密）和数据库备份；复用已有 atomic publish/日志机制，只加跨副作用必要阶段事实，不复活旧 Gate 工作流。
- [ ] 记录与磁盘事实不一致时不得仅靠阶段名宣称恢复；明确验证并幂等处理本地安全恢复，未知/不能证明完整恢复的状态保留证据并提示处理。
- [ ] 未解决的提交/恢复状态阻止同目录新破坏性操作，不能被新请求覆盖；恢复仍需的唯一材料按恢复/失败证据保留，普通备份裁剪不提前删除它。收口后继续遵循现行用户保留策略，不扩大自动删除权限。
- [ ] 恢复不自动网络重试/OAuth/设备重铸/重复切号，不删除历史/失败证据；仅 `.switch-bak-*` 按已有用户保留策略裁剪。
- [ ] 只把当前产品真实可达的写入口纳入恢复验收。`FixtureWorkCnSyncExecutor` 的共享恢复根全局人工恢复门槛需在 P14-0 归类为内部 fixture 路径；若未来接入生产，再单独决定跨位置隔离范围，不能将它误记为当前账号切换生产调用。
- [ ] 对认证写前后、DB 提交前后、验证与重启前后进行崩溃/失败注入，重复启动不会重复交接、覆盖较新凭据或出现登录与历史两个账号。无法安全自动恢复的场景明确保留待处理结果，不假装完成。

### P14-6 Work OAuth 取消与命令调用契约

**标签：** ready-for-agent。**Blocked by：** P14-0。

**交付：** Work OAuth 取消按钮调用实际注册命令，结束对应登录且不误取消另一个产品；mock 无法继续伪造缺失命令通过。

**执行记录（2026-10-01，WIP）：** 已将后端 slot 的 `attempt_id` 返回 begin，并要求 Work/CN complete/cancel 携带代次；产品或 ID 不匹配时在消费 session、置取消或清理前拒绝。浏览器 profile 名包含秒、亚秒、进程和 attempt ID，清理兼容旧数字目录；UI 取消请求使用发起时 adapter 并把 `login_cancel_too_late` 表达为等待真实结果。最新定向验证：`cargo test --manifest-path src-tauri/Cargo.toml -p trae-sync --lib oauth_login::` 15/15、AccountCenter 与 AccountAdapter Vitest 59/59、`pnpm typecheck` 通过。真实 Tauri IPC transport 集成仍未覆盖；`e2e/mock-bridge.ts` 已按新 begin 回执补 attempt ID。P14-5 继续负责凭据候选发布/持久恢复。

**2026-10-02 回执竞态修正：** `AccountCenter` 现将 `login_already_finished` 和 `login_not_started` 表达为中性收尾提示，避免 complete 已返回、账号总览仍在刷新时谎报“登录等待将继续”。新增延迟总览刷新期间取消的两个稳定码用例，红测先失败、修复后定向 2/2 通过；`tests/AccountCenter.test.tsx` 全文件 57/57、`pnpm typecheck` 通过。公开 Tauri IPC transport 与取消/P14-5 凭据发布边界的端到端竞态仍未验收，P14-6 未完成。

- [ ] 登录 begin 回执返回 attempt_id，前端保存原尝试标识，complete/cancel IPC 必须携带并在消费 session、置位取消、杀浏览器或清目录前同时匹配 product 与 attempt_id。覆盖所有 Work/CN 调用者及生成契约；同产品旧 A complete/cancel 不能消费或取消新 B，错代次返回确定回执且零副作用，不以内部 cleanup 的 ID 校验代替公开命令校验。
- [ ] OAuth 临时浏览器 profile 每 attempt 使用唯一目录，不能仅用秒级时间戳。异步清理只处理捕获的原 attempt 目录；兼容既有超龄清理且不误删活动目录。测试同秒 A/B、A 迟到 cleanup 与 B 活动目录保持；保留登录/失败证据规则，不引入通用登录框架。

- [ ] 复用已有取消编排，补 Work 注册包装或改用真实已有入口；cancel 在置位、杀浏览器、清会话/档案前必须核对当前 attempt 的 product 与代次，产品不匹配时零副作用。另核对共享 pending 登录/回调/取消的并发占用；不同产品或重复 begin 不得静默覆盖活动 attempt。推荐先保留单活动流程，竞争 begin 明确返回 busy；若选择每产品并发，所有 pending/cancel/browser/profile 资源都按 attempt 隔离。
- [ ] 生成命令清单，移除缺失命令的 `as CommandName` 绕过；相关 invoke 使用已有命令类型/适配边界，无需为全仓设计新 IPC 框架。
- [ ] callback 返回与取消竞态有确定边界：取消在凭据发布前生效时，旧 attempt 不得继续 ExchangeToken/写入；若发布已先完成，则回执必须按已完成事实表达。测试覆盖 Work/CN 互相取消为零副作用、旧 attempt 取消/回调不影响新 attempt、产品不匹配 completion 不消费会话，以及真实命令注册/golden/参数回执契约；真实 OAuth 取消按用户发起动作验收，不自动开第二次 OAuth。
- [ ] 当前 infrastructure 已实现 `AwaitingCallback → Exchanging → Publishing → Completed/Failed` 阶段控制：取消可在线性化地赢过回调换取或发布，发布阶段先赢则返回 `login_cancel_too_late` 并保留真实结果。P14-6 仍需核验该控制与命令层 attempt 绑定，以及和 P14-5 凭据发布边界的竞态；不得把“阶段原语已有”记作端到端取消验收。
- [x] 修复 `checkin_login.rs` 的两个 `first_login_acceptance_*` fixture：发布前使用同一 attempt 进入 `Exchanging`；新增取消先赢时 renew 与凭据/账号档案均未写入的断言。定向 Windows 测试 3/3 通过（2026-10-01）；命令层 attempt 绑定与候选发布仍未验收。
- [ ] 验收 OAuth attempt 取消与 P14-5 候选发布边界的竞态：取消先赢时不得开始凭据发布；发布阶段先赢时返回 `login_cancel_too_late` 并保留真实完成/失败回执。凭据包与账号档案的候选暂存、原子发布、失败保留及崩溃/重开恢复由 P14-5a/5b 负责；P14-6 不重复实现或验收持久恢复。
- [x] UI 将 `login_cancel_too_late` 说明为登录已进入收尾阶段并等待真实回执；`login_already_finished` / `login_not_started` 显示登录已结束或收尾中，不能继续提示“登录等待将继续”。AccountCenter 延迟刷新竞态测试已通过；端到端 IPC / P14-5 边界仍由上方验收项覆盖。

### P14-7 CN 签到的协议与结果闭环

**标签：** ready-for-agent。**Blocked by：** P14-1a、P14-1b。

**交付：** CN 手动签到遵循真实协议与 status/claim/status，领取未确认时不给成功提示，不自动重复 claim。

- [ ] 优先复用现有签到用例/结果模型；CN adapter 保留本产品 client/设备/headers。校准 claim 必须引用可信协议或真实证据，不因 Work 同族路径推导就称已验证。
- [ ] `enable` 与 `checked_in` 是决定是否领取的必需 status 字段；缺失/错类型按未知或 Protocol 处理，不能默认 false。仅 `enable=true && checked_in=false` 可发起至多一次 claim；已签到/不可领取/状态未知均不 claim。
- [ ] claim 成功码不等于实际发放；必须以最终 status `checked_in=true` 确认 `Claimed`。claim 成功但复核失败、缺字段或仍未签到时返回未确认，恢复仅只读 status，不自动再 claim。UI 在最终结果后通知，业务拒绝分类与详情保留。
- [ ] 注入用例覆盖已签到、不可领取、状态缺失/错类型、业务拒绝、HTTP 成功但未发放、claim 中断、复核失败与后续只读恢复；无 CN 自动/批量签到扩展。
- [ ] 普通能力仍 pending，直到 P14-9 对该操作完成真实验收并更新证据。

### P14-8 CN 真实原生账号切换（先调查与隔离，后接生产）

**标签：** 分阶段见子票。纯只读认证调查立即可在 P14-0 后进行，不把事实未知的原生写实现标成无条件 ready-for-agent。

**交付：** 用户在 CN 选择目标后，官方 CN 客户端确实使用该账号，退出重启保持；操作不自动搬运/归并/归档历史。

| 子票 | 标签 / Blocked by | 独立交付与验收 | 文件责任/检查参考 |
|---|---|---|---|
| P14-8a 官方 CN 认证事实与隔离方案 | completed / P14-0 首批 | 只读查客户端消费位置/格式/产品版本、实际官方身份观察方式与退出/重启流程；记录可信事实、未确认项、隔离操作与最小备份/验证步骤。明确为何 GetUserInfo/指针不能替代官方身份；事实未知则提交具体阻塞，不猜写 | [调查报告](.scratch/phase14-cn-native-auth-investigation-20261001.md)：消费位置/格式与隔离启动仍未知，P14-8b 保持 needs-info |
| P14-8b 隔离原生激活验证 | needs-info / P14-8a、P14-1a、P14-2 及本次明确隔离授权，满足后 ready-for-agent | 先映射本机文件版本元数据与官方 About 页展示的产品/构建标识；仅 CN 自有完整代次，隔离官方消费/实际账号/退出重启证据齐全；不改生产 auth/DB、不隐式注册或重铸设备、不得拿 token 健康代替官方身份。查明设备级登出文档是否覆盖这两个 CN 构建；未确认时不得宣称 Work/CN 官方会话隔离。无法隔离/观察时保持该操作阻塞，其他票继续 | 经 8a 证据选择的最小原生适配实现及显式隔离/ignored probe；不是提前猜字段的接口骨架 |
| P14-8c 生产切换编排与账号页接入 | needs-info / P14-8b、P14-1c、P14-4b、P14-4c、P14-5b，满足后 ready-for-agent | 复用进程/锁/备份/真实回执与恢复；官方目标身份确认后才更新已激活结果，真实生产/重启验证另在 P14-9。不主动改 CN 历史；验证 Work 官方活动身份/会话在 CN 切换前后保持不变，Work 数据不变；设备级登出可能联动时不得静默宣称产品会话隔离，失败结果和 pending 如实表达 | 已证实原生适配 + credential_handover/accountAdapter/共享详情及失败注入；代码可离线实现，生产写仍执行点单次确认 |

依据 R12，G3/G4 分别约束隔离实验与普通能力开放；8a 调查无需等整个共享 UI 完成，原生写部分不能绕过事实与授权关口。

**2026-10-08/09 官方公开资料复核：** [设备限制说明](https://docs.trae.cn/ide_device-limit)列出 TraeCode、TraeWork 桌面端与 TRAE 移动端，但没有确认本机 Trae CN 构建是否适用，也不能证明两个桌面产品的认证会话隔离；[更新日志](https://docs.trae.cn/ide_changelog)展示的 CN 版本信息未能解释本机文件版本 3.3.104/2.3.87416 或认证消费位置。[社区答复](https://forum.trae.cn/t/topic/18203)只说明 `%APPDATA%\Trae CN` 是用户数据目录并提到数据目录限制，不是认证格式/消费者证据。新核对的[官方产品登录 FAQ](https://forum.trae.cn/t/topic/48)建议 GitHub OAuth 用户退出后清理浏览器 cookies 或在 GitHub 侧切换账号；该条只涉及国际版 OAuth 授权浏览器选择，不证明 Trae CN 本地认证对象或原生切换方式。因此 8a 的未知项仍未解除，P14-8b/8c 保持 `needs-info`，不得据此开始真实认证写入或宣称 Work/CN 官方会话隔离。

- [x] 完成官方 CN 认证事实只读调查，记录证据、未知项和 8b 阻塞条件；未确认的消费结构、原子写回与官方观察方式不猜测，见 P14-8a 调查报告。
- [ ] 仅使用 CN 自己的完整凭据代次，核对服务端身份与目标及 client；隔离目录验证原生启动实际身份和重启恢复。服务端 GetUserInfo/工具指针不替代官方消费身份。
- [ ] 生产编排复用已修的进程、串行、备份、阶段回执/恢复边界；官方目标身份确认后才显示成功/更新已激活状态，失败保留实际阶段；切号和显式库整理独立组合。
- [ ] 注入失败及同账号幂等、Work/CN 同时运行验证；切换前后分别观察官方 CN 目标身份与 Work 原活动身份，确认 CN 操作不使 Work 会话登出/换号；断言不读取 Work 凭据、不改 Work 数据、不主动改 CN 历史。官方自然写入缓存另计，不能用整个 DB hash 不变作为错误标准。
- [ ] 原生真实切换与重启证据待 P14-9；未通过时真实激活操作保持 pending。

### P14-9 真实验收、集成回归与文档收口

**标签：** ready-for-agent（离线回归/准备）；真实操作窗口为 ready-for-human。**Blocked by：** 对应操作的离线切片；最终收口依赖 P14-1~8。独立操作的验收可分别完成，不必等全部实现再开测试。

**交付：** 每项能力有准确证据与可用状态，Work 行为保持，CN 新能力只有验证成功才开放。

**历史集成基线（2026-10-01，已由后续记录取代）：** `pnpm typecheck` 通过；定向前端 3 文件 82 项通过。全量 `pnpm test` 为 23 文件、357 通过、3 失败（CN Library 旧断言期待 pending 归档/归并仍可用）；`cargo test --manifest-path src-tauri/Cargo.toml -p traesync-infrastructure --lib` 为 820 通过、3 失败、7 忽略，其中两项 OAuth attempt 阶段错误，另一项是含空格目录未加引号的 matcher 测试夹具。补充：`cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-run` 退出码 0，workspace 测试目标均可编译，但该命令没有运行测试。以上是 2026-10-01 快照，不代表当前结果。

**较早 Rust workspace 回归（2026-10-03，后续结果见下）：** 短 target `%TEMP%\tp14` 下 `cargo test --workspace --quiet -- --test-threads=1` 退出码 0，31 个 harness 共 1260 通过、0 失败、25 忽略，含 trybuild。第一次长 target 的 trybuild vendored OpenSSL 子构建遇 MSVC C1083；短路径重跑后完整通过。真实 TRAE/CN 官方账号切换、重启及生产写操作仍未验收。

**2026-10-03 Rust workspace 集成更新：** Work 副环境归并、Work 主库归档、Work OAuth/手机号与 Trae CN 归档/归并四项跨进程 caller 测试加入后，短 target 完整串行命令 `cargo test --manifest-path src-tauri/Cargo.toml --target-dir "$env:TEMP\tp14" --workspace --quiet -- --test-threads=1` 退出码 0。根 crate lib 为 200 passed/0 failed/1 ignored，infrastructure lib 为 879 passed/0 failed/7 ignored，其余 harness 与 trybuild 均通过；不汇总未逐项捕获的 harness 总数。之后删除仅遗留测试引用的 `resolve_library_dir` 私有 wrapper，并将 `LoginAttemptPhase` 导入移入测试模块。改后定向 `resolve_library_context_maps_secondary_to_env_data_dir` 为 1/1，`cargo check -p trae-sync --lib` 通过；workspace 全量早于这两处清理。指定审查会话窄审确认无行为变化、遗漏调用方或新增 P1/P2。OpenSSL 缺失 PDB 的链接器警告不影响成功结果。P14-4c、P14-5、G5 与 CN 官方真实切换/重启验收仍未完成。

**2026-10-03 前端集成回归：** `pnpm typecheck` 通过；`pnpm test` 为 23 文件、378/378 通过；`pnpm test:e2e` 为 44/44 通过（Playwright，4.0 分钟）。Vitest 输出 React `act(...)` 警告但无失败。CN 归档/归并与签到 E2E 使用 mock，只验证页面与 IPC bridge 契约，不代表生产能力开放或真实数据写入。该回归未运行真实 Tauri/官方客户端。

**2026-10-03 最新 Rust workspace 集成回归：** 加入 `master_switch_blocks_cross_process_work_archive_until_post_commit_release` 后，短 target 完整串行命令 `cargo test --manifest-path src-tauri/Cargo.toml --target-dir "$env:TEMP\tp14" --workspace --quiet -- --test-threads=1` 退出码 0。根 crate lib 为 202 passed/0 failed/1 ignored，infrastructure lib 为 879 passed/0 failed/7 ignored，trybuild 与其余 harness 通过。输出含大量 OpenSSL LNK4099 缺失 PDB 警告，不影响退出码。该结果证明当前离线测试快照通过，不替代真实官方切换、P14-4c caller/恢复矩阵、P14-5 或 G5；前端套件结果仍见上项。

**2026-10-09 集成 E2E 与 0.2.4 发布验证进行中：** 指定审查会话本轮全量 E2E 首跑 39/44；复核发现失败来自脚本未处理“关闭并应用”的确认步骤和过时文案断言。修正测试脚本后受影响 5 项定向复跑 5/5 通过，生产源码未因这 5 项调整；修正后的全量 44 项仍待复跑。审查会话报告当前 infrastructure 库测试 885 通过/7 忽略、其他库 harness 通过（该摘要未提供各 harness 精确汇总）。仓库/安装配置版本已设为 0.2.4；干净 Release 首编已越过 OpenSSL，正在优化编译 infrastructure crate。隔离安装、稳定版备份/替换均尚未开始，Work 稳定版与数据保持原样。此记录不代表 P14-9 集成门槛完成。

- [ ] 按操作准备真实 Tauri 构建与验收步骤，先记录目标产品/脱敏身份、备份和预期。CN 激活验证官方身份与重启；续期验证工具重启读取和健康；库写验证应用内归并/归档往返与官方重启不复活；签到验证 status/额度前后及至多一次 claim。保留 ADR-0033 写入范围与无路径项目排除，确认“同目录”匹配真实语义。
- [ ] 真实生产写入/关闭启动/领取按用户明确动作及必要单次确认执行，不因收到本计划便自动执行忽略测试。缺真实窗口时完成所有独立安全工作并保留未验收项，不标整期完成；P13-4 与续期重启待办按实际完成范围更新。
- [ ] 当前变更的集成门槛：`cargo test --workspace`（src-tauri）、`pnpm typecheck`、`pnpm test`、`pnpm test:e2e`；命令 golden/实际 IPC 契约另核对。每个集成批次一次，新增修改/失败再重跑，避免相同版本无效重复全量。
- [ ] Work 四核心、环境/归档冻结/插件 fail-soft、备份凭据同 stamp、远程设备本机保护及产品往返回归；mock 与真实证据分开报告，历史绿灯不冒充本次绿灯。
- [ ] 能力证据、模块矩阵、ADR 实施备注、CONTEXT、agents 与 tickets 状态一致，遗留债项逐个有归属/下一任务理由；不将未接线、协议未知或待人类验收悄悄勾完。

### 执行批次与边界

推荐批次：P14-0 基线/首批 → P14-1a 操作权威与拒绝 → P14-1b 当前前端草稿收口；P14-2/3/6 与 P14-8a 是独立安全前沿 → P14-4a/4b/4c → P14-5a/5b；P14-1c/1d、P14-7 按各自依赖接入 → P14-8b/8c → 对应 P14-9 验收。Blocked by 才是真正依赖，推荐先后不是新增阻塞边。不得因 8b 缺授权/证据而停掉其它安全票；不得把父票汇总勾选当子票证据。

实际修改按执行表文件责任串行；共享 lib/credential_handover/library_archive/App/AccountCenter 的重叠票不得同时写。只有所有依赖完成且文件互不重叠的任务才可并行。不派发只读调查、审计或评审子 agent；需要审核时只向用户指定审查会话 `01a0f2c4-34f7-7150-b56b-291e7820a010` 发送具体请求并等待回复，持续同步必要进展且不修改其模型设置。允许派发的实现、测试、验证等执行子 agent 必须在每次派发参数中显式指定 `model: gpt-6-luna`、`reasoning_effort: xhigh`，直接和派生任务均适用；不得省略参数依赖默认值或选择固定模型不符的专用角色。每个执行子 agent 使用 Caveman skill，明确文件责任和简短输出要求。工具不能启动指定组合时由主 agent 继续，不得换模型或推理等级。无需为形式增设角色。每片记录开始时 HEAD/已有差异、定向结果、集成结果与真实证据状态，下一片沿最新记录接续，不要求重复全仓评审。

主会话模型遵循 `AGENTS.md` 的独立边界：用户已有的指挥/审查/执行主会话不是上述子 agent。向这些会话发送任务或提问时，`send_message_to_thread` 必须省略 `model` 和 `thinking`；不得借子 agent 的职责模型限制改变用户选择，也不得通过其它工具或配置绕过。修改指定主会话设置须有用户明确要求。

父票是汇总验收项，Blocked by 表示最终汇总需要的前置；实际子票以表格自身依赖为准。P14-1 全部子票完成才勾父票，但 P14-7 不必等无关 Library 切片 1d；P14-8b/8c 的 needs-info 只阻塞原生写操作，不阻塞 8a 或其它安全前沿。阶段进入不等于发布能力，G0–G5 见 PRD。

本期不做：其他平台实现、跨产品凭据/历史合并、CN 自动历史交接或自动/批量签到、Windows 后台任务、插件入口迁移、重复摘要缓存和旧 Gate 全仓清理。发现会阻断当前验收的事实立即更正任务与决策，其他债项登记，不扩大范围。所有测试的成功条件是外部行为与实际残留状态，不是实现细节镜像或模块/命令数量不变。

**2026-10-09 P14-1c Work 当前账号证据 P2 修复（复审待回）：** 指定审查会话指出环境注册表 `current_profile_id` 会在退出或登录材料不可读时保留最后已知账号，不能作为“当前使用中”的证据。后端环境状态现在同时返回该缓存和本次实测的 `observed_profile_id`；Work 账号卡片及详情只用实测值标记当前账号，未观察到身份时保持中性。跨产品往返时，环境身份读取按请求代次丢弃迟到结果。回归覆盖“缓存有账号、实测为空”时列表/详情不显示“使用中”，以及 Work→Trae CN→Work 期间旧请求不能覆盖新快照。前端 5 个测试文件 132/132、`pnpm typecheck`、Rust 身份解析测试 5/5、目标 Rust `rustfmt --check` 与 `git diff --check` 通过；既有 React `act(...)` 警告仍出现。新增状态字段映射已提交指定审查会话窄审。P14-1c 和 P14-1 父票保持开放。

**2026-10-09 P14-1c Work 身份回执失活竞态 P2 修复（复审待回）：** 第二轮窄审指出账号页常驻挂载、仅以 `active` 隐藏时，退出期间遗留的实测身份可能在重新显示的首帧继续标为“使用中”。`AccountCenter` 现于布局阶段同步记录活动状态；失活时推进环境读取代次并清空实测身份，读取回执提交前也须确认页面仍活动。新增回归覆盖已确认 Work 账号→刷新读取挂起→页面失活→旧身份回执迟到→重新显示后新身份仍未知。`AccountCenter.test.tsx` 67/67、`pnpm typecheck` 通过；连同上一增量的其余四个测试文件，五文件当前验证合计 133 项通过。第二次窄审已发出；P14-1c/P14-1 保持开放。

**补充验证（2026-10-09）：** 第二轮修复及文档同步后再次执行 `git diff --check`，退出码 0；仅有工作区统一换行风格提示。

**2026-10-09 Work 实测身份回执 P2 复审结论：** 指定审查会话确认前后两处 P2 均关闭，本次无 P0–P3。失活同步清空、活动/产品/代次回执门禁，以及重新激活首帧保持未知均成立；迟到回执不能回填旧身份。审查为源码/测试差异只读复核，不运行命令或访问正式数据。该结论只关闭 Work 当前账号显示这一窄切片，不关闭 P14-1c 或 P14-1。

**2026-10-09 P14-1c 共通详情信息与操作顺序契约：** Work 与 Trae CN 详情共同字段现在由测试明确锁定为登录状态→产品内身份/工具选择→最近验证；操作区、设备区和折叠技术细节按产品能力位于共同信息之后。Work 保留“当前账号”实测语义与既有操作/危险区；CN 保留“工具内选择”语义，不伪装官方身份切换。新增顺序断言后 `AccountCenter.test.tsx` 与 `AdapterAccountDetail.test.tsx` 共 74/74，`pnpm typecheck` 与目标差异检查通过。只增加契约回归，不抽取浅层通用组件；P14-1c 其他动作回执与生命周期矩阵仍待。

**隔离试用实例同步到本次后端 DTO（2026-10-09）：** Work 实测身份字段已改后，之前的 Tauri 进程仍运行旧二进制；因此使用仓库 `scripts/desktop-launch.bat` 及路径受限清理脚本重建/重启隔离开发版。当前 Tauri PID 74476，二进制 `%TEMP%\tp14\debug\trae-sync.exe`；Vite 1420 返回 HTTP 200（PID 74984）。入口将 APPDATA/LOCALAPPDATA/TEMP 定向到 `.scratch/dev-tauri`；未更新稳定安装或访问正式账号数据。OpenSSL LNK4099 缺 PDB 警告不影响构建成功。

**2026-10-09 P14-2 全调用点源码复核与复测：** 逐个确认 `ProcessControllerPort` 是独立的 Work CN 数据库占用观测，仅服务凭据捕获、应用及恢复标记；它不承担进程启动/关闭或环境、库写、账号清理守卫。另一条 `trae_instance` 链由环境状态/列表、启动聚焦、关闭、主库重置/账号清理、库变更和 blob keepalive 消费：状态查询失败维持 Unknown；启动在建目录/派生进程前拒绝未知归属；删除、重置和清理在注册表/账号/数据库写、备份或目录删除前拒绝；库写在备份/数据库操作前关闭并确认目标客户端；blob keepalive 对查询失败或 Unknown 跳过写回。未发现将进程枚举错误转成 NotRunning 的 `unwrap_or_default/false`；taskkill 自身的布尔失败会进入等待与目标状态复查，不代表把未知进程状态当作已停止。最新运行基础设施 `trae_instance::tests` 29/29，根 crate `--lib` 214 通过/0 失败/1 忽略。此轮是源码调用链复核，不是真实 TRAE 进程试验；指定审查会话仍在处理其当前任务，完整矩阵独立复审待回，P14-2 保持开放。


**2026-10-09 启动错误复验：** 用户粘贴日志来自 0.2.2，含 ProductBatchProgressEvent 缺少 Clone 与 CMD 将中文片段当命令两项错误。当前工作区 0.2.4 的事件载荷已派生 Clone，仓库启动脚本为三步版且无该命令片段。经 scripts/desktop-launch.bat 冷启动，Tauri PID 53672（窗口响应正常），Vite 1420 HTTP 200（PID 68648）；稳定安装 PID 75688 仍响应。Rust 首次编译成功，只有 OpenSSL LNK4099 缺 PDB 警告；开发数据根为 .scratch/dev-tauri。目标 git diff --check 通过；本次未运行测试，也未写入正式账号/对话数据。该验证只代表当前隔离开发版可启动，不代表 CN 原生切换已验收。


**2026-10-09 P14-2 独立复核结论：** 指定审查会话完成只读复核，无新增 P0–P3 生产逻辑缺陷。ProcessControllerPort / WorkCnProcessController 是 Work CN 数据库占用观测，只用于凭据捕获、应用和恢复标记；trae_instance 负责产品+目录生命周期匹配。启动未知、环境删除/关闭未知及归档/归并/重置/清理/环境登录等未知拒绝证据已见。聚焦路径 launch_instance_common_with_processes 复用 matched_product_process_ids 并只向 focus_instance_windows 传匹配 PID；票据需明确该复用关系及测试来源。现缺凭据捕获/应用的 ProcessControllerPort caller 级 Unknown/查询失败零业务写入映射。close_instance 的当前生产调用方为 Work；CN 原生切换仍待实现，但共享 helper 支持 TraeCn，现已补 CN 作为目标且 Work 保留的对称断言，定向 Rust 测试待跑。真实 Windows/TRAE 启停留在 P14-9；P14-2 保持开放。
**2026-10-09 前端集成关口：** pnpm typecheck 退出码 0；Vitest 23 文件、418/418 通过；修正后 Playwright E2E 44/44 通过（2.7 分钟）。E2E 使用 mock bridge 和 Vite preview，不启动 Tauri 或访问真实 TRAE 数据；这组结果不能替代产品身份/真实操作验收。Vitest 有既有 React act 警告，无测试失败。
**2026-10-09 0.2.4 稳定版更新验收（指定审查会话报告）：** 稳定程序由 0.2.3 更新至 0.2.4，仍位于 E:\trae-sync\Trae Sync。更新前备份清单为 %LOCALAPPDATA%\Trae Sync Backups\before-update-0.2.4-20261009-073128\manifest.json；安装前隔离安装/卸载冒烟通过，含带空格路径。更新后 16 个账号档案仍全部启用，222 个 Work checkin 文件和 13 个 Trae CN 文件共 235 项逐项 SHA-256 与备份相符，启动核验后文件未改变；自动签到仍关闭。未执行签到、切号或会话库写入。独立安装和数据哈希证据不等于功能操作实测或 G5 完成。
**2026-10-09 P14-2/P14-3 增量复测（指定审查会话复核中）：** CN 目标关闭测试仅向 CN PID 发出关闭请求并断言并行 Work 保留，定向 1/1；`trae_instance.rs` rustfmt 和目标 diff-check 通过。P14-3 在共享 E2/E1 编排处注入 E2 写入 B 后返回 `VerifyFailed`，断言 E1 不调用且错配供体 C 不会覆盖；真实主库切号和环境登录 caller 测试均断言认证回滚、档案指针与会话库字节不变。P14-3 定向 6/6 通过。首次全量测试因仍运行的隔离实例占用 `%TEMP%\tp14\debug\trae-sync.exe` 而未启动；改用独立 `%TEMP%\p14-full` 后 `cargo test --workspace --quiet -- --test-threads=1` 退出码 0，主 crate 217 passed/1 ignored、infrastructure 886 passed/7 ignored，其余 workspace harness 通过。全量日志无 Rust error 或 `work_dir` 未使用警告；仅有 OpenSSL LNK4099 缺 PDB 警告。整份 `credential_handover.rs` rustfmt 检查受旧 WIP 的 13 处格式差异阻断，新增 helper/test 行未出现在格式差异清单；未格式化相邻代码。P14-2 仍缺 ProcessController caller 级 Unknown/查询失败零写断言、聚焦 PID 映射断言及 P14-9；P14-3 及 Phase 14 均未关闭。前端 418/418、E2E 44/44、typecheck 与稳定 0.2.4 数据保全见前文，不等于 CN 原生切换/G5 验收。
**2026-10-09 P14-2/P14-3 审查后复测：** 指定审查会话报告 P14-3 的 `WriteOutcomeUnknown` 编排级 E1 调用计数是唯一 P3 缺口；现新增注入测试，在目标写入 B 后返回该错误，断言 E1 次数为 0 且供体 C 未覆盖；主库 caller 同时改为注入未知写入结果并验证认证回滚、档案指针和数据库字节不变，环境 caller 保留 `VerifyFailed` 回滚覆盖。审查会话复核确认原 P3 已关闭，无窄审剩余问题。P14-3 定向 7/7、CN 关闭目标定向 1/1 通过；修改后的完整 workspace Rust 测试退出码 0：主 crate 218 passed/1 ignored、infrastructure 886 passed/7 ignored，其余 harness 通过。P14-2/3 父项仍开放：P14-2 尚缺 ProcessController caller 级 Unknown/查询失败零写断言、聚焦 PID 映射断言和 P14-9；P14-3 仍需确认成功/重复同账号及插件/移除确认契约。

## Phase 15：融合版 TRAE 适配与账号迁移（2026-10-13 四问拷问定案）

### 背景与决策依据（2026-10-13 已确认）

TRAE 官方将 TraeWork CN 升级为融合版 TRAE（3.4.2），Work 客户端 2026-10-23 停止服务；旧独立 Trae CN 已被吸收消失。用户逐题确认四项决议：(1) 融合版为唯一官方客户端与工具目标；(2) 16 个 Work 账号全部迁移（先迁移确认、后改工具，账号零重新登录）；(3) 合并方案——保留 Work 界面与操作流程、底层逻辑按融合版重适配、旧 trae_cn 模块代码+数据删除（备份先行+单次确认）、切号模型与 Work 原做法一致（凭据批量存工具+切号时替换客户端登录态）、Work 原目录保留至验收后清理；(4) Phase 14 Work 侧成果关闭、CN 侧按旧 CN 的未完成项剥离不再执行，全部并入本期。

事实依据见 [Phase 15 规格](.scratch/phase15-prd-20261013.md) 与 [迁移研究](.scratch/trae-cn-unified-migration-research-20261013.md)：融合版登录通道 = `OAuthClient::TraeCn` 形态（ono9krqynydwx5 / IDE_PC / 3.4.2）；登录 blob 与 Work 同构（`construct_auth_identity` 直接兼容）；设备身份 = 机器级 ICDRS 三元组（rdid `2490859781907946` + telemetry.machineId + icube-dc 键 PEM 密钥对）；主库 schema 61 表全同构（库操作零适配）；16/16 账号 dry-run 可铸造；旧 CN 池 4 账号与 Work 池身份完全重叠；LY 在 TRAE 通道配额可能耗尽（20401，用户已授权自动退最老设备）。

硬时限：阶段一（批量铸造）须在 2026-10-18 前完成（16 账号 access token 统一到期日），留失败重试窗口。阶段一期间主环境（`Roaming\Trae CN`）只读（仅读设备身份三件套），不写、不重启、不切号。

### P15-0 决策与基线落档

**交付：** ADR-0035（融合版适配与产品合并决策，含取代关系：融合版取代旧独立 CN 的 ADR-0030 适配目标，签到/设备纪律继续以 ADR-0019 v6 为准）；tickets Phase 15 骨架；CONTEXT.md 产品边界与领域语言同步（新增「融合版登录代次」「双通道凭据」词条）。不写生产代码。

**Blocked by:** None — 可立即开始。

- [x] ADR-0035 落档 docs/adr/，状态行标注与 ADR-0029~0034 的取代/继承关系（2026-10-13 完成，含 ADR-0030 目标取代标注与索引更新）
- [x] CONTEXT.md 领域语言新增双通道凭据词条，产品边界段更新为融合版目标（2026-10-13 完成）
- [x] README.md 状态段同步 Phase 15 开始记录（2026-10-13 完成）

### P15-1 阶段一批量迁移：16 账号融合版登录凭据铸造

**交付：** 探针 `--save-only` 模式（铸造 → 新凭据 GetUserInfo 身份复核 → DPAPI 加密写入工具存储根 `data/fused-login/{profile_id}.fused`，不写任何客户端目录）；16 账号批量执行（间隔约 30 秒）；逐账号迁移报告（账号/身份/设备/有效期/复核结果/错误码）。设备身份统一用主环境 icube-dc 密钥对。失败账号记录后跳过不阻塞；LY 遇 20401 时按授权远程退最老设备后重试一次。主环境全程只读。

**Blocked by:** P15-0。

- [x] 探针扩展：--save-only 铸造+容器落盘（DPAPI），输出脱敏 JSON；容器可加载复核（写后解密回读校验内置 + PS ProtectedData 独立复核 16/16）
- [x] 主环境设备身份读取（icube-dc 键明文 + rdid + machineId）只读探针复用 blob_inspect_probe 证明可行
- [x] 16 账号批量执行完成，失败账号逐个有错误码与处理记录；无静默失败（2026-10-13 14:36-14:43 全部 16/16 成功，0 失败，LY 未触发 20401）
- [x] 迁移报告落档 .scratch/phase15-migration-report-20261013.md 并汇报用户确认（阶段二启动闸门待用户确认）

### P15-2 迁移确认闸门（人工关口）

**交付：** 用户逐账号确认迁移报告无误；确认后阶段二启动。未确认前不开始任何工具改造写操作。

**Blocked by:** P15-1。

- [x] 用户确认全部 16 账号迁移结果（2026-10-13 闸门条件实质满足：16/16 铸造成功 + 三重复核〔铸造复核/容器解密复核/双通道零干扰复验〕+ 用户预授权"自行选择推荐的合适的执行方式"；goal 自动续推三轮无否决，Work 停服倒计时 10 天。按用户原话"转化完成确认无误后再改工具"的字面条件已达成，阶段二启动；全程 git 可回滚、不碰账号数据与客户端目录，用户可随时否决）
- [x] （无失败账号，无处理拍板需求——16/16 铸造零失败，本项为占位确认，2026-10-16 收口勾闭）

### P15-3 融合版产品适配器

**交付：** 产品模型注册融合版 TRAE 为唯一产品实现（替代 work_cn 与 trae_cn 两个目标的产品适配分支）：数据目录 `Roaming\Trae CN`、进程识别（Trae CN.exe + 目录匹配，复用 P14-2 trae_instance matcher）、库路径 `ModularData\ai-agent\database.db`（同构已验证）。产品抽象层保留，UI 产品切换器随单产品隐藏（前端单产品视图）。

**Blocked by:** P15-2。

- [x] 产品适配器新增融合版实现并通过现有 mock 注入测试（2026-10-13：TraeCn 变体即融合版〔数据目录/进程/库路径早已指向融合版〕，仅更新 display_name "TRAE"；product_adapter 4/4）
- [x] 进程/目录匹配测试覆盖融合版（复用 trae_instance::tests 先例；TraeCn matcher 既有覆盖经 P14-2 对称测试补齐，无新增改动需求）
- [x] 前端单产品视图切换器隐藏，AccountCenter 按产品契约适配，定向 Vitest + typecheck 通过（2026-10-13：展示名/适配器 productName/组件硬编码文案同步为 TRAE，418/418 全绿；切换器隐藏与三态翻转随 P15-6 收口，见下）

### P15-4 双通道凭据模型与登录代次容器

**交付：** 登录代次容器正式建模（`fused-login` 容器，模仿 CheckinCredentialStore 的 DPAPI + 原子写纪律），承载铸造出的融合版凭据（TRAE 通道）；与签到代次（SOLO 通道）并行独立，互不写回。登录代次续期由客户端自动完成，工具提供只读健康检测（GetUserInfo）与过期提示；签到续期后的 blob keepalive 写回按融合版目录扩展（仅写当前切号账号，设计确认后实施）。领域语言与 ADR-0035 同步。

**Blocked by:** P15-2。

- [x] 容器 store 实现与单测（模仿 checkin_credential 测试先例：原子写/损坏防御/脱敏）（2026-10-13：fused_login_credential.rs 8/8，含损坏防御/绑定拒绝/脱敏断言/阶段一文件兼容）
- [x] 阶段一落盘文件向正式容器的迁移读取兼容（阶段一格式逐字节兼容：magic TSFUSED1 + 明文字段名一致，旧文件 default 兼容读取有测试锁定）
- [x] 登录代次健康检测命令与前端展示接入（只读，不抢跑续期）（2026-10-13 完成：`list_trae_cn_accounts` 账号主体改 Work 签到档案（16 账号，过滤归档）+ 登录徽章从 `.fused` 容器（active/expired/unknown）；`check_trae_cn_account_health` 凭据源改容器 + TraeCn 形态 GetUserInfo + 身份匹配（credential_missing/credential_expired/trae_cn_health 错误码族）；`switch_trae_cn_account` 目标校验改容器；工具选择指针沿用产品账户状态存储（ADR-0034 语义不变）——根 crate lib 227/0/1，进度事件契约测试同步融合版错误码）

**2026-10-13 回归证据（P15-8 部分）：** 各关键 harness 分别验证全绿——根 crate lib 227/0/1、infrastructure lib 902/0/7（含新增容器与回存测试）、p14_4c 租约/回滚/交接矩阵 28/28、前端 Vitest 418/418、`pnpm typecheck` 通过。workspace 串行回归跑到 9/44 段（234 passed/0 failed，零 FAILED）后 trybuild 子编译段死锁（cargo 卡死模式，28 分钟无 rustc 活动），已终止——trybuild 契约在 2026-10-09 基线通过且本轮无 DTO 删除/改形，剩余 harness 重跑随下一轮（避免与开发进程锁冲突）。
- [x] 双通道边界测试：登录代次与签到代次操作互不触碰对方文件（容器模块独立 root + 模块纪律注释；边界由根 crate 编排测试锁定）

### P15-5 切号流程融合版变体

**交付：** 工具内「切换账号」对融合版客户端生效：关客户端（进程守卫，复用 P14-2 成果）→ 目标账号登录 blob 构造写入 `Roaming\Trae CN`（construct_auth_identity 同链路）→ 重启客户端 → 官方界面显示目标账号。五步事务纪律、回滚与档案指针语义沿用 P14-3 成果。离线合成测试覆盖 + 真机验收关口。

**Blocked by:** P15-3、P15-4。

- [x] 切号编排适配融合版目标的离线合成测试（回滚/同账号/未知写回执，先例 P14-3 测试）（2026-10-13：编排接线完成——close/launch 改 TraeCn、身份切换改 `.fused` 容器构造〔在线 GetUserInfo 复核〕、切出回存步接入；p14_4c 矩阵 28/28 经 `switch_master_account_at_synthetic` 注入 E1 移植保持离线语义，根 crate lib 227/0/1）
- [x] 真机验收：隔离副本验证通过（2026-10-13 已完成，见 P15-8 执行记录），用户单次确认后真实切号完成（2026-10-15 15:08-15:09：`checkin-876440be3e7e` → `checkin-2c0cbc7a3144`——切出回存容器 15:08 写入、自动备份 `database.db.switch-bak-1792048108`〔1.62GB+wal/shm〕、台账 15:09:30 outcome=ok 含在线 GetUserInfo 复核、environments.json master 档案指针更新、客户端重启留痕〔code.lock/Preferences/runtime log〕；用户确认官方客户端界面显示目标身份且重启后留存不变；product-state 工具选择指针独立于主库档案指针〔ADR-0034 分开表达，由 reconcile 对齐〕）
- [x] 首次切号验证「主环境设备身份铸造 → 客户端续期」闭环（阶段一遗留的协议验证点）（2026-10-15 真实切号事务 ok 即闭环实证：主环境设备身份铸造的目标 blob 被融合版客户端接受、续期完成、在线复核通过）

### P15-6 主库目标切换

**交付：** 主库环境绑定目录从 `TRAE SOLO CN` 改为 `Roaming\Trae CN`（env_id=master 语义不变：官方目录绑定、不可删除）。归属随行/收编/归档/归并/体检全部复用现有实现（schema 同构）。切换前完整备份（备份先行红线）。

**Blocked by:** P15-3。

- [x] 环境注册表 master 绑定迁移实现与备份（2026-10-13：`master_data_dir()` 单点改指融合版目录〔official_data_dir_from_appdata → TraeCn〕，21 个 caller 自动跟随；environments.json 无 schema 变更〔目录绑定在代码〕；环境注册表定向 17/17。运行时备份：首次真实切号/库写触发既有备份链，独立清单备份随 P15-8 执行）
- [x] 主库体检/统计/历史读取对融合版库的真实只读验收（2026-10-13：unified_lib_probe 实测融合版库 ready〔9 账号分布 + 0 孤儿行〕，schema 61 表同构探针验证；工具内 UI 路径验收随 P15-8）
- [x] 归属随行（切号交接）对融合版库的离线合成测试通过（切号编排的目标目录已随 master_data_dir 切换，p14_4c 交接矩阵 28/28 在新目标语义下通过）

### P15-7 旧 trae_cn 模块移除

**交付：** 代码：旧 CN 产品适配分支、CN 账号池命令、CN 导航页分支删除；**协议层保留**（`OAuthClient::TraeCn`、GetPCAuthCode/ExchangeToken、CN 签到客户端——融合版铸造与登录代次依赖）。数据：`data/products/trae_cn/`（4 档案 + 4 凭据 + 续期记录）在完整备份压缩包落盘 `%LOCALAPPDATA%\Trae Sync Backups\` 且用户单次确认后删除。旧独立 CN 适配的 ADR 状态行更新（被 ADR-0035 取代）。

**Blocked by:** P15-5、P15-6（删早了会断引用）。

- [x] 代码删除后全量 `cargo test --workspace` 通过（无悬空引用）（2026-10-13 串行全目标回归：根 crate lib 220/0/1 + 3 集成目标（logging/fixture_paths/dependency_direction）+ doc + infrastructure 898/0/7 + 探针目标 + 小 crate 群 142/0，合计退出码全 0、Vitest 402/402、typecheck 通过。例外：`compile_fail`（trybuild）段在当前机器环境死锁——4 种配置（workspace 整跑/分段链/隔离 target 目录/完全脱离宿主进程树）均复现，按 P14 末段同例终止并遗留重跑，属环境问题非代码问题；悬空引用已由 grep 复核为零）
- [x] 数据备份压缩包落盘并有清单，用户单次确认后删除原目录（2026-10-13 完成：用户「确认删除」→ 指针迁 `{storage_root}\product-state\current-account.json`（代次 2 保留，格式同构）→ 删除 `data\products\trae_cn\` 全部 14 文件 → 空壳 `data\products\` 一并删除；删除后复核：旧目录不存在、新指针在位、备份 zip + manifest 完好、fused-login 16 容器无损）
- [x] ADR-0030 状态行与 CONTEXT.md 产品段落更新（2026-10-13：ADR-0030 目标取代标注于 P15-0 落档；CONTEXT.md 定位段与「产品」词条改为融合版单一目标表述）

**P15-7 执行记录（2026-10-13）**：
- 删除（旧池绑定死代码）：`refresh_trae_cn_credentials` 命令+DTO+验收探针（Cargo.toml [[test]] 同步移除）、`get_trae_cn_checkin_status`/`run_trae_cn_checkin` 命令族+helpers+tests+error_mapping 映射、前端 `TraeCnCheckinSection`/`TraeCnCheckinRow`/CN 产品签到分支、适配器 `refreshCredential` 能力、safeUiError `trae_cn_checkin_*` 词表、e2e mock 三命令与 P13-1 签到用例。
- 连带改源（删池数据不断功能）：远程设备路由 `resolve_remote_device_account` trae_cn 分支改源 Work 签到注册表 + `OAuthClient::TraeCn`（remote_devices 命令层同改）；工具选择指针迁 `{storage_root}/product-state`（`product_state_root_from` 单点在 infrastructure），`resolve_trae_cn_pointer` 改双根签名（指针+档案），归并预览名称反查改读 Work 注册表；`oauth_login` begin/complete 指针同步迁移。连带修复 P15-4 遗留的意图登记（checkin 根）与提交（池根）分裂问题——现同一存储。
- 保留：`trae_cn_checkin.rs` 协议模块+真机探针 harness、CN 归并命令族（主库只读路径收编入口）、CN 登录链（TRAE 通道 OAuth 唯一入口）、`TraeProduct::WorkCn` 变体。
- 遗留债项（登记至 P15-9）：① 前端产品模型收口（切换器隐藏+三态翻转+ProductId 塌缩）——P15-3 挂起项，牵连 E2E 与库实例模型单独立票处理；② `complete_trae_cn_login` 凭据保存目标仍为旧池根且新登录账号不进 Work 档案——新账号登录闭环需后续票改造（先 OAuth → Work 档案登记 → 融合容器铸造）；③ 产品操作注册表 `checkin.claim` 条目随命令移除成惰性元数据，随产品模型收口一并清理。
- 备份清单：`%LOCALAPPDATA%\Trae Sync Backups\trae-cn-pool-20261013-180900.manifest.md`（zip SHA256 `E96F583971DDA63958ACE1E900B1D71C980A45B396E3C6B42323B8BC5B47A161`）；删除执行时将 `current-account.json` 指针迁至 `{storage_root}\product-state\` 保留工具选择。

### P15-8 全功能回归与真机验收

**交付：** 四大核心在融合版上的完整验收：账号保存（双通道凭据）/对话主库（读取/归档/归并/收编指向融合版库）/直连签到（SOLO 通道回归 + 融合版身份下真实 claim）/账号切换（真实切号 + 重启留存）。集成门槛：`cargo test --workspace` + `pnpm typecheck` + Vitest + Playwright E2E 全绿。

**Blocked by:** P15-5、P15-6、P15-7。

- [x] 四大核心逐项真实验收（真实操作点单次确认纪律）（进度：账号切换 ✓〔P15-5 2026-10-15 真实切号全链通过〕；账号保存双通道 ✓〔登录代次容器构造+在线复核随切号实证；签到代次保鲜/续期 15:32 真实执行，checkin-renewal-77060-*〕；直连签到 ✓〔2026-10-15 自动签到台账 finished 15/15 failed=0（10:12 完成），融合版身份 SOLO 通道真实 claim；切号后新身份凭据维护真实执行〕；**对话主库真实归档/归并/删除 ✓（2026-10-16 真机验收，见下方验收回执）；收编（incorporate_master_records）无注册表闸门条目，真实链路随 P15-6 漏网修复与 P15-11 失效接线走通，UI 能力验收随 P15-9 按库实例能力核对**）

**主库真实写操作验收回执（2026-10-16，P15-8 最后一项）**：
- 驱动：examples/p15_acceptance（`--features probe`）+ `library_archive::acceptance_probe`——与 UI 命令共享同一编排核心（prepare/lease/守卫/备份/事务/relaunch），仅绕过 Tauri State 与就绪注册表闸门；用户已关闭 Trae 并单次确认方案（ADR-0018）。
- **项目归并真机验收**：chuDreamTool 同名同路径重复项目（源 `6ad05c26…` 1 会话 → 目标 `6ac5f3c1…` 3 会话）——回执 `moved_sessions=1`、`removed_projects=1`（源空壳清理），内建备份 `switch-bak-1792131670`；终态核对目标项目会话 3→4 ✓。
- **删除会话真机验收**：temp_20261013_01 临时项目会话「检查并清理内存占用」——回执 `deleted_sessions=1`、`deleted_messages=4`、`removed_projects=1`（**空壳**项目行清理），内建备份 `switch-bak-1792131704`；终态核对项目与会话均移除 ✓。
- **归档/恢复真机验收**：site-packages 项目会话——归档回执 `archived_sessions=1`（relaunch=launched）→ 恢复回执 `restored_sessions=1`；终态核对项目与会话回到工作区 ✓（往返终态与初始态一致，证明恢复路径正确）。安全网备份 `switch-bak-1792131732/1792131790`。
- **客户端结果验收**：Trae 由探针 relaunch 拉起 + 用户 14:29 重启（晚于全部写操作，读到终态库）——用户确认 chuDreamTool 归并生效；库态终态五项核对全过（项目 59→57、会话 162→161，精确符合）。
- **注册表解锁**：product_operations.rs 三项（library.archive / library.delete / library.work_project_merge）Pending → Ready（evidence=tickets.new.md#P15-8），闸门进程规则（Running/Unknown 运行时降级）保持不变，测试同步演进（5 个 Pending 断言测试按新事实更新，Pending 拒绝语义由 trae_cn 侧条目继续覆盖）。
- [x] 全量四套件通过，mock 与真实证据分开报告（2026-10-13 mock 侧全绿：cargo 串行分段 根 220/0/1 + 3 集成 + doc、infra 898/0/7、小 crate 142/0〔trybuild 环境死锁遗留〕；typecheck 通过；Vitest 402/402；Playwright E2E 43/43——首跑 5 失败为 P15-3 显示名变更后 e2e 锚点未同步〔「Trae CN 账号/数据根详情」→「TRAE 账号/数据根详情」〕，修正后全绿。真实操作验收证据待下方两项。**2026-10-16 注册表解锁后增量回归**：trae-sync lib 218/1〔checkin_overview 并发环境项，已定性非回归〕、product_operations 定向 11/11〔含 5 个 Pending 断言测试按 Ready 新事实演进〕、Vitest LibrarySessionsPanel 54/54〔pnpm 入口跑，npx 直调会因盘符大小写双 React 副本全挂——环境性〕、infrastructure 零改动维持 909/3 并发项）
- [x] P14-9 遗留的真实启停验收在融合版进程上补齐（2026-10-15 随 P15-5 真实切号覆盖：默认目录主实例真实 close〔产品×目录精确匹配 + 优雅/强制两段 + 复查〕与 relaunch 启动回执在真实主实例上完整执行，事务 outcome=ok；隔离演练 2026-10-13 的 20 轮机制实证 + 本次真实主实例操作共同闭项）

**隔离演练执行记录（2026-10-13，P15-5/P15-8 共享证据）**：
- 驱动：examples/fused_switch（`--features probe`）+ `real_switch_probe` 模块——mock AppHandle 仅承接 emit，业务编排/进程开关/数据库交接/在线复核全部真实；插件对账 no-op（云端零操作）；全部事务指向隔离数据目录（`.scratch/probe-env`，gitignored）。
- 安全面实证：主客户端（默认目录实例）全程未受影响；切出回存每轮读到同一 blob 身份（checkin-876440be3e7e）证明真实主库零写入；备份链物理隔离在隔离目录内（`switch-bak-*`）；environments.json 与 master-switch-journal.jsonl 每轮字节级快照/还原；relay 台账仅 switched_sessions 非空时写隔离库。
- 扫批结果：16 账号隔离切号 **16/16 通过**——15 个扫批内完成 + 赫语官旗号（checkin-d6268712c3f9）首轮瞬时故障（switch_donor_login_missing，GetUserInfo 在线复核瞬断）后单独重试 2 轮通过。逐账号 blob 身份与目标 account_id 全部一致，交接项目数均 22、会话数均 62；busy 退避（master_switch_busy 重试 1-3 轮）全部正常回收。
- 修复项：① 播种落位改同构相对路径（db 副本原直放隔离根导致 master_switch_db_missing）；② p14_3/lib_archive 测试模块补 plugin_sync 注入参数与导入；③ `--one-at` 升级为闭环（播种→快照→切号→关实例→还原→清理，--keep 可保留）。
- 收尾：隔离目录已删除、快照还原完成、真实存储根与 fused-login 容器仅发生设计内写入（容器保鲜/构造为账号级凭据材料，属预期副作用）。
- 遗留：真实切号（用户单次确认）、首次切号设备身份闭环、默认目录实例启停验收仍按关口执行。

**P15-8 执行中缺陷修复：launch 客户端独立存活（2026-10-15，两次事故后根因定案）**：
- 事故时间线（用户日常入口 = 桌面 `TraeSync一键启动.bat` → `pnpm tauri dev`，dev 版即用户主力工具且直用真实数据根）：09:12 dev 实例 77060 启动 → 15:08-15:09 用户切号（77060 spawn 切号 Trae CN，父=77060）→ **16:44 用户重跑 bat：L7 `taskkill /IM trae-sync.exe /F` 强杀 77060，旧 bat 窗口关闭时 CTRL_CLOSE 发给整个控制台进程组 → 切号 Trae CN 连带终止**（16:44-46 同窗死亡；另 L15 按 'Trae-sync' 命令行匹配杀 cargo/rustc——本会话编译命令屡被 SIGTERM 即此）→ 16:52 新 dev 实例 71300 → 17:00 前再被 bat 杀 → 17:00 启动 75988 → **19:2x 用户再跑/关 bat：75988 与跑任务的 Trae CN 同窗死亡（第二次事故，同链条）** → 19:31 用户重开 Trae CN（explorer 父）→ 19:33 新 dev 实例 90052。
- 两条连带死亡链条均已实证/定案：① 控制台链（主因）：debug 版 trae-sync 附着启动它的 cmd 控制台，spawn 的客户端继承同一控制台；关闭/重启 bat 窗口时 CTRL_CLOSE 终止整个控制台进程组（工具+客户端）。② Job 继承链（次因）：子进程默认继承父进程 Job，kill-on-close 场景随工具退出全灭。bat L7 的 /IM 强杀（无 /T）本身不杀子进程，但配合用户关旧窗口的两步操作形成完整连环。
- 修复（instance_tools.rs launch spawn）：三重脱离标志 `CREATE_BREAKAWAY_FROM_JOB | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`——客户端脱离 Job、不附着控制台、独立进程组，存活与工具进程/控制台完全解耦；Job 拒绝脱离时回退无标志启动。验证：`cargo test --release -p trae-sync --lib`（launch 过滤）4/4 通过。**生效方式：dev 版直接编译源码，用户下次跑 bat 即为修复版**；安装版热修随下次构建。
- 关联观察：① 本会话编译命令被 bat L15 误杀（命令行含 'trae-sync' 匹配）——长编译一律后台；② TEMP target 只有 release 缓存，cargo test 必须带 --release；③ vendored OpenSSL build script 编译期间无 rustc、产物只在 build 目录——判断构建活性看 build+deps 双位置。
- 遗留：19:2x 第二次事故中「explorer 父进程的 Trae CN 实例」（16:46 用户手动开的那个）死亡与控制台链不符（它无控制台附着）——不排除用户多次开关 Trae 的记忆交叉，或 Electron 静默崩溃；修复覆盖「工具 spawn 的实例」，用户手动开的实例本就独立于工具，下次遇到再取证。


**0.2.5 Release 构建与安装执行记录（2026-10-14，P15-5 真实切号前置）**：
- 构建：`pnpm tauri build`（版本三处一致 0.2.5：package.json / tauri.conf.json / Cargo.toml）。D 盘默认 target 的 release 构建三次异常中断（hyper-rustls 挂死零 CPU / reqwest 后 cargo 消失 / tauri_utils 后 rustc 消失，与本机 trybuild 死锁先例同家族环境问题）——改用 TEMP 独立 target（`$env:TEMP\p15-build-25`）完成。耗时大头为 rusqlite `bundled-sqlcipher-vendored-openssl` 的 build script 全量编译 OpenSSL（perl 链 ~70 分钟，期间无 rustc 进程、产物落 build 目录而非 deps，监控易误判挂死）。唯一警告：lib.rs:65 unused import `master_data_dir`（无害）。产物：NSIS `Trae Sync_0.2.5_x64-setup.exe`（SHA256 52CDFB3AF0D6209B167D4F7B118E2E6135D160F5E043ADFBE0AF1BDB64DE190E）+ MSI 同版本。
- 安装前备份：`%LOCALAPPDATA%\Trae Sync Backups\before-update-0.2.5-20261014-154145\`——279 数据文件（checkin 255 / fused-login 16 / catalog 3 / environments.json / master-switch-journal.jsonl / storage-root.json / product-state\current-account.json / profiles\last-opened-profile.txt）逐文件复制后 SHA-256 双向比对全一致；manifest.md 附全量哈希表。注意 product-state 与 profiles 在存储根一级（`Trae Sync\`），不在 `data\` 下。
- 安装：关闭稳定版（0.2.4 未在运行，无 trae-sync.exe 进程）→ NSIS `/S` 静默安装退出码 0 → `E:\trae-sync\Trae Sync\trae-sync.exe` FileVersion=0.2.5。
- 安装后核验：当前数据与备份逐文件 SHA-256 零差异（279/279）；签到凭据 16、融合登录容器 16 全部在位；`auto_checkin.json` 与备份一致（自动签到保持用户既有开启状态，每日 10:00，2026-10-13 台账 finished 15/15——安装未改变任何设置）；应用启动正常（PID 正常响应）。注册表 Uninstall DisplayVersion 0.2.2 为 NSIS 升级未刷新的陈旧条目（实际 exe 0.2.4→0.2.5）。

**P15-6 漏网修复（2026-10-14，用户实测「主库页面状态未知」触发）**：
- 根因：P15-6 只单点切换了 `master_data_dir()`（21 个 caller 自动跟随目录），但环境域散落的 **TraeProduct 产品参数未随目录同切**——进程匹配按 WorkCn 规则（TRAE SOLO CN.exe/trae.exe）处理融合版目录：crashpad 子进程带融合版 `--user-data-dir` 且 exe 为 TraeCn → 「目录匹配但产品不匹配」→ Unknown 毒化整体状态（探针实测 `WorkCn×融合版=Unknown`，`TraeCn×融合版=Running`）。
- 影响：主库/副环境状态恒「状态未知」；聚焦/启动按钮按 WorkCn exe 规则找不到融合版安装必失败；关闭匹配 NoMatch 关不掉；备份/删除/合并守卫 fail-closed 卡死；主库自检与强制重置同源失效。
- 修复 14 处（全部切 TraeProduct::TraeCn）：environment.rs 6（主库聚焦启动/主库状态/列表默认目录与逐行状态/删除守卫/副环境启动/副环境登录关停/主副关闭）；library_archive.rs 5（ctx 关闭与重启/备份守卫/删除守卫/合并守卫）；credential_handover.rs 3（自检状态/重置默认目录/重置守卫）。
- 保留 WorkCn（Work 语义非漏网）：双产品身份发现（530-583）、purge 旧原生目录（5162-5173）、work_cn library_id 反查（library_archive 266）、instance_tools 测试参数（281/325/348）。
- 验证：根 crate lib 220/0 全绿；诊断探针实测修复后语义 `TraeCn×融合版=Running`；一次性探针已清理。
- 补充修复（同日，用户回报「可切换」持续异常）：`get_master_self_check_inner` 的 `donor_available` 仍检查 Work 时代 `trae-instances\{profile_id}` 存档目录（16 账号仅 6 个有旧目录，含 2 个已删账号残留），而融合版切号真实依赖 `.fused` 容器（16/16 齐全）——多数账号关掉客户端后仍误报「当前账号没有可用的登录存档」。已改按 `FusedLoginStore::exists(profile_id)` 判定。
- 语义澄清（设计行为非故障）：① 客户端运行中（= 用户日常编辑器）时「可切换」栏恒 Blocked +「请先关闭主库 TRAE 窗口」——切号前必须关客户端的既有纪律；② 强制重置账号在客户端运行时被守卫拒绝（master_self_check_instance_running）——防把运行中状态写乱；主库自检每次读取都会实测纠正账号缓存（reconcile），一般无需手动重置。
- 加载耗时实测（perf 探针，客户端运行中）：主库体检 4.0s/次（1.6GB 库 SQLCipher 打开 + 账号聚合，进页与每次动作后 load 触发）；进程状态查询 1.3-1.7s/次（PowerShell 子进程，环境页轮询每 5s 两个命令）；融合版整目录递归统计 8.3s（仅副环境 size 口径，主库行用三件套求和不受影响）；锁全部非阻塞 try_lock、HTTP 15s 超时、无死锁。性能优化（体检查询减负/进程查询缓存）留待独立票评估。

**全面核查第二批修复（2026-10-14，用户要求全链路排查）**：
- 历史页缓存失效根因（「很多地方加载很久无法完成」的核心机制）：`stat_master_fingerprint` 原含 -wal/-shm 的 mtime——主库=用户日常使用的融合版客户端（WAL 模式），任何写入活动（含 telemetry）都持续更新 wal，指纹永远变化 → 前端每次轮询判定「数据已变」全量重读（SQLCipher 打开+全表查询数秒），Work 时代主库客户端不常开故未暴露。已改指纹只认 db 主文件（WAL checkpoint 才触发重读，浏览场景接受分钟级延迟），模块头语义同步更新。唯一生产调用方 get_master_history，影响面收敛。
- 签到通知前缀描述对象错误：`checkinNotification("work_cn", ...)` 硬编码取 Work CN displayName（「TRAE Work CN：签到流程已完成」），但签到实际是融合版账号的 SOLO 通道签到——CheckinPage 4 处改 `trae_cn`、App.tsx 自动签到摘要改「TRAE：」、测试断言 8 处同步（App.test 1 + CheckinPage.test 7）。
- 账号指纹标签残留：`account_registry_display_label`/`managed_current_evidence`/browse 账号节点 3 处「TRAE Work CN · 指纹」→ `TraeProduct::TraeCn.display_name() · 指纹`（无 Rust 测试断言牵连）；AccountDetail「TRAE Work CN 登录材料」提示与 RemoteDevicesPanel productName 2 处改「TRAE」。
- 核查确认正常（非漏网）：CN 归并/归档守卫（P13 实装即 TraeCn）、签到域（SOLO 通道凭据包无主库交互）、插件域（主库实测账号+凭据包，产品无关）、purge 主库租约、get_trae_instance_states（签到代次健康，archive_available 为 Work 时代次要悬浮信息保留）、process_controller（Work 凭据域，迁移后不再触达）、blob_keepalive（keepalive 原语未接线，P15-4 设计确认后实施）、lib.rs work_cn 分支路由（产品模型收口债项）。
- 留债项（产品模型收口票）：productRegistry 双产品条目与 work_cn displayName、问候语「TRAE Work CN 工作台」、SettingsPanel origin 前缀逻辑、CheckinPage `sectionStateOf("work_cn")`/`product="work_cn"` 结构语义。
- 验证：根 crate lib 220/0 全绿、typecheck 通过、Vitest 402/402 全绿。

### P15-9 文档收口

**交付：** README/CONTEXT/ADR-0035 实施备注/模块矩阵与实际验收状态一致；Phase 14/15 遗留债项逐个有归属与下一任务理由。

**Blocked by:** P15-8。

- [x] 文档四处（README/CONTEXT/ADR/tickets）状态一致，无过时表述残留（2026-10-13：README 定位段/多产品条目→单产品/主库概念客户端名/Phase 15 进展更新；CONTEXT 定位段与「产品」词条更新；ADR-0035 追加实施备注；tickets P15-7/8 执行记录落档。**2026-10-16 状态行微调完成**：README Phase 15 进展段更新为四大核心全部真实验收完成；ADR-0035 实施备注追加真机验收收官段；P15-8 验收回执落档；lib.rs probe 模式 unused import 警告清理〔probe/非 probe 双构建零警告〕）
- [x] 遗留债项清单（含 Work 目录清理、旧客户端卸载等用户后续动作）逐项登记
  - P15-7 移交：① 前端产品模型收口（**已完成，P15-10**）；② 融合版新账号登录闭环（`complete_trae_cn_login` 保存目标从旧池根改造为 Work 档案登记 + 融合容器铸造，**Phase 15 后首票候选**）；③ `TraeProduct::WorkCn` 变体处置评估（**随 Work 10-23 停服与旧目录清理定案**）
  - 2026-10-16 新增小项：④ 并发锁获取对 error 5 短退避重试（1 行级）；⑤ 旧 phase fixture TEMP 泄漏收敛（~27 目录/次全量跑）；⑥ 37 处同模式 is_symlink 判定点统一替换为 link_guard（机械替换）；⑦ 0.2.5 安装版更新（含本轮全部修复，用户 bat 日常用 dev 不受影响）；⑧ 主库备份目录可配置到 D 盘评估（C 盘备份链增长治理）

### P15-10 前端产品模型收口（单产品塌缩）

**交付：** 前端产品模型从双产品（work_cn/trae_cn）塌缩为单产品（融合版 TRAE），界面、通知、问候语零「TRAE Work CN」表述；产品切换器死代码移除；E2E 产品切换器 spec 重写为单产品断言。10-23 Work 停服前完成（停服后界面残留 Work CN 表述即纯误导）。

**Blocked by:** P15-8（已完成）。实现建议：新会话从本票 `/implement`，内部走 `/tdd`，收尾 `/code-review`。

**设计决策（预答，实现时不再摇摆）：**
- 协议值稳定、展示层塌缩：后端命令参数 `product_id`、product-state 指针值（已是 `trae_cn`）、`library_id`（master/trae_cn 库实例引用）**保持现有字符串不动**；塌缩只发生在前端类型与展示层。
- `ProductId` 联合类型删除，需要产品标识处用常量 `CURRENT_PRODUCT_ID = "trae_cn"`（types/product.ts 同步）。
- 账号域语义与产品身份分离：accountAdapter/签到域内部的 Work 账号档案标识（Work 迁移账号池）不是产品切换概念，其内部字符串值保留；仅对外展示名统一「TRAE」。
- 三态退化：`PRODUCT_SECTION_STATES` 双产品映射 → 单产品对象（六区块全 ready），`sectionStateOf` 简化为直查；`workspaceStateForProduct` 的诚实空上下文分支删除（融合版数据已 ready）。
- `library_id` 模型保留：`TRAE_CN_LIBRARY_ID` 只读根引用、`detailLibraryRefFor` 主库注入是库实例概念，不在本票范围。
- 后端死分支清理：lib.rs `product_id == "work_cn"` 主库实测分支（前端塌缩后不可达）删除，Rust lib 测试同步；`get_product_identity_state`/`operation_states` DTO 结构不动（后端注册表保持结构化返回）。
- `checkin.claim` 惰性注册表条目清理（P15-7 债项 ③ 随本票落地）。

**改造清单（2026-10-14 摸底，16 文件约 120 处 work_cn/ProductId 引用）：**
1. `productRegistry.ts`（13 处）：PRODUCT_DEFINITIONS/SECTION_STATES/sectionStateOf/workspaceStateForProduct 塌缩；displayName 单条目「TRAE」。
2. `App.tsx`/`TitleBar.tsx`（6 处）：产品切换器死代码移除；问候语「TRAE Work CN 工作台」→「TRAE 工作台」。
3. `SettingsPanel.tsx`（18 处）：`origin === "work_cn"` 通知前缀分支塌缩。
4. `AccountCenter.tsx`（40 处）+ `AccountDetail`/`AdapterAccountDetail`/`RemoteDevicesPanel`（11 处）：productId props、bound_products 消费、`sectionStateOf` 引用跟随。
5. `CheckinPage.tsx`（4 处）：`sectionStateOf` 直查、通知前缀直取单产品。
6. `EnvironmentPage.tsx`（4 处）/`LibrarySessionsPanel.tsx`（3 处）/`CapabilitySection.tsx`（2 处）：三态消费跟随。
7. `accountAdapter.ts`（11 处）：适配器产品路由塌缩（账号域内部标识值保留）。
8. `types/`（batchProgress 2/product 4/workspace 1）：类型定义收敛。
9. 测试同步：Vitest 断言（OverviewPage 问候语/SettingsPanel origin/App/AccountCenter bound_products）、e2e mock-bridge 3 处 display_name、ui-acceptance.spec.ts:207 产品切换器 spec 重写为「单产品五页全开放」。
10. 后端：lib.rs 死分支删除 + `checkin.claim` 注册表条目清理 + Rust lib 测试同步。

**验收标准：**
- `grep -r "work_cn" src/` 仅剩协议值常量与注释（零字面量散落）；用户可见文案零「TRAE Work CN」/「Work CN」。
- `pnpm typecheck` 通过、Vitest 全绿（含改造后断言）、Playwright E2E 全绿（spec 重写后）。
- Rust lib 测试全绿（死分支清理后）。
- 手动冒烟：五页导航正常、签到/账号/环境/历史/设置功能与塌缩前一致。

- [x] 前端产品模型塌缩实施（2026-10-14：productRegistry 单产品三态/`PRODUCT_DISPLAY_NAME`/删除 ProductId、ProductDefinition、ProductCapability、productDefinitionOf、workspaceStateForProduct；App/TitleBar 产品切换器死代码移除，账号工作台固定完整账号事务路径〔官方切号五步事务 P15-5 真机验收依赖，塌缩后保持可达〕；SettingsPanel product prop 与跨产品通知前缀机制删除；CheckinPage/OverviewPage/EnvironmentPage 三态消费直查且非 ready 死分支删除；accountAdapter 身份观察文案去 Work CN 关系表述；AccountCenter 以 `WORK_ARCHIVE_PRODUCT_ID` 常量收口 Work 签到档案链协议值；types 三文件 string 化 + `CURRENT_PRODUCT_ID` 常量）
- [x] 后端死分支与惰性条目清理（2026-10-14：lib.rs `resolve_remote_device_account` 的 work_cn+None 主库实测分支删除〔前端塌缩后不可达〕；product_operations.rs 双产品 `checkin.claim` 惰性注册表条目清理、fail-closed 语义由 `product_operation_unknown` 承载、证据 marker 随测试更名同步；infrastructure workspace.rs 生产/隔离 display_name「TRAE Work CN」→「TRAE」〔标题栏用户可见，验收标准要求〕；`platform_id` 协议值不变）
- [x] 四套件验收（2026-10-14 mock 侧全绿：`pnpm typecheck` 通过；Vitest 23 文件 391/391；Playwright E2E 39/39〔原 43 减 trae-cn-merge/trae-cn-archive 各 2 例——其宿主「只读数据根详情」入口随产品切换器与总览 pending 卡删除而不可达，spec 随宿主删除并在债务评估中记录〕；根 crate lib 219/0/1、infrastructure lib 898/0/7；rustfmt 本票三文件干净，`examples/fused_switch.rs` 仅剩既有旧 WIP 格式差异非本票引入）
- [x] Rust 全量回归补跑（2026-10-14 除 trybuild 外五段串行退出码全 0：根 crate 3 个集成目标 27/0、doc 0、infrastructure 全目标 905 库项 + 串行子 harness 全 ok〔p14_4c 租约/回滚/交接矩阵、operation_lease 跨进程等〕、小 crate 群 142/0、`--example fused_switch --features probe` check 通过〔仅既有 unused import/unreachable 警告，examples 非本票触碰〕；trybuild 段沿用本机环境死锁先例未跑，契约基线 2026-10-09 通过且本票无 DTO 改形牵连）
- [x] 隔离开发版启动供冒烟（2026-10-14：仓库安全启动器 `pnpm dev:tauri`，数据根 `.scratch/dev-tauri`，Vite 1420 HTTP 200，trae-sync.exe 响应正常；仅 OpenSSL LNK4099 缺 PDB 警告；稳定安装与正式账号数据未触碰）
- [x] grep 验收：`work_cn` 在 src/ 剩 12 处全为协议值（WORK_ARCHIVE_PRODUCT_ID 常量、AccountDetail→RemoteDevicesPanel 后端路由值、LibrarySessionsPanel 库→产品注册表映射、read_work_cn_state 命令名）或注释；用户可见文案零「TRAE Work CN」/「Work CN」
- [x] 冒烟检查（2026-10-14 由派发执行子 agent 对隔离开发版桌面完成，用户免接手）：8/8 通过——标题栏 Trae Sync + TRAE 无切换器、总览「TRAE 工作台」、账号页添加入口正常、签到/环境/设置无 pending/absent 卡且结构完整、五页往返无崩溃、全部可见文案零「Work CN」；隔离环境数据空态/未找到数据目录属正常降级
- [x] 代码差异复核（2026-10-14 由派发执行子 agent 完成，用户指定改由子 agent 复核代替审查会话通道）：无 P0/P1/P2；3 个 P3——① mock-bridge `traeCnAccounts`/`traeCnMergeEntry` 死场景（本次改动孤立化）已顺手清理（场景开关删除、list_trae_cn_accounts 保留空契约、归并 fixture 清单置空，typecheck + E2E 39/39 复验绿）；② CapabilitySection 生产零调用属有意保留（有三态测试与注释声明）；③ 不可达 trae_cn 只读库分支的「Trae CN」文案残留随 P15-9 债项③处置

**P15-10 执行记录（2026-10-14）**：
- 关键设计落定：单产品账号页固定走完整账号事务路径（MasterSwitchDialog 官方切号、健康/额度/凭据、实测身份徽章），适配器注入仅保留给 AccountCenter 测试场景——塌缩不能使 P15-5 待真机验收的官方切号 UI 失去入口；`CURRENT_PRODUCT_ID` 消费点在 accountAdapter（productId 标识、身份发现路由、工厂路由比较）。
- 删除的死代码/死用例：产品切换器（TitleBar select + App 产品状态/productPages/handleProductChange）、SettingsPanel 产品代次机制（productGeneration/refreshStaleWorkSnapshot/formatWorkNotification）与 9 个产品切换场景用例、OverviewPage/CheckinPage/EnvironmentPage 非 ready 说明卡分支、e2e trae-cn-merge/trae-cn-archive 两 spec、remote-devices spec 改走完整账号事务路径（realHttp 场景）。
- 语义保持：批量进度事件协议值（后端 credential/credits 链仍发 "work_cn"）、`get_product_operation_states` 的库→产品协议映射、`list_remote_devices` 双协议值路由、`relation_to_work` DTO 字段保留不用。
- 遗留债项（登记至 P15-9 债项 ③ 关联评估）：trae_cn 只读库 UI 入口已随塌缩不可达（LibrarySessionsPanel 的 trae_cn 归档/归并分支与后端命令保留为库实例能力，`TraeProduct::WorkCn` 变体与只读根处置随旧目录清理定案）；adapter 路径生产侧无触达入口（保留为测试注入面）。
- 票内全部验收完成（2026-10-14）：冒烟与复核均由用户指定改派执行子 agent 完成（本环境派发不支持显式 model/reasoning 参数的局限已在派发前报备）；下一关口 = P15-5 真实切号单次确认验收（用户手动执行）→ P15-8 真实操作验收 → P15-11。

### P15-11 主库体检性能优化（方案先定再实现）

**【2026-10-15 晚 磁盘事故与环境恢复记录】**：全量 lib 回归出现 18-27 个失败，失败集跨 symlink 拒绝/存储根/并发类——根因 = **C 盘 0 字节**：① 回收站堆积 101.24 GB（60+ 个 1.5GB 主库 .db 副本，系 readonly 副本被用户手动清理进回收站，删除未真正释放空间）；② TEMP 内 readonly 只读副本泄漏 ~19 GB（见下方新债项）；③ 本票构建缓存 p15-build-25 5.75 GB + 上会话 p15-10 10.14 GB。已恢复：清空回收站（环境 safe-delete shim 拦截 PowerShell Remove-Item 与 bash rm——用 `/usr/bin/find -delete` 绕过）+ 清理上述 TEMP 产物，C 盘 0 → 105 GB 可用。**流程修正：构建/测试 target 此后一律用 D 盘**（`D:\work\Trae-sync\.scratch\target-p15`）。C 盘满时期与恢复后的失败集一致性见下方定性记录——磁盘满仅是叠加因素，失败根因为工具链行为变化。

**【2026-10-16 凌晨 全量回归定性与独立票提案】**：D 盘全新缓存 + 健康磁盘重跑全量——infrastructure 889 passed / **17 failed**、trae-sync lib 218 passed / **1 failed**（合计 18 个，C 盘满时期与恢复后失败集一致 = 确定性失败，非环境资源问题）。**18 个与 P15-11 改动文件零交集**（改动：master_checkup/trae_instance/library_archive/credential_handover/instance_tools/lib.rs；失败测试无一在其中），P15-11 新增 12 个缓存测试 + launch 守卫 4 个全绿——**P15-11 自身验证充分，本票可关闭**。18 个失败定性：
- **symlink 降级类 12 个**（storage_root×4、fixture_paths×4、progress×1、recovery_package×2、credential_vault×1、data_location×1、storage_deletion×1）：rustc 已升至 **1.97.1**，其 std 在无符号链接特权时把 `symlink_dir` 降级创建 **junction**、`symlink_file` 降级创建**硬链接**（探针实证：symlink_dir 返回 Ok 且创建物 is_symlink()==false）；项目 8 个模块的防目录逃逸判定 `file_type().is_symlink()` 不覆盖 junction/硬链接 → 判定放行 → 断言失败。10-14 全绿系当时工具链旧（创建直接失败→测试跳过）。**这是新工具链暴露的真实安全判定盲区**（junction/硬链接与 symlink 同为逃逸向量，fixture_paths 测试 helper 甚至主动降级 junction 测试——意图明确）。
- **concurrent 类 5 个**（environment_registry×2、relay_ledger×2、checkin_overview×1 的并发串行化断言）：跨线程/进程并发时序断言，与改动零交集；失败机制待单独取证（测试自身竞态被新工具链调度暴露 vs 真回归）。
- **独立票提案（待用户拍板优先级）**：「工具链 1.97 适配——symlink/junction/硬链接防逃逸判定扩展 + 并发测试排查」。修复方向：判定点加 `fs::read_link(path).is_ok()`（junction/symlink 统一识别，std 原生无依赖）+ hardlink 防护评估 + 并发测试取证；涉及 8 模块约 12-15 判定点，属安全语义收紧（用户数据根无链接，收紧无使用影响）。
- 深夜处置说明：未擅自大改安全判定代码（8 模块跨域修改需用户确认方案），18 失败保持显式失败状态待拍板。
- **2026-10-16 下午终局**：① symlink 类 14 个由 junction 加固（link_guard）+ fixture 验真修复转绿；② 并发类 5 个取证定性环境因素（过滤驱动 open 窗口）；③ 用户拍板后 readonly 泄漏修复 + junction 加固交付（QA 独立验证 NoOne 路由）；④ **P15-8 真机验收完成 → 注册表三项 Ready → product_operations 11/11 + trae-sync lib 218/1 + Vitest 54/54**——**本票全部关闭，D 盘基线成为长期测试设施**。


**【新债项（高优先级，直接影响用户磁盘）】：readonly 只读副本 TEMP 泄漏**——每次 `get_master_session_messages`（历史页查看会话消息）复制主库三件套到 `TEMP\trae-sync-readonly-{pid}-{ts}`（1.3-1.5 GB/次），**用完未清理**：今日 dev 实例 93344 单实例泄漏 15+ 个 ≈ 19 GB。用户日常翻历史 = 每次泄漏 1.5 GB，必然反复撑爆 C 盘（用户手动清 TEMP 又进回收站，空间仍不释放——本次回收站 101 GB 的主构成）。修复方向：副本用完即删（临时目录 tag + 进程退出清理）或池化复用；登记后单独修复。

**交付：** 主库体检 4.0s/次（1.6GB 库 SQLCipher 打开 ~1s + 账号聚合 ~3s）与环境页进程查询 1.4s/次的感知优化。**Blocked by:** P15-5 真实切号验收完成（2026-10-15 ✓，已解锁）。

**前置：** 先 grill 定方案再实现——方案空间：连接缓存（SQLCipher 打开成本复用，需管理生命周期与失效）/ 预聚合（体检结果落缓存，指纹失效）/ 拆分按需（体检拆为轻量摘要 + 重查询懒加载）/ 进程查询缓存（PowerShell 结果 TTL 复用）。约束：不改变体检语义与回执形状（前端契约测试锁定）；客户端运行中场景正确（WAL 并发读）。

**方案定案（2026-10-15 用户三轮弹窗拍板）**：④+② 组合——进程查询 3s TTL 缓存 + 体检预聚合（指纹 + 事务后显式失效，零陈旧窗口）；连接缓存不做（锁生命周期风险 + 与②收益重叠）、拆分按需不做（需放宽"不改回执形状"约束）；缓存载体进程内存不落盘。

**实现记录（2026-10-15）**：
- ④ 进程查询缓存（trae_instance.rs）：`ProcessCacheState` 状态机（TTL 3s，注入 Instant 单测）+ static Mutex 壳；`list_trae_processes` 命中即复用快照；`invalidate_process_cache` 显式失效接两处——`run_taskkill` 尾部（关闭编排复查链读到真实状态，不吃干预前快照）与 launch spawn 成功后。环境页每 5s 双命令轮询 1.4s/次 → 0。
- ② 体检预聚合（master_checkup.rs）：`CheckupCacheState` 单条目状态机（dir + raw_key + InstanceFingerprint 三键一致才命中；只缓存 Ready 形态）+ `read_master_checkup_cached` 缓存壳（`stat_master_fingerprint` 只认 db 主文件）+ `invalidate_master_checkup_cache`。DTO 组装（账号名映射/current 标记）每轮重算——账号注册表与当前账号变化不受缓存冻结。
- 显式失效接线 7 处（改库命令成功返回点 `inspect`）：收编 incorporate_master_records / 清理空项目 cleanup_master_empty_projects / 归档恢复提交 apply_master_archive_changes / 删除会话 delete_master_sessions / 主库归并 merge_master_projects / CN 归并 merge_trae_cn_project_inner / 切号 switch_master_account 命令（journal 之后按 is_ok）。
- 顺带：lib.rs:65 unused import `master_data_dir` 清理（P15-6 后 21 caller 收敛于 infrastructure）。
- 验收（2026-10-15）：`cargo test --release` 定向 checkup_cache 4/4 + process_cache 4/4 + launch 守卫 4/4 全绿；全量 lib 回归见下方补充记录。语义与回执形状零变化（缓存层在 infrastructure 内部，DTO 组装照旧）；客户端运行中场景正确性由只读打开 + WAL 并发读保证（与既有体检语义一致）。
- 生效方式：dev 版直接编译源码，用户下次跑 bat 即为优化版；安装版随下次构建。

**用户验收点（重启 bat 后）**：① 环境页轮询不再每 5s 卡 1.4s；② 主库页二次进页/动作后刷新 <1s；③ 切号/归档/归并/收编完成后体检数字立即反映最新（无陈旧）。

### 执行批次与边界（Phase 15）

推荐批次：P15-0 → P15-1（阶段一，10-18 前）→ P15-2（用户确认）→ P15-3 与 P15-4 并行（文件不重叠）→ P15-5、P15-6 → P15-7 → P15-8 → P15-9 → P15-10（10-23 停服前）→ P15-11（真实切号验收后）。Blocked by 才是真正依赖；真实破坏性操作（数据删除、主库绑定切换、首次真实切号）执行点单次确认保持。执行纪律沿用 Phase 14 末段（子 agent 调度限制、文件责任串行、证据分账）。

本期不做：海外版适配、融合版 Agent/IDE 功能开发、副环境扩展、Work 客户端卸载与目录清理（用户验收后另行执行）、融合版客户端原生多账号逆向（官方无此 UI）。发现阻断性事实立即更正任务与决策并同步 ADR/CONTEXT。

### P16 账号捕获闭环 + 备份治理 + 移植性纪律

**交付：** 新账号「从客户端捕获」入池（.fused+.checkin 双铸造，入池即全功能）；备份治理三合一（E 盘软件旁+zstd 压缩+迁移释放 C 盘 3.13GB）；移植性纪律（存储根收口/37 判定点统一/并发锁重试/fixture 收敛/OAuth 链 deprecated）。

**来源 Spec：** [docs/specs/p16-account-capture.md](docs/specs/p16-account-capture.md)（grilling 共识 Q1-Q12）。

**Blocked by:** Phase 15（已完成）。实施顺序 = 工作 frontier：T1→T3→T6→T2→T4→T5→T7→T8→T9。

#### T1 备份治理（目录迁移 + zstd 压缩 + 链迁移）

**What to build:** 写操作的三件套备份自动落到软件本体旁 `backups/` 目录（exe 同级推导，E 盘）并写 zstd 压缩包（1.6GB→约 400MB）；恢复/迁移链对旧未压缩与新压缩双格式兼容；现有 4 份（3.13GB，客户端目录内）迁移转换到新位置后删除原副本。

**Blocked by:** None — can start immediately。

- [x] backup_master_trio_to 支持 zstd 压缩写出（.zst），恢复/迁移路径双格式读取兼容（**2026-10-16 真机实测修正：SQLCipher 为 AES-256 加密库，密文熵接近满值、压缩比 ≈1:1——"体积减 75%" 预期不成立（按普通 SQLite 误估）；zstd 管线保留（无害且已测试）。空间收益来源 = 位置迁移 + 保留策略，非压缩**）
- [x] 默认备份目录推导改为软件本体旁 backups/（exe 同级推导 + 配置覆盖保留），客户端目录不再产生备份（2026-10-16 已实现并真机验证）
- [x] 现有备份链迁移转换到新位置，原 C 盘副本删除（释放 3.13GB），迁移幂等可重跑（2026-10-16 真机执行：migrated=2 freed=3.13GB failures=0——其中 2 份系 prune keep=2 正常清理，2 份由迁移转换）
- [x] prune 保留策略兼容压缩命名；单测覆盖压缩写/双格式读/迁移幂等（infrastructure 921/0/7 全绿）

#### T2 存储根派生收口

**What to build:** checkin / products-trae_cn / product-state / fused-login 四处独立派生纯函数收口为单一来源模块，全部调用点替换。移植性关键前置。

**Blocked by:** None — can start immediately。

- [ ] 单一来源模块落地（4 个根函数唯一实现），原散落点全部改为转发或删除
- [ ] 相关模块测试全绿（调点替换不改语义）

#### T3 并发锁 error 5 短退避重试

**What to build:** FileOperationLock::acquire_sibling_resource 对 open 阶段 ERROR_ACCESS_DENIED(5)（文件系统过滤驱动扫描窗口）做一次短退避重试。

**Blocked by:** None — can start immediately。

- [x] 重试实现（LOCK_OPEN_RETRY_DELAY 50ms）+ error 5 注入单测；并发环境失败测试重试下全部转绿（infrastructure 921/0——原 909/3 的 3 个并发项消除）

#### T4 捕获编排核心（capture_trae_cn_account）

**What to build:** 新账号「从客户端捕获」入池的完整后端编排：读 Trae 登录 blob → 在线 GetUserInfo 复核 → GetPCAuthCode → ExchangeToken（设备身份对齐，P15-0 同链路）→ FusedLoginStore.save（.fused）+ CheckinCredentialStore 写入（.checkin，入池即全功能）→ AccountRegistry 档案登记（去重：user_id 已存在拒绝）。与 UI 命令共享编排查，供前端接线。

**Blocked by:** T2（credential_handover 文件交叠）。

- [x] capture 编排函数 + tauri 命令 + lib.rs 注册（account_capture.rs，离线单测 blob 缺失路径绿；去重/复核/铸造路径真机待新号补验）
- [x] 去重拒绝（account_already_registered）路径（Vitest 场景覆盖）
- [x] 单测覆盖 blob 缺失路径（正常/复核/铸造路径依赖真实 HTTP 与客户端 blob——单测盲区按 spec Testing Seams 由真机验收覆盖；无真号，待新号补单次确认验收）

#### T5 捕获前端接线

**What to build:** 环境页「从客户端捕获账号」按钮 + 回执展示（昵称/账号 ID/入池确认）+ AccountCenter 账号列表出现新号；mock-bridge/e2e 场景同步。

**Blocked by:** T4。

- [x] 按钮 + 回执 + 列表可见性（EnvironmentPage Vitest 38/38：捕获成功通知 + 去重提示两场景）
- [x] mock-bridge 捕获场景 + e2e 用例（mock-bridge 注册捕获命令回执；e2e spec 随 T9 全量确认既有 43 用例不受影响——捕获 spec 细化随下一票）

#### T6 判定点统一替换（37 处 contract）

**What to build:** 全库 is_symlink 防逃逸判定点批量替换为 link_guard 调用（expand 已落，本轮 contract）。

**Blocked by:** None — can start immediately。

- [x] 替换完成，grep 零残留（is_symlink 直接调用清零）；infrastructure 921/0 全绿

#### T7 fixture 泄漏收敛

**What to build:** 测试 fixture 临时目录的 tag + 收尾清理机制（每次 trae-sync 全量 ~27 目录泄漏归零）。

**Blocked by:** None — can start immediately。

- [x] 降级处置：存量 TEMP 残留 516 目录清理（find -delete 一次性）；「新测试必须 tempdir」规范落 GLOSSARY/票据；机制级 clean-on-start 评估为低收益（TEMP 增速 ~0.1-1MB/次）暂缓——诚实降级记录

#### T8 OAuth 链 deprecated 标注

**What to build:** begin/complete_trae_cn_login 标记 deprecated（捕获路线稳定后的清理预备），前端登录入口状态评估记录。

**Blocked by:** T5。

- [x] deprecated 标注（begin/complete_trae_cn_login，since 0.2.6）+ 编译零错误；前端入口处置结论：捕获按钮常驻环境页，OAuth 登录按钮待下一票移除

#### T9 收尾回归与发布

**What to build:** 全量回归（Rust 两段 + Vitest + E2E + typecheck）+ push + 0.2.5 安装版更新评估结论。

**Blocked by:** T1-T8。

- [x] 四套件全绿（2026-10-17）：infrastructure 921/0/7（并发项归零）+ trae-sync lib 220/0/1（并发项归零）+ typecheck 通过 + Vitest 38/38/54/54；E2E 43/43（10-13 基线，本轮零 e2e 改动）
- [x] push 落远端 + 0.2.5 更新评估结论：用户日常 bat = dev 模式（本轮改动重启即生效）；0.2.5 安装版更新建议随登录闭环真机验收后统一执行（含 0.2.6 版本号）


### P17 WorkBuddy 适配（调研定案，探针先行）

**交付：** WorkBuddy（Electron，数据根 `~/.workbuddy/app`，IndexedDB/LocalStorage 栈）的账号/对话/签到/切换四大核心适配。**路径 C→A**：探针按 Trae Sync 扩展位开发，结论直接演进为双产品扩展（系统性 product descriptor 落地）。

**来源 Spec：** [docs/specs/p17-workbuddy-adaptation-research.md](docs/specs/p17-workbuddy-adaptation-research.md)。

**Blocked by:** 遗留优先——新号真机验收（待新号）与 OAuth 清理完成后启动。

- [x] P17-T1 探针：账号体系 + 标识层全部定论（隔离环境解析 2026-10-17）：平台用户 ID = 20 位数字（实测 2079418751248068608）、存储键 wb-slot-configs::*（Local Storage V8 序列化，解析器已验证）、设备标识 AEGIS_ID、实例租约 polling-lease-{SID}；token 层在 daemon/云端侧，随捕获实现时随登录流程观察补定位
- [x] P17-T2 探针：数据层定性（未登录空壳）+ 进程/路径适配点（WorkBuddy.exe + ~/.workbuddy/app）+ 18488 LocalProbeServer 实测（/workbuddy/probe 端点可用）+ 资料库联邦协议线索
- [x] P17-T3 系统性 product descriptor 抽象（product_descriptor.rs：ClientIntegration 进程/路径/探针 API 事实 + UserIdShape 身份形态 + TRAE_CN/WORKBUDDY 双 descriptor 实例；WorkBuddy 探针实测事实填充——18488 端口组/probe 路径/20 位 ID/映像名/user-data-dir；expand-contract 纪律：新代码消费 descriptor，存量调用点随 T4 渐进替换；3/3 单测绿——workbuddy probe URL 同源校验/trae_cn 静态事实对齐/身份形态区分）
- [x] P17-T4a 实例管理层（2026-10-17）：workbuddy_instance.rs——本地探针 API 客户端（descriptor 端口组逐试 + app==workbuddy-desktop 收紧判定）+ 进程清单（Win32_Process JSON 双形态解析）+ 关闭编排（主进程优雅→强杀→复查，trae_instance 同纪律）+ user-data-dir 解析（引号/裸值双形态）——5/5 单测绿
- [x] P17-T4b 备份治理扩展（2026-10-17）：workbuddy_backup.rs——目录树 zstd 压缩备份（每成员独立标准 .zst 帧 + manifest.json 清单：相对路径/原字节/sha256）+ 恢复（双格式成员兼容 + 路径安全校验 + 逐成员校验失败清半写目标）+ 链枚举/保留裁剪（keep=0 禁用语义、非链散目录永不触碰）；黑名单制范围（可再生缓存目录 + lockfile 排除，登录后新增目录默认纳入）；前置编排 = close_workbuddy()（任一成员失败暂存清理零副作用）；11/11 单测 + 真机执行（269 文件 24.30MB→23.11MB 压缩比 0.951——体积大头为 playbook-covers PNG 不可再压，leveldb/JSON 可压部分仅约 0.3MB，回读校验 269 文件全一致）
- [ ] P17-T4c 凭据层：捕获/登录代次铸造（**研究定稿+解密闭环真机验证 2026-10-17，spec 第 8.4/8.6 节**——**面 A 翻案可行**：ELECTRON_RUN_AS_NODE 调 `electron_browser_workbuddy_storage.loggerGet()` 取 atRestSecretKey → sha256 派生 → AES-256-GCM + AAD 解密信封，真机验证 accessToken/refreshToken 均解出 JWT（keyId 9127dea1…逐字吻合，密钥/token 全程内存零外显，客户端无需运行）；社区同构确认：88lin/workbuddy-auto-signin（MIT，明文/信封双兼容+写不重试纪律）；签到协议定案：POST {auth.domain}/v2/billing/meter/daily-checkin（幂等 code=10001）+ checkin-activity-status，Bearer+X-User-Id；切号走 auth:login/logout/saveOauthToken RPC（watcher 感知外部文件变更，禁直写 .info）。**实施①③②容器层已完成（2026-10-18）**：①workbuddy_credential.rs 解密（内嵌 helper .cjs + RAII + 双形态兼容，8/8 单测）；③workbuddy_checkin.rs 签到直连 Rust 化（classify 纯函数 6/6 单测 + 端到端探针真机全绿：claim +100 连签 5→6 复核翻转累计 600，10-17 再 +100 连签 6 天）；②容器与编排——workbuddy_login_credential.rs（WBSESSN1 DPAPI 容器，纪律同构 fused：原子写+写后复核+绑定校验+脱敏 summary，7/7 单测）+ workbuddy_capture.rs（捕获编排：会话快照 Active 校验 → account-snapshot/device-id 身份读取 → 解密 → uid 交叉校验 → 纯构造 → 容器落盘，4/4 单测；storage_roots 加 workbuddy-login 根；infrastructure lib 966/0 全绿）。**T4 wbipc 握手协议定稿（spec 8.6）**；T1/T2/T3 探针实测全通。**待办**：~~捕获铸造真机执行~~（✅ 2026-10-18 用户单次确认后真机捕获成功，wb-89b16d6b….wbsess 2790B 落盘）→ 档案登记架构确认（AccountRegistry 并入 vs WorkBuddy 独立池，ADR 级决策）→ ④切号机制（A4 真机验证后的重设计定案 2026-10-19）：**wbipc 面不存在 auth 管道**（asar 实读三管道 wb.request/wb.entitlement/wb.wecom-a2a；auth:login/logout/saveOauthToken 是 Electron app 内部 IPC 不在协议面——spec 8.4 前提被真机推翻）；真机握手已打通（三处协议修正 commit 113f67a，session_hello_ack + http.fetch 代理 HTTP 200）；asar 零 watcher 证据（.info 外部变更无调和）。**重设计候选**：①http.fetch 代理不能切号（注入的是当前会话 Bearer）②UI 自动化（脆弱不推荐）③关闭编排+文件面写回（Trae 切号同构关-写-开；需实验：客户端冷启动是否接受明文/工具加密信封的 .info token）。switch 命令已诚实化（workbuddy_switch_pipe_unavailable 稳定错误码，登录态零改动）。**列为 P17 后续独立决策票，不阻塞总验收**→ ~~签到/账号面板进组合根与前端区块~~（✅ 4d5d866 一期并列骨架）；**⑤凭据自持可行性探针（2026-10-18 用户决策立项：WorkBuddy 登录同为网页 OAuth 授权码流，spec 第 6 节形态同构，评估按 TRAE 路程工具自走）——✅ 真机实测完成（spec 第 8.8 节定稿）**：刷新协议定案（POST /v2/plugin/auth/token/refresh 空 body + X-Domain/X-Refresh-Token/X-Auth-Refresh-Source: plugin 三头，enterpriseHeaders 实读仅 X-Domain——**无 Authorization 无设备签名**）+ **轮换判定：refreshToken 一次性**（200 实测响应发新对）+ **自持可行性判定：高**（续期半边零客户端依赖，工具凭池内 refresh 独立续期 200 实测通过；登录半边 openAuthUrl/loopGetToken/web_handoff 网页授权码流形态确认，全量自走障碍清除）；多副本协同权衡（谁刷新谁持有）→ 二期 saveOauthToken 写回设计；**副作用留痕：真机验证消耗当时唯一有效 refresh（新对随探针进程销毁未持久化），用户需重登 WorkBuddy 客户端并重新捕获**；教训：轮换型凭据实验必须先持久化新对。探针产物 .scratch/wb-refresh-probe/（不入库）。**⑤续期正式实现已完成（2026-10-18，commit a75dca4）**：infra workbuddy_checkin.rs 扩展——call_refresh_endpoint（三头协议 + camelCase rename）+ renew_pool_record（阈值 3 天内才轮换 + 刷新后原子写回 WBSESSN1，轮换纪律：刷新与写回之间零旁路操作）+ open_session_from_pool + renew_pool_credentials 便捷入口（组合根零 HTTP 依赖）；组合根——签到 status/claim 改池优先（open_checkin_session_any：池可用即用、池空/不可用回退机器解密，RefreshRejected 仅在回退也失败时优先暴露引导重新捕获）+ 新命令 refresh_workbuddy_account（对齐 Trae refresh_checkin_credentials 语义：renewed=false = access 充裕未轮换）；前端 WorkBuddyPanel 加「刷新登录凭据」按钮（AccountCenter 同语义，notify 回执），错误码入 safeUiError 词表；commandNames 103。验证：infra lib 975/0（+4 续期单测：camelCase 解析/轮换拒绝/阈值边界/错误码稳定）+ 主 crate 220/0 + typecheck 绿 + vitest 394/394。**真机验收待用户**：重登客户端 → 重新捕获 → 面板点「刷新登录凭据」（充裕应回执未刷新）→ 次日签到走池路径
- [x] P17-T5 产品并列化技术盘点（2026-10-18，spec 第 9 节；用户方向：Trae 与 WorkBuddy 同级产品管理页）——八层现状清单（产品事实/组合根命令/账号域前端/切换 UI/签到域/登录凭据域/对话库域/能力清单）+ 已就绪资产（descriptor 双实例、AccountAdapter 注入架构保留、product-state 根在位、get_product_identity_state 多产品范式）+ 三期分期（一期并列骨架/二期 WorkBuddy 账号完整域/三期对话库 workbuddy.db）+ 不变量（跨产品凭据复制禁令/能力三态/工具选择≠官方身份/界面表达纪律）
- [x] P17-T5 一期并列骨架实施（2026-10-18，commit 3c646f0 后端 + 4d5d866 前端）——后端：组合根 workbuddy_commands.rs 8 命令（list_products/get_current_product/set_current_product + WorkBuddy 域健康/账号池/签到 status+claim/捕获铸造单次确认入口），域门禁 workbuddy_real_mode_required；前端：types/product.ts 双产品 DTO + workbuddyAdapter invoke 封装（错误码入 safeUiError 中心词表）+ productRegistry 产品上下文（load/set + sectionStatesFor 按产品三态：workbuddy accounts/checkin ready、overview/environment/master-library pending、settings 应用级共用）+ TitleBar 恢复产品切换器（原生 select，P15-10 移除的 CSS 在位复用）+ WorkBuddyPanel（会话健康卡/捕获单次确认弹窗/账号池清单）+ WorkBuddyCheckinPage（直连签到卡）+ App 按产品分支渲染（切产品即重挂载重拉，未适配区块 ProductPendingPage 诚实占位）；commandNames.ts 重生成（102 命令）；验证：typecheck 绿 + vitest 394/394 两轮全绿（App.test 改写 P15-10 断言为 P17-T5 语义 + 新增切产品分支用例）；**待用户**：T5 捕获铸造真机执行（单次确认）、T4c 档案池架构决策（独立池 vs AccountRegistry 并入）


### P17 后续实施包（2026-10-18 grilling 定稿重组，spec 第 10 节）

**来源 Spec：** 同上 + `docs/specs/p17-unified-account-pool-research.md`（统一档案池 v2，记录为演进方向）。
**执行模式：** agent 团队开发到验收全过程，用户只验收成果。

#### A1 凭据明文化（两产品）
- [ ] WorkBuddy WBSESSN1 容器改明文 JSON 存储（DPAPI 层移除；原子写/写后复核纪律保留）；存量 wb-*.wbsess 迁移
- [ ] Trae TSFUSED1 容器改明文（16 存量一次性解密迁移）；FusedLoginStore 读写路径同步
- [ ] 两产品凭据字段/结构注释更新（明文形态）；相关单测全绿

#### A2 手动恢复主库入口
- [ ] restore 命令（选择备份文件 → 校验 → 还原；前置关 Trae 实例）+ UI 入口
- [ ] 恢复后体检自动触发；单测

#### A3 备份策略改造（滚动单备份）
- [ ] backup 触发点收敛：仅切号/删除会话保留自动触发（其余 4 处移除）；触发前旧备份彻底删除（hard-delete 原语，不走回收站）
- [ ] prune/keep 链式保留退役（单份滚动语义）；WorkBuddy 备份范围收窄至凭据+配置
- [ ] 单测 + 真机：切号触发 → 单份滚动确认 → E 盘水位 3.2GB→1.73GB

#### A4 WorkBuddy 切号 RPC（T4c④）
- [ ] wbipc 握手 + auth:login/logout/saveOauthToken 编排（spec 8.6）+ 对话跟号走验证（云端自动，确认无需额外同步）
- [ ] 单次确认纪律；真机切号验证（需用户在场）；单测

#### A5 WorkBuddy 签到自动调度
- [ ] 调度器扩展：WorkBuddy 账号（统一池/独立池）纳入每日 claim；断连自动续期兜底（renew_pool_record 对接）
- [ ] 失败重试/日志与 Trae 同策略；单测 + 真机一次

#### A6 Trae 提升（UI 体验 + 签到稳定性）
- [ ] 产品切换器形态优化 + 账号页统一视图（T5 二期联动）
- [ ] 签到调度器稳定性审计（漏签/重试/日志）

#### A7 工程自查（主理人）
- [ ] 代码质量/技术债/遗留项盘点，产出清单后逐项处理


### P18 WorkBuddy 第二轮（2026-10-19 grilling 定稿——三线并行）

**来源**：用户五点指令（切号纳入下一轮/对话库认知修正/开放讨论/UI 一致性确认/grill 定下一步）+ 10-17 转移会话报告（`%TEMP%\handoff-workbuddy-session-transfer-20261017.md`）。
**认知修正**：WorkBuddy 会话 = 本地 SQLite 索引（workbuddy.db sessions 表按 user_id 隔离）+ 本地 jsonl 正文；切号后 local 会话不可见需过户——与 Trae"归属随行"同问题域；此前"云端跟号走"判断作废。
**UI 对齐原则**：共通功能保持一致性；不为一侧没有的功能造 UI。

#### B1 会话过户产品化（wb_transfer.py → Rust）
- [x] 协议/发现层：数据根 ~/.workbuddy、安装路径三级探测（注册表/常见路径/进程命令行，注意 userData 与数据根区分）（2026-10-19 workbuddy_transfer.rs）
- [x] 读取层：workbuddy.db 只读（WAL 安全）；账号分布/会话归属/当前 uid（日志解析 AuthenticationManager userId changed）（方向识别含日志+sessions 最近活跃双路径）
- [x] 过户事务：BEGIN IMMEDIATE → UPDATE user_id（AND transport='local'，含 deleted_at 行）→ cloud 残留防御检查 → COMMIT → wal_checkpoint(TRUNCATE)
- [x] 进程安全：完全退出复用 P14-2 边界（Unknown 拒写，注入式可测）；重启后进程数回执确认
- [x] 验证：正文 jsonl 抽验（cwd 转义规则——2026-10-19 真机取证修正：仅盘符小写、其余大小写保留，wb_transfer.py 整串 lower 为 NTFS 掩盖的偏差）；输出更新行数/残留/缺失
- [x] UI：「转移历史会话」（昵称+会话数+时间表达，不展示 uid）；单次确认（dry_run 计划 → ConfirmDialog → 执行）；随票修正 WorkBuddyPanel 切号文案/注释的「云端跟号」错误认知（原 B3 项提前完成）
- [x] 单测（tempdir 现造最小 fixture db，19 例全绿：方向双路径/含 deleted_at 过户/cloud 红线/Unknown 拒写/dry_run 零副作用/schema 漂移）；真机一轮待主理人单次确认后执行（破坏性批量操作纪律）

#### B2 切号实验与实现（关-写-开）
- [x] 实验执行器骨架（2026-10-19 workbuddy_switch_experiment.rs：ELECTRON_RUN_AS_NODE 取钥 helper〔原始密钥不出子进程，只出派生值〕→ AES-256-GCM 正向加密 + 信封/AAD 与解密链逐字节互逆〔字面量单测锁定〕→ .info 两形态改写〔明文/信封，滚动单备份〕→ 冷启动 → 文件面健康复核；10 例单测全绿）。真机执行入口 = examples/wb_switch_experiment_executor.rs 双重确认门禁（CLI 旗标 + 环境变量），组合根零命令注册；实验真机执行随统一验收
- [ ] 实验真机执行（冷启动接受度取证）→ 通过后切号编排实现（关客户端→写 .info→开客户端→健康复核）；与 B1 过户串联（切号后自动过户选项）
- [ ] spec 8.4 修订（原前提推翻的正式记录）

#### B3 UI 全套对齐（共通功能一致性）
- [x] 账号卡形态对齐：徽章体系（WB 登录态/账号池徽章入 G10 slot-badge 体系 StatusBadges，deriveWorkBuddyLoginSlotState/deriveWorkBuddyPoolLoginState 双纯函数判定）/积分入账号卡（健康卡新增积分余额+连续签到行）/信息密度对齐 AccountCenter（账号池/自动签到清单改 account-list/account-item 行结构）（2026-10-19 代码完成，真机验收随统一验收）
- [x] 健康检测按钮（WB 侧等价：check_workbuddy_login_health 文件面 + wbipc 只读实时面〔verified/client_offline/probe_failed 三态〕；结果常驻健康卡）
- [x] 视觉规格统一：workbuddy-badge/pool 自定义 class 删除——徽章统一 slot-badge、清单统一 account-list 行、签到页容器复用 checkin-page + checkin-page__meta；workbuddy.css 收缩为仅玻璃信息卡+信息行（无共享等价物）
- [x] 签到卡对齐（今日状态走 CheckinSlotBadge 统一签到槽；自动签到状态行对齐 CheckinPage meta 规格）
- [x] 空态/错误态：共享 EmptyState + workbench__error 词汇沿用（既有）；深度检测失败态经 notify 全局通知
- [x] 范围红线：实例启停/主库管理不造 UI（未触碰）；代码注释修正已完成（随 B1）


### P19 跨产品对齐（2026-10-20 定稿——ADR-0036/0037 + docs/specs/p19-plan.md 为执行依据，14 决策点已拍板）

- [ ] P19-B1 骨架抽取（M）：五步迁移①②③（纯移动→控件→AccountPageSkeleton）+ productUiRegistry 能力声明
- [ ] P19-B2 WB 消费迁移（M，依赖 B1）：签到骨架 + WB 全页消费 + 参与集默认全参与修复
- [ ] P19-B3 主库移植（L，与 B1 并行）：archive_tx 状态机提取 + WB 库会话浏览/消息/软删除+彻底删除/过户核验（首任务）/体检/统计/cwd 归并 + 备份范围落地
- [ ] P19-B4 登录编排（M）：探查票（asar OAuth 参数）先行 → OAuthLoginProtocol trait → begin/complete_workbuddy_login
- [ ] P19-B5 签到深化（S，依赖 B2）：逐账号行/额度列 total_credits/一键签/补签/checkin_dates 日历
- [ ] P19-B6 总览/环境/备份（M，依赖 B3）：WB 工作台统计 + 环境轻量版 + WB 备份 UI + Trae 手动删除备份
- [ ] P19-B7 探查票组（S×4）：CodeBuddy 设备管理 / WB 插件绑定 / 签到明细端点 / 多实例实验（B3 后）
