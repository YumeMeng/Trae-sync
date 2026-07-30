# Trae Sync

独立的 TRAE 系列本地对话历史管理与同步桌面工具。

## 当前状态

- 阶段：V1 实现规格完成
- 状态：D-001 至 D-045 已确认，尚未生成程序代码
- 当前工作目录：`D:\work\Trae-sync`
- 与 `Trae-Account-Manager-main` 的关系：仅参考，不建立代码依赖

设计已经收敛。程序实现仅在用户明确批准后开始，并从 Gate 0 的项目骨架进入，不直接写真实同步功能。

## 文档

- `docs/DESIGN_DISCUSSION.md`：逐项设计决策及收敛状态
- `docs/TECHNICAL_BASELINE.md`：已验证的解密、数据库结构和迁移事实
- `docs/DESIGN_AUDIT.md`：V1 设计审计、问题结论和对应技术 Gate
- `docs/SESSION_MERGE_FEASIBILITY.md`：单条对话重挂与同项目历史合并的真实副本验证
- `docs/ACCOUNT_DETECTION_FEASIBILITY.md`：当前账号证据、真实切换验证和 Gate B 结论
- `docs/IMPLEMENTATION_SPEC.md`：V1 功能、模块、数据、执行和恢复规格
- `docs/GATE_PLAN.md`：Gate 依赖、fixture、通过条件和证据要求
- `docs/UI_PROTOTYPE_REVIEW.md`：融合 UI 的体验结论、信息取舍和未来功能扩展边界

完整逆向记录另存于：

```text
E:\系统存储\桌面\TraeWork对话同步
```
