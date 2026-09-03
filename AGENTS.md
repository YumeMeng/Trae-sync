# 项目规则

- 使用中文交流；生成代码时使用中文注释。
- 开始工作前读取 `CONTEXT.md`、`docs/DECISIONS-20260822-GRILL.md`（注意其头部取代标注）、相关 ADR 和 `tickets.new.md`。
- 项目定位：个人使用的 TRAE Work CN 多账号日常管理工作台。四个核心需求（账号保存、对话主库共享、直连签到、账号切换）是项目存在的理由，一切工作围绕它们展开。
- V1 仅支持 TRAE Work CN；保留 Product Adapter 边界，但不提前实现其他平台。
- `D:\work\Trae-Account-Manager-main` 与已安装的 Manager 仅供行为参考，不建立代码或数据依赖；账号一律通过 App 内 OAuth 登录。
- 数据红线已解除（ADR-0018）：可直接读写 TRAE 真实数据库与 API。数据安全保障 = App 内手动备份入口 + 破坏性批量操作（删除/覆盖/迁移）执行前单次确认。
- 铁律（2026-09-02 修订，ADR-0018 决策 3）：工具不得自动删除用户历史、快照或失败证据；主库备份（`.switch-bak-*`）例外——按用户配置的保留策略自动清理旧备份（P5-9，默认保留最近 5 份、可关闭）。
- 遵循 `tickets.new.md` 的 Phase 计划推进；修改与计划无关的代码需先说明理由。
- **决策链同步纪律（2026-09-02）**：ADR 状态行是决策权威——引用任何规划或决策前，先核对 ADR 索引与状态行的取代关系（如 ADR-0021 取代 ADR-0020 的收入/返还语义、ADR-0024 取代其实例模型）；发现文档间冲突时，先修复文档再继续任务，不得按过时内容工作。决策变更落地时，同步更新 tickets.new.md、CONTEXT.md 与 agents 文档中的关联引用，防止旧表述残留误导。
- 只做当前任务要求的精准修改，不增加未确认功能，不重构无关内容；随任务产生的孤立代码要清理。
- 每项任务完成后，说明下一项推荐任务及理由。
- 服务端交互（签到/OAuth/ExchangeToken）遵循 ADR-0019 的设备身份与幂等判定规则；签到协议事实以 `docs/TECHNICAL_BASELINE.md` 与 `.scratch/checkin-http/` 实测证据为准，禁止凭猜测修改协议行为。
- 界面表达纪律（2026-09-01）：用户界面不得把底层编号、内部标识或错误码当主信息展示（记录 ID、市场 UUID、registry 标识、账号指纹原文、profile_id 等）；主信息只用用户能直接理解的表达——名称、数量、时间、动作结果。技术细节确需保留时收进默认折叠区或悬浮提示，不占主视野。文案用主流产品的自然说法，不发明术语（如「重铸」「随行」「腿」），不用高理解成本的抽象词（如「台账」「对账」「吸收」进 UI 正文）；描述对象对用户是什么就叫什么。

## Agent skills

### Issue tracker

使用本地 Markdown。实施计划位于 `tickets.new.md`（Phase 0-7 结构，现行唯一计划文件）；调查、PRD 和临时任务位于 `.scratch/`。旧 T01-T21 计划与旧规范文档已于 2026-08-22 清理删除；旧版 `tickets.md`（Phase 0-4 结构，含被 ADR-0021/0024 废止的主库规划）已于 2026-09-02 删除（git 历史可查）。

### Triage labels

使用默认五种角色标签。详见 `docs/agents/triage-labels.md`。

### Domain docs

使用 single-context：根目录 `CONTEXT.md` 与 `docs/adr/`。详见 `docs/agents/domain.md`。

<!-- code-review-graph MCP tools -->
## MCP Tools: code-review-graph

**This project has a knowledge graph. Start with the code-review-graph
MCP tools to narrow scope, then read the source.** The graph is cheaper than scanning files and
gives you structural context (callers, dependents, test coverage) that file search cannot.

### When to use graph tools FIRST

- **Exploring code**: `semantic_search_nodes_tool` or `query_graph_tool` instead of Grep
- **Understanding impact**: `get_impact_radius_tool` instead of manually tracing imports
- **Code review**: `detect_changes_tool` + `get_review_context_tool` instead of reading entire files
- **Finding relationships**: `query_graph_tool` with callers_of/callees_of/imports_of/tests_for
- **Architecture questions**: `get_architecture_overview_tool` + `list_communities_tool`

### Verify in the source

- Narrow scope with the graph, then read the source. Do not change code from graph output alone.
- For any non-trivial change, read the implementation and the relevant tests before concluding.
- Verify the exact source when touching behavior, database logic, migrations, retries, fallbacks,
  recovery, or compatibility code.
- When the graph and the source disagree, the source wins. The graph may be stale or may not
  model that relationship.
- An empty graph result can mean "not indexed" or "not statically visible", not "does not exist".

### Key Tools

| Tool | Use when |
| ------ | ---------- |
| `detect_changes_tool` | Reviewing code changes — gives risk-scored analysis |
| `get_review_context_tool` | Need source snippets for review — token-efficient |
| `get_impact_radius_tool` | Understanding blast radius of a change |
| `get_affected_flows_tool` | Finding which execution paths are impacted |
| `query_graph_tool` | Tracing callers, callees, imports, tests, dependencies |
| `semantic_search_nodes_tool` | Finding functions/classes by name or keyword |
| `get_architecture_overview_tool` | Understanding high-level codebase structure |
| `refactor_tool` | Planning renames, finding dead code |

### Workflow

1. The graph auto-updates on file changes (via hooks).
2. Use `detect_changes_tool` for code review.
3. Use `get_affected_flows_tool` to understand impact.
4. Use `query_graph_tool` pattern="tests_for" to check coverage.
<!-- /code-review-graph MCP tools -->
