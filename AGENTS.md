# 项目规则

- 使用中文交流；生成代码时使用中文注释。
- 开始工作前读取 `CONTEXT.md`、相关 ADR、`docs/IMPLEMENTATION_SPEC.md` 和 `docs/GATE_PLAN.md`。
- V1 仅支持 TRAE Work CN；保留 Product Adapter 边界，但不提前实现其他平台。
- `D:\work\Trae-Account-Manager-main` 仅供参考，不建立代码依赖。
- 对真实 TRAE 数据的能力必须受 Gate 控制；对应 Gate 未达到 `Qualified` 前只操作 fixture，不写真实活动数据库。
- 任何数据库写入必须遵守强制双备份、完整验证、崩溃协调和用户确认要求。
- 工具不得自动删除用户历史、快照、备份或失败证据。
- 只做当前任务要求的精准修改，不增加未确认功能，不重构无关内容。
- 每项任务完成后，根据 `ask-matt` 路由说明下一项推荐任务及原因。

## Agent skills

### Issue tracker

使用本地 Markdown。实施计划位于 `tickets.md`；调查、PRD 和临时任务位于 `.scratch/`。详见 `docs/agents/issue-tracker.md`。

### Triage labels

使用默认五种角色标签。详见 `docs/agents/triage-labels.md`。

### Domain docs

使用 single-context：根目录 `CONTEXT.md` 与 `docs/adr/`。详见 `docs/agents/domain.md`。
