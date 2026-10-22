# 已知问题（不计入架构例外，逐条跟踪）

> 迁移中"发现但不修"的行为差异记这里：修复需要单独取证与测试，不在 UI 合并中顺手改。

## K-01 健康检测进度事件的产品值不一致（2026-10-23 发现）

- 现象：后端 `health-check-progress` emit 的 `product_id` 是 `trae_cn`，而 `credential-refresh-progress` / `credits-refresh-progress` emit 的是 `work_cn`。
- 旧前端主路径恒传 `work_cn` 做匹配，导致**健康检测的进度事件一直被丢弃**（用户看不到健康检测进度）。
- 现状：P4c-5a 的 `useAccountBatchProgress` **逐字沿用旧口径**，不擅自"修正"，以免在 UI 重构中混入行为变更。
- 修复条件：后端三处 emit 统一为 `work_cn`（或前端按产品分别匹配），并补一条单测覆盖"健康检测进度可见"。
- 归属：独立票，不在 P4c 范围内。

## K-02 适配器（池面）数据只存在于测试注入（2026-10-23 记录）

- 现象：`traePool` / `poolStatusLabel` 来自 `platform/accountAdapter.ts`，生产路径 `AccountWorkbench` 不再使用适配器；`tests/AccountAdapter.test.tsx` 仍需要它。
- 约定：`useAccountsSnapshot` 保持纯数据面；适配器池由页面以**可选 prop** 注入（P4c-6 落地），不塞进 hook。
