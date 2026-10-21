# P19 草案：跨产品信息架构模板 + 功能映射矩阵

> 2026-XX（P19 立项草案）。作者：产品经理（Xu）。范围：以**产品一致性视角**定义六页信息架构模板（产品无关规格 + TRAE/WB 逐区块数据源）、108 命令逐项映射矩阵、WB 用户故事补全、术语锚定清单（ADR-0037 首批条目）。本文档不修改代码。
>
> 上游共识：设计思想三铁律（信息架构统一 / 组件共享 / 能力开关）+ ADR-0032 能力三态（`ready`/`pending`/`absent`）+ P17 四次误判教训 + 八域对齐方案（用户已确认）。

---

## 0. 设计原则（本文档的判定规则）

1. **模板先行**：六页（总览/账号/签到/库/环境/设置）的区块构成与交互模式是**全产品同一套模板**。新增产品 = 注册表 + 适配器 + 探针，不改模板。
2. **能力三态渲染**（ADR-0032）：每个区块按产品能力呈现 `ready`（真实数据与操作）/ `pending`（适配中，诚实说明，不渲染操作）/ `absent`（协议事实不存在，明示"该产品无此能力"）。**禁止**按 productId 整页分支。
3. **扩展判定纪律**（P17 教训）：任何"该平台没有 X 能力"的结论，必须先过 §4 术语锚定清单的判定问题清单 + 协议探针，否则默认标 `pending` 而非 `absent`。
4. **术语锚定**：所有区块与命令映射使用 §4 的跨产品概念名（概念名独立于任何产品的实现词）。

---

## 1. 六页信息架构模板

> 标注约定：**[必备]** 全产品必须渲染（状态可为三态之一）；**[能力开关]** 仅在产品能力清单含该能力时渲染操作，否则三态降级；**[后置]** 本期不做但模板预留位。每区块给出：显示字段 / 交互 / 空态 / TRAE 数据源 / WB 数据源。

### 1.1 总览页（Overview）

产品无关规格：`问候横条 + 账号横条 + 统计卡网格（签到卡、库卡）+ 健康提示条`。

| 区块 | 必备性 | 显示字段 | 交互 | 空态 | TRAE 数据源 | WB 数据源 |
|---|---|---|---|---|---|---|
| 问候横条 | [必备] | 时间问候语 + 产品 displayName（注册表） | 无 | 无（恒有） | `get_current_product` + `list_products` | 同左（复用，displayName=WorkBuddy） |
| 账号横条 | [必备] | 当前账号：头像+昵称+产品徽章；账号池总数；凭据健康摘要 | 点击→跳账号页；快捷切换入口 | "尚未添加账号 → 去添加"（按钮跳账号页添加编排） | `get_managed_account_state` + `get_account_registry` | `list_workbuddy_accounts` + `account-snapshot.json`（primary） |
| 签到统计卡 | [必备] | 参与账号数 / 今日已签 N·M / 积分合计 / 连签天数（streak） | 点击→跳签到页 | 三态：WB 已 ready | `get_checkin_overview` | `get_workbuddy_auto_checkin_status` 逐账号聚合（total_credits / streak_days / checkin_dates），WB 侧待聚合命令（映射矩阵 #43） |
| 库统计卡 | [必备] | 库内账号数 / 项目数 / 会话数 / 最近活动时间 | 点击→跳库页 | `absent` 或 `pending` 三态（WB 现为占位页 = 违反铁律，本票修复） | `get_master_library_stats` | `workbuddy.db`（sessions 表按 user_id 统计）+ jsonl 目录扫描——移植 `get_master_library_stats` 的 WB 适配器 |
| 健康提示条 | [能力开关] | 异常条目列表：凭据将过期 / 体检异常 / 备份缺失 | 条目可点击跳转对应页处理 | 全正常时不渲染（非空态） | `get_master_self_check` + `get_managed_credential_statuses` | `get_workbuddy_session_health` + .info `expiresAt/refreshExpiresAt` |

### 1.2 账号页（Accounts）

产品无关规格：`操作行 + 过滤器行 + 视图切换 + [账号行]×N + 详情页 + 空态`。

| 区块 | 必备性 | 显示字段 | 交互 | 空态 | TRAE 数据源 | WB 数据源 |
|---|---|---|---|---|---|---|
| 操作行 | [能力开关] | 添加账号 / 刷新 / 健康检测 / 刷新额度 / 刷新凭据 / 隔离浏览器（TRAE 共 8 操作元素） | 每按钮独立；批量操作作用于当前过滤结果 | 不渲染空态（恒在） | begin/complete/cancel_trae_cn_login、capture_*、check_*_health、refresh_* | 添加=OAuth 授权码流编排（CodeBuddy 协议参数，复刻 Trae 编排）；刷新/健康=`refresh_workbuddy_account`/`get_workbuddy_session_health` 已复用；刷新额度/凭据=移植（.info expiresAt + checkin-status 额度）；隔离浏览器=WB 无此能力→`absent` |
| 过滤器行 | [必备] | 过滤维度：全部 / 已归档 / 凭据异常 / 自动签到开关态；排序：昵称/验证时间/额度 | 单选 + 排序切换 | — | `get_account_registry` | `list_workbuddy_accounts`（含 allAccounts 历史数组） |
| 视图切换 | [必备] | 列表视图 / 卡片视图 | 双态切换，记忆用户选择 | — | 前端本地 | 前端本地（同一组件） |
| 账号行 ×N | [必备] | 头像+昵称(display_name)+双徽章（账号源/状态）+最近验证时间+uid+凭据剩余天数+模型积分+切换按钮 | 行点击→详情；切换按钮→单次确认后切换 | "暂无账号 → 添加第一个账号"（按钮触发添加编排） | `get_managed_credential_statuses` + `get_checkin_overview`（积分列） | `list_workbuddy_accounts`（uid/nickname）；凭据剩余=.info `expiresAt`；模型积分=`workbuddy_checkin_status` total_credits |
| 详情页 | [能力开关] | display_name 编辑 / 归档开关 / 删除（含 purge 前置预览）/ 原始字段只读区 | 编辑保存、归档即生效、删除走单次确认 | — | `set_account_display_name` / `set_account_archived` / `purge_account_records` | 移植三个命令的 WB 适配器（八域对齐方案第 4 条） |
| 隔离浏览器入口 | [能力开关] | — | — | — | TRAE 专用 | `absent`（WB 捕获为文件解密链路，无需隔离浏览器；探针已闭环） |

### 1.3 签到页（Check-in）

产品无关规格：`参与集行 + 批量操作行 + [签到账号行]×N + 今日台账状态条 + 日历 + 空态`。

> P19.1 修正：**每日自动签到设置卡归设置页**（以 Trae 真机为事实源——真机的调度设置在设置页，签到页只留签到动作区）。WB 侧随此排布：`WorkBuddyAutoCheckinSettings`（总开关/触发时间/台账/参与集行）由 WB 设置页变体消费，签到页仅保留只读的自动签到状态条。

| 区块 | 必备性 | 显示字段 | 交互 | 空态 | TRAE 数据源 | WB 数据源 |
|---|---|---|---|---|---|---|
| 参与集行 | [必备] | "已选 N / M"+全选/反选；默认全参与（八域对齐方案第 3 条） | 复选 | — | `list_checkin_accounts` | 移植：WB 参与集 = 账号池 ∩ 自动签到开关态（`set_workbuddy_account_auto_checkin` 已有） |
| 批量操作行 | [能力开关] | 一键签到(N) / 一键补签(N) / 刷新额度 / 刷新凭据 | 单次确认纪律（写操作不重试） | — | `run_checkin` + `cancel_checkin` + refresh_* | 移植批量编排；底层 `workbuddy_checkin_claim` 已复用（幂等 code=10001 对齐 already_checked_in） |
| 签到账号行 ×N | [必备] | 复选+昵称+额度（total_credits/daily_credit）+爆存时点+最近验证时间+独立签到按钮 | 独立签到=单账号单次确认 | "参与集为空 → 去账号页添加/开启自动签到"（TRAE 同款空态文案模板） | `get_checkin_overview` 逐行 | `workbuddy_checkin_status` 逐账号（接口字段已实证齐全——P17 误判③已翻案） |
| 签到日历 | [能力开关] | 每账号 checkin_dates 月历视图 | 月份切换；点日期看当日明细 | 无记录月份显示空月历 | Trae 签到记录表 | `workbuddy_checkin_status.checkin_dates`（字段已实证） |
| 虚拟设备管理 | [能力开关] | 设备指纹/重置 | 重置=单次确认 | — | `reset_checkin_device` | `pending`（WB 直连 Bearer 无设备绑定字段；探针若发现风控再升级为重定义） |
| 签到登录编排 | [能力开关] | — | — | — | begin/complete/cancel_checkin_login | `absent`（WB 签到直连接口无需独立登录态，token 即凭据；探针实证） |

### 1.4 库页（Library / 对话库）

产品无关规格：`主库卡 + 体检卡 + 会话列表 + 会话详情 + 操作行 + 空态`。

| 区块 | 必备性 | 显示字段 | 交互 | 空态 | TRAE 数据源 | WB 数据源 |
|---|---|---|---|---|---|---|
| 主库卡 | [必备] | 库路径 / 状态（挂载/未挂载）/ 大小 / 版本 | 启动主库 / 打开详情 | "未找到对话库 → 指引定位" | `get_master_library_stats` + `launch_master_library` | 移植：`workbuddy.db`（sessions 按 user_id 隔离，与 Trae 主库同构——P17 误判①已翻案） |
| 体检卡 | [能力开关] | 体检五项结果（完整性/孤立记录/索引/凭据关联/备份时效） | 立即体检（只读） | "从未体检 → 立即体检" | `get_master_checkup` + `get_master_self_check` + `get_master_verification` | 移植五项的 WB 适配器（八域对齐方案第 2 条） |
| 库内统计 | [必备] | 账号数 / 项目数 / 会话数 / 占用空间 | — | — | `get_master_library_stats` + `get_account_storage_footprint` | 移植：db 统计 + `~/.workbuddy` 目录体积 |
| 会话列表 | [必备] | [会话行]×N：所属项目 + 标题 + 最近时间 + 消息数；项目分组/归并入口 | 过滤/搜索；选中→详情 | "库中无会话" | `get_master_history` | 移植：sessions 表查询；jsonl 正文按 cwd 分目录归并 |
| 会话详情 | [必备] | 消息时间线（角色+内容）/ 删除（单次确认）/ 过户到其他账号（收编语义）/ 归并到项目 | 删除/过户/归并均单次确认 | — | `get_master_session_messages` / `delete_master_sessions` / `prepare_handoff_intent` / `incorporate_master_records` / `merge_master_projects` / `apply_master_archive_changes` / `get/set_archive_draft` | 全套移植（语义 = 收编：把历史会话归到正确账号名下，WB 侧"转移历史会话"已有基础命令） |
| 操作行 | [能力开关] | 清理空项目 / 新建环境（跳转）/ 外部项目识别与归并预览 | 单次确认 | — | `cleanup_master_empty_projects` / `list_trae_cn_foreign_projects` / `preview_trae_cn_project_merge` / `merge_trae_cn_project` | 移植（cwd 归并语义） |
| 主/副库切换 | [后置] | — | — | — | Trae 后置项 | 同后置 |

### 1.5 环境页（Environments）

产品无关规格：`客户端卡 + 环境列表 + 健康卡 + 多实例区`。本期 WB = 轻量版先行（八域对齐方案第 6 条）。

| 区块 | 必备性 | 显示字段 | 交互 | 空态 | TRAE 数据源 | WB 数据源 |
|---|---|---|---|---|---|---|
| 客户端卡 | [必备] | 客户端安装路径 / 运行状态 / 版本 / 数据根 | 启动 / 关闭客户端 | "未检测到客户端安装" | `get_trae_instance_states` | 移植：`WorkBuddy.exe` 映像名 + `--user-data-dir` 参数解析（P17 已实测可行） |
| 环境列表 | [能力开关] | [环境行]×N：名称 / 实例状态 / 当前账号 / 启动 / 删除（删除前预览） | 启动/关闭/改名/删除 | "暂无环境 → 新建环境" | `list_environments` / `create_environment` / `launch_environment` / `close_environment_instance` / `rename_environment` / `delete_environment` / `get_environment_delete_preview` | `pending`：WB 多实例（--user-data-dir）可行性待实验，独立实验票；实验通过前本区块显示"适配中" |
| 健康卡 | [必备] | 环境健康：数据根可写 / 实例冲突 / 凭据关联正常 | 立即检测 | — | `get_environment_state` + `repair_master_current_account` | 移植轻量版健康检查（客户端卡 + 健康卡为 WB 本期最小集） |
| 环境内登录 | [能力开关] | — | — | — | `login_environment` | `pending`（跟随多实例实验票结论） |
| 插件预览（切号随迁） | [能力开关] | 切号影响的插件清单预览 | — | — | `preview_master_switch_plugins` | `pending`（WB 插件生态待探查票） |

### 1.6 设置页（Settings）

产品无关规格：`自动签到设置区 + 备份区 + 存储区 + 恢复包区 + 转移账本 + 产品与运行状态 + 插件入口`。

| 区块 | 必备性 | 显示字段 | 交互 | 空态 | TRAE 数据源 | WB 数据源 |
|---|---|---|---|---|---|---|
| 自动签到设置 | [必备] | 总开关 / 触发时间 / 今日台账 / 账号参与清单 | 保存即生效 | — | `get/set_auto_checkin_settings` | `get/set_workbuddy_auto_checkin_settings` + `list/set_workbuddy_checkin_accounts`（P19.1 归位：签到页 → 设置页，对齐 Trae 真机） |
| 备份区 | [必备] | 备份目录 / 保留策略 / 备份链列表（时间+大小+可恢复点） | 手动备份 / 恢复 / **手动删除备份**（本次新增，八域对齐方案第 7 条）/ 迁移备份 | "尚无备份 → 立即创建" | `create_master_backup` / `get_master_backup_chain` / `restore_master_backup` / `migrate_master_backups` / `get/set_backup_dir` / `get/set_backup_retention` | 全套复用（治理骨架产品无关）；WB 备份范围补凭据材料（.info/keyblob/device-id——P17 8.7 缺口警示，待拍板） |
| 存储区 | [能力开关] | 存储根状态 / 各账号占用 / 删除计划预览 | 迁移存储根 / 计划删除（两步：plan→apply） | — | `get_storage_root_state` / `migrate_storage_root_command` / `plan_storage_deletion` / `apply_storage_deletion` | 移植（`~/.workbuddy` 根组已定常量） |
| 恢复包区 | [必备] | 已导出恢复包列表 | 导出 / 导入 | "无恢复包" | `export/import_recovery_package_command` | 复用 |
| 转移账本 | [必备] | 账本条目：时间 / 类型 / 源→目标 / 结果 | 只读 + 清理入口 | "账本为空" | `get_relay_ledger` | 复用（账本表结构产品无关） |
| 产品与运行状态 | [必备] | 产品身份 / 各操作 ready-pending-absent 状态表 | 只读诊断 | — | `get_product_identity_state` / `get_product_operation_states` / `restore_operation` / `read_work_cn_state` | 复用前三；`read_work_cn_state` 对 WB `absent` |
| 插件入口 | [能力开关] | 插件工作台入口卡 | 跳转 | `pending`（探查票先行） | `get_plugin_tab_state` / `browse_plugin_market` / `install_plugin` / `uninstall_plugin_everywhere` | WB `pending`→探查后定 `ready`/`absent` |

---

## 2. 功能映射矩阵（108 命令逐项）

> 图例：**复用**=WB 已有等价实现；**移植**=待做，WB 侧方案一句话；**重定义**=语义变化（说明）；**豁免**=WB 不适用（理由）。分域计数：产品/工作台 8 · 账号 29 · 签到 20 · 库 18 · 环境 14 · 备份/存储/设置 15 · 插件 4 = **108**。来源：`src/types/commandNames.ts`（generate_handler 全量）。

### 2.1 产品 / 工作台层（8）

| # | 命令 | 判定 | 说明 |
|---|---|---|---|
| 1 | get_current_product | 复用 | 产品注册表读取，产品无关 |
| 2 | get_product_identity_state | 复用 | 身份状态协议统一 |
| 3 | get_product_operation_states | 复用 | 三态渲染的判定源 |
| 4 | get_workspace_state | 复用 | 工作台聚合，前端通用 |
| 5 | list_products | 复用 | 注册表遍历 |
| 6 | set_current_product | 复用 | 产品切换 |
| 7 | restore_operation | 复用 | 通用恢复操作，产品无关 |
| 8 | read_work_cn_state | 豁免 | Work CN 特有客户端状态读取；WB 捕获/健康已由专用命令覆盖，无此状态面 |

### 2.2 账号域（29）

| # | 命令 | 判定 | 说明 |
|---|---|---|---|
| 9 | begin_trae_cn_login | 移植 | WB 版 OAuth 授权码流启动（CodeBuddy 协议参数：oauthClientId/authorizedFlow，与 Trae 编排模式同构） |
| 10 | cancel_trae_cn_login | 移植 | WB 登录编排取消（同一编排骨架） |
| 11 | complete_trae_cn_login | 移植 | WB 授权码回换 token → 档案登记（复刻编排第 3 步） |
| 12 | capture_current_account_credential | 移植 | WB 捕获 = .info 信封解密链路（ELECTRON_RUN_AS_NODE 取钥 + AES-256-GCM + AAD，已真机闭环），替换 Trae 的 storage.json blob 铸造 |
| 13 | capture_trae_cn_account | 移植 | WB 捕获编排（快照→解密→在线复核→代次容器铸造→登记，P17 8.4 三轮定稿） |
| 14 | capture_workbuddy_account | 复用 | 已实现 |
| 15 | check_trae_cn_account_health | 复用 | 健康检测协议统一，WB 走 #18 |
| 16 | get_account_registry | 复用 | 档案注册表产品无关 |
| 17 | get_managed_account_state | 复用 | 托管态产品无关 |
| 18 | get_managed_credential_statuses | 移植 | WB 凭据状态 = .info `expiresAt/refreshExpiresAt` + daemon 刷新日志（access 28 天/refresh 30 天） |
| 19 | get_workbuddy_session_health | 复用 | 已实现 |
| 20 | list_remote_devices | 重定义 | Trae=TRAE 平台远端设备管理；WB=CodeBuddy 账号是否有设备管理页待探查，探针前标 `pending` 不标 `absent`（P17 纪律） |
| 21 | clear_remote_device | 重定义 | 同 #20，跟随探针结论 |
| 22 | open_device_management_docs | 复用 | 文档跳转，产品无关 |
| 23 | list_trae_cn_accounts | 重定义 | 泛化为按产品的账号列表接口；WB 侧等价 = #24 |
| 24 | list_workbuddy_accounts | 复用 | 已实现（含 allAccounts 历史数组） |
| 25 | prepare_managed_account_switch | 移植 | WB 切号前置：走 wbipc `auth:logout→auth:login/saveOauthToken` RPC（客户端 watcher 调和约束，不可直改 .info——P17 8.4 定案） |
| 26 | purge_account_records | 移植 | WB 账号记录清除（`~/.workbuddy` 专属目录 + 档案登记删除，单次确认） |
| 27 | refresh_managed_current_account | 移植 | WB 当前账号刷新 = 触发 daemon 刷新语义（客户端续期、工具只读纪律） |
| 28 | refresh_workbuddy_account | 复用 | 已实现 |
| 29 | set_account_archived | 移植 | 归档位写入 WB 档案登记（UI：详情页开关） |
| 30 | set_account_display_name | 移植 | display_name 写 WB 档案登记（八域对齐方案第 4 条） |
| 31 | set_account_mobile | 豁免→待探查 | WB 手机号为加密信封可读不可写；写入无协议依据，探针前不实装（显示为只读字段） |
| 32 | switch_account | 复用 | 切换通用编排 |
| 33 | switch_and_handoff | 移植 | WB 切换+过户组合（过户走库域 #85 WB 适配） |
| 34 | switch_master_account | 重定义 | 主库当前账号切换——WB 语义=workbuddy.db 当前 user 关联切换，随库域移植 |
| 35 | switch_trae_cn_account | 重定义 | 泛化为按产品切换接口；WB 侧等价 = #36 |
| 36 | switch_workbuddy_account | 复用 | 已实现 |
| 37 | get_account_storage_footprint | 移植 | WB 占用 = `~/.workbuddy`（app/security/storage 分目录）体积计算 |

### 2.3 签到域（20）

| # | 命令 | 判定 | 说明 |
|---|---|---|---|
| 38 | begin_checkin_login | 豁免 | WB 签到直连（Bearer + X-User-Id），无独立签到登录态（探针实证） |
| 39 | cancel_checkin | 复用 | 取消调度的签到任务，调度器产品无关 |
| 40 | cancel_checkin_login | 豁免 | 同 #38 |
| 41 | complete_checkin_login | 豁免 | 同 #38 |
| 42 | get_auto_checkin_settings | 复用 | 调度设置，产品无关 |
| 43 | get_checkin_overview | 移植 | WB 聚合视图：逐账号调 `workbuddy_checkin_status` 汇总（total_credits/streak/daily_credit），供总览卡+签到行 |
| 44 | get_checkin_capability | 复用 | 能力探针结论声明位（WB=ready 已探针实证） |
| 45 | get_workbuddy_auto_checkin_status | 复用 | 已实现 |
| 46 | list_checkin_accounts | 移植 | WB 参与集 = 账号池 ∩ auto_checkin 开关态 |
| 47 | refresh_checkin_credentials | 移植 | WB 签到凭据刷新 = .info 重解密取新 token（客户端续期后） |
| 48 | refresh_checkin_credits | 移植 | 额度刷新 = `workbuddy_checkin_status` 只读重查 |
| 49 | remove_checkin_account | 移植 | 退出参与集（不动账号池） |
| 50 | reset_checkin_device | 豁免→条件重定义 | WB 直连无设备绑定字段；探针若发现设备风控则升级重定义（虚拟设备 WB 版） |
| 51 | run_checkin | 复用 | 批量签到执行器产品无关；底层 claim 走 #55 |
| 52 | set_account_auto_checkin | 复用 | TRAE 侧开关 |
| 53 | set_auto_checkin_settings | 复用 | TRAE 侧设置 |
| 54 | set_workbuddy_account_auto_checkin | 复用 | 已实现 |
| 55 | set_workbuddy_auto_checkin_settings | 复用 | 已实现 |
| 56 | workbuddy_checkin_claim | 复用 | 已实现（幂等 code=10001 对齐 Trae already_checked_in） |
| 57 | workbuddy_checkin_status | 复用 | 已实现（checkin_dates 支撑日历区块） |

### 2.4 库域（18）

| # | 命令 | 判定 | 说明 |
|---|---|---|---|
| 58 | apply_master_archive_changes | 移植 | WB 归档变更落盘（db 写 + jsonl 目录调整） |
| 59 | cleanup_master_empty_projects | 移植 | WB 空项目清理（sessions cwd 分组无会话项，单次确认） |
| 60 | delete_master_sessions | 移植 | WB 会话删除（db 行 + jsonl 文件，两步确认） |
| 61 | get_archive_draft | 移植 | 归档草稿读取（通用草稿机制） |
| 62 | get_master_checkup | 移植 | WB 体检五项适配器（db 完整性/孤立 jsonl/索引/凭据关联/备份时效） |
| 63 | get_master_history | 复用 | 过户/转移历史记录（WB 转移历史会话已有基础） |
| 64 | get_master_library_stats | 移植 | WB 统计 = workbuddy.db 聚合 + jsonl 目录扫描 |
| 65 | get_master_self_check | 移植 | WB 自检适配器 |
| 66 | get_master_session_messages | 移植 | WB 消息浏览 = db 索引定位 + jsonl 正文解析 |
| 67 | get_master_verification | 移植 | WB 库验证适配器 |
| 68 | incorporate_master_records | 移植 | WB 收编（把游离会话归到正确账号 user_id 名下） |
| 69 | launch_master_library | 移植 | WB 库查看工具启动（sqlite 浏览器指向 workbuddy.db） |
| 70 | list_trae_cn_foreign_projects | 移植 | WB 外部项目识别（cwd 不在已知项目集的会话目录） |
| 71 | merge_master_projects | 移植 | WB 项目归并（cwd 归并语义） |
| 72 | merge_trae_cn_project | 重定义 | 泛化归并预览+执行对；WB 侧走 #70/#71 语义 |
| 73 | prepare_handoff_intent | 移植 | WB 过户意图（会话 user_id 重指派，收编语义） |
| 74 | preview_trae_cn_project_merge | 重定义 | 同 #72 |
| 75 | set_archive_draft | 移植 | 归档草稿写入（通用） |

### 2.5 环境域（14）

| # | 命令 | 判定 | 说明 |
|---|---|---|---|
| 76 | close_environment_instance | 重定义 | WB 本期=关闭客户端（进程匹配 `WorkBuddy.exe`）；多实例关闭随实验票 |
| 77 | create_environment | 重定义 | WB 轻量版无环境创建；语义收敛为"实例登记"，多实例实验通过后恢复完整语义 |
| 78 | delete_environment | 重定义 | 同 #77 |
| 79 | get_environment_delete_preview | 重定义 | 同 #77（删除前预览，实验票通过后实装） |
| 80 | get_environment_state | 移植 | WB 环境状态 = 客户端运行态 + 数据根健康（轻量版最小集） |
| 81 | get_trae_instance_states | 重定义 | WB 版实例状态 = 映像名+user-data-dir 参数解析（P17 实测可行）；多实例枚举随实验票 |
| 82 | launch_environment | 重定义 | WB 本期=启动客户端；多实例启动（`--user-data-dir` 注入）随实验票 |
| 83 | list_environments | 重定义 | WB 轻量版返回单"默认实例"行；实验票通过后扩展 |
| 84 | login_environment | 豁免 | 环境内登录为 TRAE 多实例编排；WB 随实验票后置（探针前不实装） |
| 85 | rename_environment | 重定义 | WB 轻量版单实例不可改名 → `absent`（轻量版）+实验票恢复 |
| 86 | set_environment_current_account | 重定义 | 环境级账号绑定；WB=当前账号即客户端登录账号（切换走账号域），轻量版 `absent` |
| 87 | force_reset_master_current_account | 移植 | WB 当前账号强制重置（库域当前账号关联修复，走 wbipc RPC） |
| 88 | repair_master_current_account | 移植 | WB 当前账号修复（db user 关联 vs .info 对账） |
| 89 | preview_master_switch_plugins | 豁免→待探查 | 插件随迁预览依赖插件生态探查票结论 |

### 2.6 备份 / 存储 / 设置域（15）

| # | 命令 | 判定 | 说明 |
|---|---|---|---|
| 90 | apply_storage_deletion | 移植 | WB 删除执行（plan→apply 两步，zstd 压缩对未加密 WB 数据有效） |
| 91 | create_master_backup | 移植 | WB 备份 = `~/.workbuddy/app` + 凭据材料扩展（缺口警示 8.7，待拍板） |
| 92 | export_recovery_package_command | 复用 | 恢复包导出，产品无关 |
| 93 | get_backup_dir | 复用 | 备份目录设置 |
| 94 | get_backup_retention | 复用 | 保留策略 |
| 95 | get_master_backup_chain | 复用 | 备份链，治理骨架产品无关 |
| 96 | get_storage_root_state | 移植 | WB 根组注册（`~/.workbuddy` 常量已定） |
| 97 | import_recovery_package_command | 复用 | 恢复包导入 |
| 98 | migrate_master_backups | 复用 | 备份迁移 |
| 99 | migrate_storage_root_command | 移植 | WB 存储根迁移 |
| 100 | plan_storage_deletion | 移植 | WB 删除计划（预览层） |
| 101 | restore_master_backup | 移植 | WB 备份恢复（含凭据材料恢复位，待拍板） |
| 102 | set_backup_dir | 复用 | 备份目录写入 |
| 103 | set_backup_retention | 复用 | 保留策略写入 |
| 104 | get_relay_ledger | 复用 | 转移账本，账本结构产品无关 |

### 2.7 插件域（4）

| # | 命令 | 判定 | 说明 |
|---|---|---|---|
| 105 | browse_plugin_market | 豁免 | WB 插件生态探查票先行（八域对齐方案第 8 条）；探针前 UI 显示 `pending` |
| 106 | get_plugin_tab_state | 豁免 | 同 #105 |
| 107 | install_plugin | 豁免 | 同 #105 |
| 108 | uninstall_plugin_everywhere | 豁免 | 同 #105 |

**映射汇总**：复用 33 · 移植 43 · 重定义 18 · 豁免 14（其中"豁免→待探查/条件重定义"5 项，探针结论可升级）。WB 已复用基线 = 10 条既有 WB 命令全部落在矩阵内。

---

## 3. WB 侧用户故事补全（用户在 WB 侧的一天）

> 格式：作为 [角色]，我想要 [能力]，以便 [价值]。标注涉及页面与命令（矩阵编号）。

1. **捕获新账号**：作为多账号运营者，我在 WB 客户端用 CodeBuddy 账号登录后，想一键把当前登录态捕获进工具，以便不依赖手动记录 token。——账号页操作行（#12/#13 捕获编排；空态→档案登记成功提示）。同日第二种情形：客户端关着也能捕获（文件解密链路，零客户端运行）。
2. **看今日签到概况**：作为每天开工的用户，我想在总览一眼看到"参与 N 个账号 / 今日已签 M / 积分合计 / 连签天数"，以便判断今天还有没有号没签。——总览页签到统计卡（#43 聚合 + #45/#57 逐账号）。
3. **一键批量签到 + 补签**：作为懒人用户，我想在签到页勾选全部账号点"一键签到(16)"，漏掉的走"一键补签"，以便 30 秒完成全池签到。——签到页参与集行+批量操作行（#39/#51 批量编排，底层 #56/#57；幂等语义防重复领）。
4. **单账号异常单独处理**：作为细致的用户，某账号签到失败时，我想只对它单独重签+刷新额度，以便不整批重跑。——签到页账号行独立签到按钮（#48/#49/#47）。
5. **转移历史会话（收编）**：作为刚切换账号的用户，我发现旧账号名下有会话实际是新账号产生的，想把它们过户到正确账号名下，以便库内归属准确。——库页会话详情过户（#73/#68/#63；账本留痕 #104）。
6. **浏览与清理对话**：作为库管理员，我想按项目浏览 WB 的全部会话、看消息正文、删掉废弃会话、归并同项目散落目录，以便库整洁可检索。——库页会话列表+详情+操作行（#66/#60/#71/#70）。
7. **库体检**：作为谨慎的用户，我想一键体检 WB 库（完整性/孤立记录/凭据关联/备份时效），以便在出问题前发现隐患。——库页体检卡（#62/#65/#67）。
8. **切换当前账号**：作为多账号用户，我想在账号页一键切换 WB 当前登录账号（走官方 RPC 通道），以便客户端立刻生效且不触发误登出。——账号页账号行切换按钮（#25/#36；wbipc RPC 纪律）。

---

## 4. 术语锚定清单（ADR-0037 概念映射表·首批条目）

> 用法：扩展任何新平台前，先过该概念的"判定问题清单"，**全部回答后才允许下 `absent` 结论**；答不出的标 `pending`。

### 4.1 对话库（会话存储）——P17 误判①（"云端跟号"臆断）

- **Trae 语义**：主库 = 本地 SQLCipher SQLite（61 表）+ jsonl 正文；按 user_id 隔离；工具可直接读写（闸门制）。
- **WB 语义**：`workbuddy.db` sessions 表按 user_id 隔离（与 Trae 主库同构）+ jsonl 正文按 cwd 分目录。**本地，非云端**。
- **判定问题清单**：① 该平台会话存储在哪（本地/云端/混合）？② 是否本地 SQLite/类 SQL 结构？账号隔离字段是什么？③ 正文在哪个文件形态（jsonl/leveldb/IndexedDB）？目录键是什么（cwd/项目 id）？④ 有无官方导出/云同步端点与本地冲突？

### 4.2 环境（多实例）——P17 误判②（以为 WB 无多实例就不做）

- **Trae 语义**：环境 = `--user-data-dir` 多实例 + 环境内登录 + 环境级账号绑定。
- **WB 语义**：同一机制候选——Electron 显式 `--user-data-dir`（`~/.workbuddy/app` 即显式指定产物）+ polling-lease 多实例租约线索。**可行性待实验，非"不存在"**。
- **判定问题清单**：① 该平台是否接受 `--user-data-dir` 类参数？② 进程/映像名如何识别实例？③ 有无实例租约/单例锁（WB 有 polling-lease）？④ 实例内登录态与主实例是否隔离？

### 4.3 额度（积分）——P17 误判③（没逆向就说没有）

- **Trae 语义**：额度字段来自签到协议（模型积分/合计），账号行与签到行展示。
- **WB 语义**：`checkin-activity-status` 接口实证含 `total_credits / streak_days / daily_credit / checkin_dates` 全部字段。**存在，且字段更全**。
- **判定问题清单**：① 有无计费/积分查询端点（查官方 billing/meter 族路由）？② 查询是否只读不消耗配额？③ 字段含哪些（合计/当日/连签/历史日期集）？④ 刷新凭据链路是否独立于签到？

### 4.4 添加账号（登录编排）——P17 误判④（以为登录只能在客户端）

- **Trae 语义**：客户端内登录 + 工具侧捕获铸造（storage.json blob + DPAPI）。
- **WB 语义**：网页 OAuth 授权码流（CodeBuddy 协议参数）+ 工具侧文件解密捕获（.info 信封 + 客户端运行时取钥），与 Trae **形态同构**。客户端关闭亦可捕获。
- **判定问题清单**：① 该平台登录是网页 OAuth 还是仅客户端内？② 授权码流参数（client_id/app_name/callback）能否从 asar 提取？③ 凭据落盘在哪、加密形态（明文/信封/DPAPI）？④ 无客户端运行时能否解密（取钥路径是否存在）？

### 4.5 签到登录态（反向锚定：Trae 有 WB 无）

- **Trae 语义**：签到需独立虚拟设备 + 签到登录编排（begin/complete/cancel）+ 设备重置。
- **WB 语义**：直连 `Bearer accessToken + X-User-Id`，无设备绑定、无独立登录编排。
- **判定问题清单**：① 签到端点认证方式（复用主 token/独立凭据）？② 有无设备指纹/风控字段？③ 失败重试语义（幂等码？）？④ 补签是否存在？

---

## 5. 待拍板决策点（需要用户裁决）

1. **WB 备份范围补凭据材料**（矩阵 #91/#101）：是否把 .info / keyblob / device-id / account-snapshot.json 纳入 WB 备份与恢复（P17 8.7 缺口警示）？涉及凭据材料落盘，按单次确认纪律。
2. **`set_account_mobile`**（#31）：WB 手机号只读展示 + 不实装写入，是否接受？
3. **多实例实验票优先级**（环境域 8 条重定义命令）：WB `--user-data-dir` 多实例实验排在库域移植之后还是并行？
4. **设备管理探针票**（#20/#21）：CodeBuddy 账号设备管理探针是否立项？
5. **签到日历颗粒度**：`checkin_dates` 只有日期集，日历区块是否需要当日明细（需要额外端点探查）？
6. **插件域**：探查票立项顺序（库域移植完成后）。
