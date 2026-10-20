# P19 跨产品对齐执行计划（正式版）

状态：2026-10-20 用户确认 14 决策点后定稿。两份讨论草案（p19-information-architecture-draft.md / p19-architecture-draft.md）为本计划的详实依据，本文是执行依据。

## 决策结论（14 项，全部已拍板）

1 WB 备份范围 = 凭据材料（.info/keyblob/device-id）+ workbuddy.db + projects 正文目录，全纳入。
2 WB 破坏性写进程策略 = 一律"关客户端→写→重启"（复用 B1 进程边界）。
3 UI descriptor = 前端静态 productUiRegistry（与后端 productRegistry 的 sectionStatesFor 并表）。
4 B4 登录编排前置探查票 = 立项（asar 逆向 CodeBuddy OAuth 参数；用户配合登录一次抓 daemon 日志，届时单次确认）。
5 多实例实验票 = 主库移植（B3）之后。
6 WB 账号详情页 = display_name 编辑/归档/删除 + 手机号只读 + 凭据状态。
7 WB 软删除"彻底删除"入口 = 做。
8 签到日历 = checkin_dates 现有字段先行，当日明细端点探查票。
9 localStorage 偏好分产品 key = 接受一次性重置。
10 B6 环境页轻量版 = 只读状态卡（客户端卡+登录域健康卡），不含重启动作。
11 B1 软删行过户单测核验 = B3 首任务。
12 set_account_mobile = WB 手机号只读不写。
13 CodeBuddy 设备管理探针票 = 立项。
14 插件域探查票 = 排在 B4 探查之后。

## 批次计划（依赖序）

| 批次 | 内容 | 规模 | 并行性 |
|------|------|------|--------|
| P19-B1 骨架抽取 | 五步迁移①②③：纯移动→控件→页编排骨架 AccountPageSkeleton + productUiRegistry 声明 | M | 前端线，与 B3 并行 |
| P19-B2 WB 消费迁移 | 五步迁移④⑤：签到骨架 + WB 全页消费迁移（参与集默认全参与修复） | M | 依赖 B1 |
| P19-B3 主库移植 | archive_tx 事务纪律状态机提取 + WB 库（workbuddy.db）会话浏览/消息（jsonl）/软删除+彻底删除/过户核验/体检/统计/cwd 归并 + 备份范围落地 | L | 后端线，与 B1 并行；首任务=B1 软删行过户单测核验 |
| P19-B4 登录编排 | 探查票（asar OAuth 参数+daemon 日志）→ OAuthLoginProtocol trait → begin/complete_workbuddy_login | M | 探查可与 B1/B3 并行 |
| P19-B5 签到深化 | 逐账号行/额度列 total_credits/一键签/补签/checkin_dates 日历 | S | 依赖 B2 |
| P19-B6 总览/环境/备份 | WB 工作台统计 + 环境轻量版 + WB 备份 UI + Trae 手动删除备份 | M | 依赖 B3 |
| P19-B7 探查票组 | CodeBuddy 设备管理 / WB 插件绑定 / 签到明细端点 / 多实例实验（B3 后） | S×4 | 随时可排 |

## 验收标准

- 每批次对照本表与两份草案的验收小节执行；UI 批次附截图对比（pyautogui 截图链已有）。
- 测试基线：infrastructure ≥ 1000 passed、主 crate ≥ 230、vitest ≥ 400（随新票增长）。
- 铁律：PR 禁止骨架外产品分支；absent 结论必须有探针证据；真机操作推迟至用户统一验收窗口。
