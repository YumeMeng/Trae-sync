# 迁移状态（2026-10-24，分支 feat/ui-refactor-20261022）

> 本文件是重构进度的事实来源。新会话接手时先读它 + `UI-CHARTER.md` + `UI-DIRECTION.md` + `KNOWN-ISSUES.md`。
>
> **当前结论：六个页面全部收敛为单一实现，P5 收敛（CSS 四层 + 死码清理 + 接缝删除）已完成。** 重构主线结束，后续为独立票（见文末「已知问题索引」）。

## 一、最终基线（2026-10-24 P6-2b 实测）

| 项 | 值 | 命令 | 本次是否实跑 |
|---|---|---|---|
| TypeScript 类型检查 | **0 错误** | `pnpm typecheck` | 是（exit 0） |
| 单元测试 | **32 文件 / 359 例全绿** | `pnpm test` | 是（32 passed / 359 passed） |
| 债务闸门 | **新增 0 / 既有 0**（基线文件 0 条目） | `pnpm guard` | 是（exit 0） |
| E2E | 41 例 | `pnpm test:e2e` | 否（沿用 P5-5 提交记录，本票未跑） |
| 源码规模 | 前端 200 文件 / 29,506 行；Rust 127 文件 / 121,335 行 | `node scripts/gen-module-map-inventory.mjs` | 是（脚本生成） |
| 已注册 IPC 命令 | **135 条**（`generate_handler!` 实际注册；文档命令表逐条一致） | 同上 | 是 |

## 二、六页 + P5 收敛：已完成

### 六个页面（每页一个交互只有一份实现）

| 页面 | 落点 | 旧实现删除 | 完成票 | 遗留 |
|---|---|---|---|---|
| 总览 | `src/features/overview/` | 旧 504 行 | P4a | 无 |
| 签到 | `src/features/checkin/` | 旧 1,014 行 | P4b-1..4 | 无 |
| 账号 | `src/features/accounts/`（40 文件 / 4,961 行） | 累计约 6,675 行 | P4c-1a..9c | K-01 / K-02 / K-06 缺口 5 |
| 环境 | `src/features/environment/`（24 文件 / 3,099 行） | 1,764 + 220 行 | P4d-8b-2c | K-07 账号捕获缺渲染断言 |
| 主库 | `src/features/library/`（36 文件 / 4,563 行） | 5 文件 4,183 行 | P4e-9c | K-10 G1/G2/G5 |
| 设置 | `src/features/settings/`（14 文件 / 1,624 行） | 5 文件 1,103 行 | P4f-7 | K-11（有意变更） |

产品差异的落点统一为「声明式登记表 + 产品 builder」：页面不得判断 productId，`src/platform/` 是唯一允许出现 `invoke(` 的层（宪章 §4.4，由 `scripts/ui-guard.mjs` 规则 4/5 静态拦截）。

### P5 收敛（CSS 与接缝）

| 票 | 内容 | 结果 |
|---|---|---|
| P5-0 | 只读审计 | 12 份 CSS 6,574 行；死规则 1,396 行（21.0%）；13 处字面色值 + 9 处 guard 盲区 |
| P5-1a | 设计令牌 | 7 个新令牌替换全部 13 处在扫描域内的字面色值，guard 基线 13 → 0 |
| P5-1b | 修 guard 盲区 + 悬空变量 | 徽标由 Bootstrap 蓝 #3b82f6 归位设计靛蓝 #4f46e5（对比度 3.68 → 6.29 AA） |
| P5-2a/b/c | 三轮死码清理 | 合计 **-1,548 行**，死类实质归零（余 12 个为动态模板或非类名） |
| P5-3a | CSS 去重 | 14 处真值冲突 → 0，`min-height` 收敛为单一真源 |
| P5-3b | CSS 四层落盘 | tokens / base / components / pages；39,528 个计算值逐一比对一致 |
| P5-4a | 六槽位内联进 App | 用 `key={productId}` 固化重挂载契约（删 key 后测试即红，契约被证明） |
| P5-4b | 删过渡接缝 | `sectionSlots` -152 行、`SettingsPanel` 直接渲染页面、E-01 例外撤销 |
| P5-5 | guard 基线归零 | 探针证明 guard 仍会拦截（注入违规即 exit 1） |

`docs/architecture/P5-CONVERGENCE.md` 保留为 P5-0 审计时点的历史快照，行号与字节数**不随落地重算**；现行路径与职责以 [MODULE_MAP](MODULE_MAP.md) 为准。

### 目录清理（P6-2b）

重构残留的空目录已删除，`Test-Path` 均为 false：

- `src/pages/`
- `src/components/product/trae/`（连带空的父目录 `src/components/product/` 一并删除）

`MODULE_MAP` 的目录清单现由脚本生成，输出为「无残留空目录」——后续若再出现空目录，重跑生成脚本会立即列出。

### 文档回填

- P6-1：CSS 路径、六 feature + 五 platform 职责、ADR 索引 0036/0037。
- P6-2a：`MODULE_MAP` 源码清单改为目录级统计 + 生成脚本 `scripts/gen-module-map-inventory.mjs`（含 `--check` 与链接自检）。
- P6-2b：命令表改为**全量登记 135 条**（口径见下），删除 3 条源码中不存在的命令，本文件与 KNOWN-ISSUES 收口。

## 三、命令表口径（P6-2b 定稿）

`generate_handler!` 实际注册 **135 条**。此前文档只登记 96 条 Work/CN 命令，差额 42 条为 WorkBuddy 域命令（另有 3 条已退役命令被误登记）。本票选择 **(a) 补全**：

- 42 条 WorkBuddy 域命令全部补进表内（定义模块：`workbuddy_commands.rs` / `workbuddy_library_commands.rs` / `workbuddy_backup_commands.rs` / `workbuddy_login_commands.rs` / `workbuddy_schedule.rs`）。
- 删除 3 条源码中不存在的命令（旧 CN 签到/凭据刷新命令族，P15-7 已随池绑定代码退役；逐条名称见 `.scratch/cmdr/P6-2b-report.md` §3）。
- 生成脚本每次运行做**双向比对**：handler 有而表缺、表有而 handler 无，任一非零即在表前写出不一致明细。

未选 (b) 的理由：口径说明无法替代条数事实。只要表只登记部分命令，「96 vs 134」这类差额就要靠口头解释，且新命令默认漏登记不会被发现；全量登记后，覆盖率说明只在真的不一致时报警，脚本可自动兜底。

## 四、已知问题索引

详见 [KNOWN-ISSUES](KNOWN-ISSUES.md)。当前仍有效的条目：

| 编号 | 一句话 | 类型 | 归属 |
|---|---|---|---|
| K-01 | 健康检测进度事件 `product_id=trae_cn`，前端按 `work_cn` 匹配 → 进度被丢弃 | 行为缺陷（待修） | 独立票 |
| K-02 | 适配器池面数据只存在于测试注入，生产路径不用 | 结构说明（非缺陷） | 随适配器面退役处理 |
| K-05 | 批量「忙碌但未收到首批事件」时缺回退文案 | 可见反馈小缺口 | 独立小票 |
| K-06 | WB 详情四写操作（备注名/归档/参与开关/删除）缺命令级渲染断言 | 测试缺口（写操作） | 独立票 |
| K-07 | 环境页账号捕获（成功 / 已注册去重）缺渲染断言 | 测试缺口（写操作） | 独立票 |
| K-10 | 库页 G1 切库代次隔离、G2 轮询指纹、G5 CN 归并三链缺渲染级用例 | 测试缺口（数据可信度） | 独立票（同批） |
| K-09 | WB 库页会话时间格式由 locale 漂移改为确定格式 | **有意行为变更** | 已定案，无需修 |
| K-11 | WB 设置页读取门禁由「挂载即读」改为「可见才读」 | **有意行为变更** | 已定案，无需修 |
| K-13 | 产品徽标颜色恢复为设计靛蓝（可见变化） | **有意修复** | 已定案，无需修 |

已闭环（保留正文与结论，不再作为待办）：K-03、K-04、K-08、K-12。

## 五、铁律（每票都适用）

1. 一票 ≤300 行新代码（Trae 执行者输出上限 16k token，超了就零产出）
2. 每票必须自己跑：typecheck / test / guard；接线票加 build + e2e
3. 不许删断言、不许放宽条件；改断言必须说明「语义等价」
4. 切号事务、DPAPI、登录三类收尾、设备 productId=`work_cn`、批量凭据二次确认——不可简化
5. 完成定义：旧文件已删 + e2e 绿 + guard 绿 + 截图自检 5 问

## 六、验证命令

```powershell
cd D:\work\Trae-sync
pnpm typecheck; pnpm build; pnpm test; node scripts/ui-guard.mjs; pnpm test:e2e
node scripts/gen-module-map-inventory.mjs --check   # MODULE_MAP 三段清单是否与磁盘一致
```
