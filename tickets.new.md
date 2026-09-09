# Trae Sync 实施计划（Phase 0-8）

> 2026-08-22 重建。依据 `docs/DECISIONS-20260822-GRILL.md` 与 ADR-0018/0019/0020。
> 旧 T01-T21 计划已于 2026-08-22 清理删除（git 历史可查；其中 T01-T14 已完成并发布 0.2.1，T15-T21 部分完成的资产折算入本计划）。
> 当前状态（2026-09-03 三次刷新）：Phase 0-3 全部闭环（P3-3 被废止）、Phase 4 被 Phase 5 环境模型取代、四大核心需求全部落地。Phase 5 全部完成（P5-9 备份保留策略与 P5-7 主库两层校验均于 2026-09-02 实施闭环；P5-A 日常观察为用户配合项）；Phase 6 全部完成（P6-4 环境管理 V2 于 2026-09-02 收官）；Phase 7 全部完成。Phase 8 UX 治理立项（2026-09-03，两轮 grill 25 项决策，P8-1 起实施）。

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
- **背景**：v5 自动重铸链在生产暴露三类问题——a) 9074 时间窗不可预测（盈国新账号实测：同 SOLO 形态同 180 秒铸签间隔，距登录 17 分钟拒、20 分钟过；冷却 3 分钟对新账号不成立），自动重试撞墙且每轮重铸消耗服务端设备配额；b) 冷却期 sleep(180) 阻塞持锁 + 前端倒计时为组件本地状态（退出页面即丢）+ 重复点击静默排队 → "一直签到中"；c) `credential_refresh_failed` 无文案映射落入"稍后重试"兜底误导用户（旧 Work 凭据账号实际出路是重新登录）。
- **目标**（grill 六项决议）：
  1. Work 通道代码彻底删除（`OAuthClient::Work` / `TRAE_CLIENT_ID` / `from_client_id` 未知回退），全链路仅 SOLO；
  2. 自动链整体取消：`RemintCheckinRunner` 退化为 status → 单次 claim → status 复核；删除自动重铸、退役设备自动恢复、冷却 sleep 与 `checkin-phase` cooldown 事件；
  3. 后端记录每账号最近设备重置时间戳（`AppState`，仅报错上下文用，不门控不自动重试）；
  4. 签到命令 try-lock 快速失败：执行中重复触发立即返回"签到正在进行中"；
  5. 失败文案按语义细分：9074 按重置时间上下文化（刚重置→引导等待；未重置→引导重置设备）、9095 设备日配额、`credential_refresh_failed` 引导重新登录、20401 设备数上限；
  6. 完整遥测头集合（trae-mate 式 `x-market-user-id`/`vscode-sessionid`/每请求 `x-request-id`/`x-tt-trace-id`/固定头）：探针实测通过后纳入 `checkin_http.rs`。
- **完成情况（六项决议全部落地，2026-09-02）**：Work 通道全删（`OAuthClient` 仅剩 `Solo`，探针/实测收编脚本同步改写）；`RemintCheckinRunner` 重命名为 `BatchCheckinRunner` 并退化为 status→单次 claim→status，自动重铸/冷却/退役恢复链全删；`AccountRecord` 新增 `device_created_at_unix_seconds`（登录与重铸两个写入点），9074 在设备铸造后 5 分钟内被拒时 detail_code 改标 `device_too_new`（用户4993529391 实测：登录后 192 秒同设备重试即过，证明首签失败是时间窗而非设备问题）；签到命令 try-lock 快速失败（`checkin_already_running`）；前端删除冷却倒计时 UI/监听（`checkin-phase` 仅剩 inter_wait）、`fallback_device_id` 字段与"已自动更换设备"文案全清；`safeUiError` 补 `checkin_already_running`/`device_too_new` 映射并改写 9074/9095 文案。**决议 6（遥测头）**：探针实测 PASS（status 端点 2 账号 × 3 组带头/裸头对比全部 200/code=0 且业务字段一致，报告 `.scratch/checkin-http/reports/telemetry-headers-probe-20260902-234021.json`），据此 `checkin_body` 统一附完整头集合（固定头 + 设备 ID SHA-256 确定性派生 `x-market-user-id`/`vscode-sessionid` + 每请求刷新 `x-request-id`/`x-tt-trace-id`），3 个形状/稳定性单测全绿。
- **验收**：Rust 全量测试 + 前端 typecheck/vitest/e2e 全绿；手动签到路径无任何静默等待（秒级返回或明确报错）；9074 场景文案按上下文给出正确下一步；探针头集合实测记录落档 `.scratch/checkin-http/reports/`。
- **实施注意**：取消自动链后 `RemintCheckinRunner` 相关 7 单测需同步重写；`restore_retired_device` 路径删除但 `checkin/retired/` 留档不删（铁律）；LY 账号凭据仍为 Work 形态，需用户重新登录一次换发 SOLO 凭据（非代码任务，提醒用户）。


---

## Phase 2：账号注册表与切换

### P2-1 账号注册表与卡片列表 ✅（功能已落地：注册表 + OAuth 登录 + DPAPI 凭据包由 P1-0 全账号真机闭环；卡片/列表/健康度由 U-2 重设计落地。残余项「主力账号 OAuth 注册」并入 P5-A 批次 0 用户配合步骤）
- **目标**：账号档案完整管理（OAuth 登录入口、列表、基础信息、积分、签到状态、凭据健康度）；现有 A/LY/梦梦重新授权。
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
- **E2E 验收（2026-08-23 computer-use 实测，UI + PowerShell 进程交叉验证）**：双实例并行启动（梦梦 + 用户4050081350，26 进程/2 独立目录）→ 关闭其一无"关闭失败"误报（修复前 taskkill /F 终止滞后 1~3 秒会误报，已加 3 次复查吸收）→ 强杀后 10 秒内重启成功（code.lock 复用无占用报错，证明实例目录可循环使用）→ 最终关闭全部清理（0 管理进程残留，用户原生实例全程不受影响）。附带确认：状态检测不误报原生实例（user-data-dir 前缀匹配生效）。

### P2-4 实例登录态可见性 ✅（2026-08-24 实施完成，同日升级四态 + 健康检测）
- **目标**：让"启动过实例但未在 TRAE 内登录"的账号状态可见（2026-08-24 诊断：梦梦账号 5 次启动均显示登录页，用户不知需手动登录一次；且 seed 的 NativeMissing 分支只报一次，storage.json 空壳存在后不再提示）。
- **完成情况**：`instance_login_state()` 纯函数（读实例 storage.json 判 `iCubeAuthInfo://usertag` 键，三实例实证：LY/import 有键=已登录，梦梦无键=未登录；解析失败按待登录处理，登录幂等无害）；`launch_trae_instance` / `get_trae_instance_states` 返回 `login_state`；账号卡片登录态徽章；启动消息按登录态分支（未登录给"在 TRAE 窗口内登录一次"引导）。用户在 TRAE 内登录后徽章 5 秒内自动翻转（轮询闭环）。
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
- **完成情况**：`StatusBadges.tsx` 全局徽章组件（签到槽三态 + 实例槽复合态，stale 停止态也亮琥珀加强——持续性异常下次启动需重新登录）；`AccountCenter` 双视图 + 排序 + localStorage 记忆（`accounts.view` / `accounts.sort`）；列表宽行（名字独占主列永不挤压）与卡片（三级层级）双布局；meta 行「手机号 · 令牌 N 天 · 设备尾号」（令牌 ≤7 天转琥珀）；详情页删除下沉独立危险区；`--accent` 全局切换靛蓝 + 新 token 层落地（styles.css 双层 token：新语义层 + 旧桥接层）。
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
- **真机端到端验收（2026-08-31 通过，CDP 驱动）**：梦梦 → 用户4050081350 五步事务全链路完成——弹层六阶段推进正常；备份链 `.switch-bak-*` 两份（create_new 语义验证）；TRAE 重启后 renderer.log 出现 `User info loaded {"userId":"1307767855650905"}`（互换凭据被服务端接受）；`project.user_id` 随行为 1307767855650905（inspect 探针只读核验）。**注意**：目标账号实例的登录 blob 会被 TRAE 清理（陈旧吊销），LY 实例 blob 已失效报 `switch_donor_login_missing`（文案与判定均正确），切换前需目标账号实例保持有效登录；本次改用 blob 有效的用户4050081350 验收。
- **边界说明**：`master_switch_conflict` 人工决策分支未真机触发（单测覆盖）；种子 DB 无会话，换腿与接力台账 0 条目路径未产生真机样本（逻辑由单测覆盖，待日常使用中积累）。

### P5-2 环境页 + 账号页切号主面板（Q6/Q7/Q9）✅（2026-08-31 真机验收通过）
- **完成情况**：
  - 环境页 `EnvironmentPage.tsx`：主库实例卡（启动/聚焦 `launch_master_library`、运行/登录态徽章、当前账号块、data_dir 足迹）+ 副环境占位（Q7 规划中说明，无可点入口）；页面可见时 5 秒轮询运行态，隐藏即停；未登录时展示首次登录引导文案。
  - 切号弹层 `MasterSwitchDialog.tsx`：消费 `master-switch-progress` 六阶段事件推进五步清单与总进度条；完成回执展示随行项目/交接会话数；`master_switch_busy` 走「等待完成 / 强制切换」分支（force=true 重发）；全量错误码独立文案（含 `master_switch_conflict` 人工决策指引）；运行中禁止点遮罩中断。
  - 账号页接入 `AccountCenter.tsx`：当前账号展示「使用中」胶囊（禁切换/禁删除），其余账号一键切换入口唤起弹层；完成后刷新环境状态与总览。
  - 导航 `NavigationRail` 增「环境」工作区（六个工作区）；样式 `environment.css` 全 token 化玻璃面板（与原型契约一致）。
- **验收（2026-08-31 回归）**：typecheck 通过；vitest 182/182（新增 EnvironmentPage 4 + MasterSwitchDialog 5 + App 导航六工作区断言）；cargo test 全绿；e2e 48/48。
- **真机验收（2026-08-31 通过，CDP 驱动）**：环境页启动/聚焦主库与徽章流转正常；切号弹层六阶段推进 + 完成回执（随行项目 1 · 交接会话 0）+ 供体 blob 失效错误文案均真机验证；账号页「使用中」胶囊切号后正确移至目标账号；环境页「当前登录」联动正确。已知小瑕疵：页面切回瞬间可能短暂显示旧状态（5 秒轮询/手动刷新兜底收敛），不阻塞验收。

### P5-3 历史页主库视图（Q4/Q5，替换 U-6 骨架）✅ 2026-08-31 完成
- **目标**：历史页改为主库数据源（项目左列 + 会话右列两栏，按 `project.user_id` 过滤当前账号）；接力轨迹徽章（会话行头像链，hover 全轨迹 = 接力台账聚合）；U-6 的账号中心骨架方案被环境模型取代，保留授权记忆化与增量扫描资产。
- **依据**：原型历史页 + 接力台账数据结构（`RelayLedgerEntry`）。
- **实现**：
  - 后端：`relay_ledger` 补 `from_session_id`（换腿后身份链回完整轨迹，旧文件兼容 + 环数据防御）；`master_history.rs` 读主库 project/chat_session（当前账号过滤 + 三件套 stat 指纹，unchanged 预检）；`get_master_session_messages` 主库消息流变体；lib.rs 注册 `get_master_history` / `get_master_session_messages` / `get_relay_ledger` 三命令。
  - 前端：`HistoryWorkbench` 全重写（两栏 + 搜索/时间范围筛选 + 只读预览弹层 + 接力徽章 fixed 定位悬停浮层视口钳制）；5 秒指纹轮询，unchanged 不刷新台账；类型 `src/types/history.ts`；样式 `history.css`（全 token 玻璃，聚焦合同不写 outline:none）。
  - 测试：vitest 9 用例重写；e2e mock-bridge 补 P5-3 命令边界（含三跳接力链 fixture）；`history-workbench.spec.ts` 重写、`ui-acceptance`/`layout-diagnostics`/`visual-contract` 同步两栏结构（旧授权/扫描 UI 断言全部退役）。
- **验收（2026-08-31 全绿）**：typecheck 通过；vitest 117/117；cargo test workspace 全绿（含 master_history 4 + relay_ledger 5 用例）；e2e 28/28；visual-contract 33/33。
- **真机验收（2026-08-31 通过，CDP 驱动）**：`get_master_history` 真机 ready，左栏渲染真实项目「assistant」（DB 只读核实：project 表当前账号 1 条、chat_session 0 条 → 右栏空态「当前账号还没有对话记录」为数据事实）；`get_relay_ledger` 真机返回 0 条（无交接历史，无徽章正确）。主库消息预览因真机无会话未能触发，逻辑由 e2e 覆盖（下次交接产生会话后自然补充实证）。
- **2026-08-31 裁定：缓刑观察（ADR-0021）**——单一归属模型下记录归属随行，切号/收编后 `project.user_id` 恒为当前账号，历史页「当前账号过滤」趋于恒真，账号区分视图失去意义。保留现有实现随日常使用观察：若长期恒真，后续任务移除 user_id 过滤与账号视角相关残留（含原型中的接力轨迹多账号语义评估），不在本期动。
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
  1. **记录分布体检**：主库 DB 按 `project.user_id` 聚合各账号记录数（探针 SQL 正式化），列出非当前账号的滞留记录（账号名 + 项目数 + 会话数）+ 空壳镜像行；
  2. **未注册账号检测**：解密主库 `storage.json` 登录 blob 识别本地登录过但未在 App 注册的账号，提示补注册（blob 解密复用 blob_keepalive 资产）；
  3. **一键归入（收编）**：非当前账号记录全部改写 `project.user_id` 为当前登录账号——**自动关-改-重启编排**（2026-08-31 裁定：与切号五步一致，进度事件复用现有弹层），执行前二次确认弹窗明确告知「将关闭主库 TRAE 实例」防误触；复用切号交接原语（三件套备份 → 空镜像行清理（UNIQUE 冲突防御）→ 归属改写 → 完整性校验）；**逐会话写接力台账**（from=原账号 to=当前账号，2026-08-31 裁定：保溯源链，与切号交接同构）；孤儿 user_input 行清理沿用交接链路策略。
- **完成情况（2026-09-01）**：`infrastructure/master_checkup.rs`（新模块：探针 SQL 正式化——账号分布聚合（LEFT JOIN 保留 0 会话账号，全量含软删与归属改写同口径）+ 孤儿行统计 + 滞留账号过滤，3 单测）；`master_handover.rs` 扩 `HandoverSession.previous_user_id`（逐会话记录原归属，收编台账多账号杂居精确到行）。`lib.rs` 新增 `get_master_checkup`（只读命令：分布 + 注册表比对投影账号名/registered/current）与 `incorporate_master_records`（四步编排：只读预检（有滞留才动，避免空跑打扰）→ 关实例 → 三件套备份 → 归属改写/换腿/完整性校验（复用交接原语单事务）→ 接力台账（fail-soft）→ 重启；生成中拒绝 `master_incorporate_busy`；不改登录身份与插件环境）；进度事件 `master-incorporate-progress`（closing/backing_up/incorporating/restarting/done）。前端环境页体检区块（有滞留才渲染，无差异静默；账号名/数量主信息，user_id 收悬浮提示）+ 收编弹层（确认规模与账号列表（ADR-0018 单次确认）→ 四阶段进度条（复用切号弹层步骤条样式）→ 完成回执数量表述；运行中禁关）；`safeUiError` 新增 8 个收编错误码文案。简化说明：未注册账号检测通过注册表比对分布行实现（分布行无对应注册即显示「未登记账号」），未解密 blob——达成同样用户目标且零新增解密面；体检随 active/手动刷新/切号/收编后同读同刷，未做 5 秒轮询（体检非时敏数据，减少打扰）。孤儿行（user_id IS NULL）只报告不收编（无归属行改写缺乏依据，保守不动）。
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
    | checkin-import-1307767855650905 | 1571.2 MB | 1490.5 | 76.2 | 3.0 | ✅ |
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
- **方案**：卡片/列表 meta 删"令牌 N 天"与"设备尾号"（内部标识，界面纪律），移入详情页"技术详情"折叠区（含续期阈值说明）；卡片仅续期失败时提示（`refresh_error_code` 标记保留，纳入 G10 色彩）。
- **验收**：卡片 meta 只剩手机号；详情页折叠区含两项；续期失败标记仍显示。

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

**G8+G21+G22 对话列表重构（两栏式，先出原型确认）（✅ 2026-09-09 实施闭环；独立历史入口删除留 P8-6）**
- **前置**：补「Library 抽象」ADR——主库和副库的基地是"库"，库内模块（对话/插件/归档）不与具体库实例耦合；HistoryWorkbench 重命名 LibrarySessionsPanel，宿主页注入库参数（数据目录、raw key、当前 user_id）。
- **G21 两栏式**：左栏=项目树（点击原地展开会话子级：标题+相对时间；"未关联文件夹"末位；搜索框置顶；底部"选择"按钮）；右栏=对话查看器（顶部会话标题+[消息|接力]tab 默认消息；消息流上下排列，用户右对齐、助手左对齐；默认滚动到底部最新，向上滚动加载更早历史；2000 条截断保留）。用户消息按接力账号着色（色板 A=蓝/B=绿/C=琥珀 循环分配，色块头像+首字母），LLM 回复中性色。会话底部接力徽章收悬浮。
- **G22 统一选择模式**：左栏底部[选择]按钮进入（项目行/会话行显示勾选框，右栏变只读预览）；底部浮出 Gmail 式操作栏 [已选 N 会话·M 项目]+[归档][合并到…][恢复]（归档视图）/[删除]（会话级、单次确认列明数量与消息量）。合并流：弹层选目标项目→确认页→事务改挂+空源组清理。归档流：直接执行不确认（可逆），toast 反馈；左栏"已归档"入口行（有归档内容才显示，进入归档视图——同一棵树灰显，层级 模式→项目→会话）。浏览态项目行悬浮"归档"快捷图标。
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

**G4 残余文案（✅ 2026-09-09 实施闭环）**：插件卡片标签改名（随主库/仅此账号→切号保留/仅此账号）随 G23 一并落地（对账条文案已被 G23 取代作废）。

- **P8-5 实施记录（2026-09-09）**：G18 增加主库四项只读自检、健康/可自愈/需人工分级、缓存纠正与切号弹层强制重置兜底；G7 总览改读主库真实当前账号与最近活跃时间；G5/G3 收窄主视野并折叠技术细节；G4 文案已在 G23 收口。验收：Rust 自检判定 6 项通过，vitest 213 项通过，tsc 通过；P8-5 全量 e2e 33/33 与 workspace 回归通过。

### P8-6 收尾批

**G8 历史页删除（最后执行，避免中途测试断链）**
- **方案**：删除历史页与导航入口（NavigationRail/路由/测试清理）；对话列表功能已在 P8-4 LibrarySessionsPanel 承载。
- **验收**：导航无历史页；`rg "HistoryWorkbench|历史页"` 零命中（改名后）；全量回归（cargo test + tsc + vitest + e2e）。

### 贯穿约束（Phase 8 全程）
- 每批完成即跑全量验证（cargo test --workspace + tsc + vitest + e2e 三尺寸截图）+ code-review 后提交，不跨批欠账。
- 界面表达纪律（2026-09-01）全程适用：内部标识不进主视野、技术细节收折叠区/悬浮、G17 判定标准（常驻提示须可据此行动）。
- 数据红线解除但铁律不变：P8-4 归档/删除操作保留单次确认（G22 删除）；不自动删任何历史数据（G19 只过滤不删）。

## 后续设计议题：凭据代次稳定性（2026-09-09，核心约束已确认）

本议题由 `梦梦` 与 `用户2361650421` 的 `20403 Token device not match` 故障触发。当前只记录已确认的设计边界，不提前实施未确认的状态机、批量修复或迁移。

### P9-0 首次登录验收与一次 OAuth 约束（设计已确认，待拆分实施）

- 首次 OAuth 不以 AuthCode 交换成功作为完成条件；新凭据代次必须通过本地完整性/绑定校验、账号身份校验、一次真实续期轮换及成功写回。
- 首次真实续期返回 `20403` 时，只允许使用本次登录结果中的 access token 自动执行一次设备重铸；不得静默发起第二次 OAuth。
- 验收或修复失败时保留旧代次、候选代次与失败证据，不发布半成品凭据，不覆盖可恢复材料。
- 凭据发布采用“候选代次 → 完整验收 → 原子发布”边界：发布前不切换当前代次；任一步失败都保持旧当前代次不变，避免凭据文件与账号档案指向不同代次。
- 增加不含 token/私钥正文的凭据发布日志，记录前后代次、文件校验信息和事务阶段；启动或账号操作前只做本地幂等恢复，不自动触发 OAuth、设备重铸或网络重试。
- 对已完成登录验收的现有账号，续期明确返回 `20403` 且当前 access token 身份仍可校验时，自动设备重铸一次；按当前凭据代次持久记录次数，仅作用于续期 `20403`，不改变签到 `9074/9095` 的手动处理规则。
- 下一项待确认：续签设备身份与签到设备身份是否拆分，以及拆分后的服务端验收与回退边界。
