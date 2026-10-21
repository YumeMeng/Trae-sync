- **删除纪律（2026-10-19 五修定稿——删除保护已关闭，恢复正常删除）**：**WorkBuddy 删除保护已由用户于 2026-10-19 关闭并重启客户端生效**（此前五版纪律记录的沙箱重定向问题已终结）——bash 任意形态/cargo/rustc/Node fs 的删除均为**直接删除**，回收站零增量（实测验证：裸 rm 与 find -delete 均真删）。**保留的纪律**：①Rust 代码内删除用 `std::fs::remove_file` / `fs::remove_dir_all`；②构建产物清理用 `cargo clean`；③**删后验证习惯保留**——大批量删除后运行 `python scripts/diag-recyclebin.py` 监控回收站增量（防止设置回滚或机制变化）；④**回收站异常膨胀仍先取证**（diag 脚本解析 $I 元数据）再清空。历史背景（沙箱重定向四版实验记录）见 git log 本条目演进。# 项目规则

- 使用中文交流；生成代码时使用中文注释。
- 开始工作前读取 `CONTEXT.md`、`docs/DECISIONS-20260822-GRILL.md`（注意其头部取代标注）、相关 ADR 和 `tickets.new.md`。
- 项目定位：个人使用的 TRAE 多产品渐进适配工作台；TRAE Work CN 是完整支持面，四个核心需求（账号保存、对话主库共享、直连签到、账号切换）仍是 Work 的行为基线。
- 当前按 ADR-0029~0034 适配 Work CN 与 Trae CN；复用 Product Adapter 与共享页面，不提前实现其他平台。操作的实现、真实验收与当前可用性分别判定；CN 原生切换不自动交接历史，工具账号选择不等于官方客户端身份。
- `D:\work\Trae-Account-Manager-main` 与已安装的 Manager 仅供行为参考，不建立代码或数据依赖；账号一律通过 App 内 OAuth 登录。
- 数据红线已解除（ADR-0018）：可直接读写 TRAE 真实数据库与 API。数据安全保障 = App 内手动备份入口 + 破坏性批量操作（删除/覆盖/迁移）执行前单次确认。
- 铁律（2026-10-18 修订，P17 grilling 定稿 spec 10.2，取代 ADR-0018 决策 3 的多份保留）：**滚动单备份**——仅切号与删除会话两种高危操作触发自动备份，触发前旧备份**彻底删除**（不走回收站，可再生数据硬删纪律），同一时刻全目录仅保留一份；keep=N 链式保留与保留策略配置退役（backup-retention.json 仅兼容读写）。回收站（safe-delete）仅用于用户可能误删需找回的场景。
- **删除纪律（2026-10-19 四修定稿——沙箱机制真相 + 务实策略）**：**本开发会话运行在 WorkBuddy agent 沙箱内，进程树内的一切文件删除（bash 任意形态/cargo/rustc/Node fs/任何子进程）都会被沙箱重定向进回收站**——四连实验铁证：裸 rm、绝对路径 rm、`env -u CODEBUDDY_SESSION_ID`、直调 /bin/rm 真二进制，全部进站，与环境变量/调用方式无关，无法绕过。**务实策略**：①**最小化删除**——构建产物清理只做 `cargo clean`（进回收站属正常损耗，事后清空即可）；②**大批量删除前知会用户**（预期回收站增长量级）；③**删后验证**——大批量删除后运行 `python scripts/diag-recyclebin.py` 确认增量与预期一致（当前 13,617 条 / 2.96GB 为 target 构建产物，属可再生数据，用户清空即可）；④**回收站定期清空**由用户执行（或经用户授权后以系统清空命令程序化处理——建议作为工具"空间维护"能力的候选）；⑤**回收站若异常膨胀**（单日 >3 万条或含用户文档路径）：先跑 `python scripts/diag-recyclebin.py` 取证（$I 元数据还原原路径+删除时间）再清空。沙箱外执行（用户在独立终端手动 cargo clean / 资源管理器 Shift+Del）不受此限制。
- **一键启动入口语义（2026-10-21 用户规则）**：用户日常用 `E:\系统存储\桌面\TraeSync一键启动.bat` 启动工具 = `pnpm tauri dev`（dev 模式，每次启动增量编译 main 最新代码）。**每次更新代码后必须保证该入口可见最新内容**——即所有改动合入 main 即视为"交付到用户手上"，不存在"旧构建"缓冲期；真机走查反馈一律先按最新代码定位，禁止用"旧版本"解释用户看到的现象。
- **新产品接入五步强制流程（2026-10-20，ADR-0037 铁律）**：扩展任何新平台前必须 ①过概念映射表（GLOSSARY「跨产品概念映射」章节，逐概念回答判定问题清单，禁止凭印象）②逆向验证（每个"该平台没有"的结论必须有探针/asar/协议证据，无证据的 absent 结论禁止成立）③映射矩阵（Trae 全命令逐项标注复用/移植/重定义/豁免-待探查）④骨架复用（UI 走 src/components/shared/ 组件 + productUiRegistry 能力开关，ADR-0036，禁止骨架外产品分支渲染）⑤登记（新平台语义差异回填映射表）。违反任一步即返工。
- 遵循 `tickets.new.md` 的 Phase 计划推进；修改与计划无关的代码需先说明理由。
- **本轮子 agent 调度限制（2026-10-08 再确认）**：不得派发只读调查、审计或评审子 agent；需要审核时只向用户指定的审查会话 `01a0f2c4-34f7-7150-b56b-291e7820a010` 发送具体请求并等待回复，持续同步必要进展，不得修改该会话的模型或推理设置。允许派发的执行子 agent（实现、测试及验证）必须在每次派发参数中显式指定 `model: gpt-6-luna`、`reasoning_effort: xhigh`；直接与派生任务均适用，不得省略参数、依赖默认值或选择固定为其他模型的专用角色。调度工具不能启动指定组合时，由主 agent 继续处理或说明阻塞，不得换模型或推理等级。执行子 agent 均须使用 Caveman skill，并收到简明的文件责任、任务边界和输出要求。此限制覆盖本轮全部 Phase 14 子票及相关任务。
- **主会话模型边界（2026-10-03）**：上述按职责划分的模型限制只约束实际派生的子 agent，不授权覆盖用户已有的指挥、审查或执行主会话。主会话的模型与推理等级由用户选择；执行 agent 及其子 agent 向已有主会话调用 `send_message_to_thread` 时必须省略 `model` 和 `thinking`，保留目标会话当前设置。不得通过其它工具或配置绕过；只有用户明确指名目标会话并要求修改设置时，才可按其要求调整。按需向审查会话提问的授权不包含改模授权。
- **Phase 14 当前执行状态（2026-10-01）**：本次独立审核已返回，用户已授权持续实施并要求按需向审查会话提问、不再重复等待总体确认。见 .scratch/phase14-execution-review-20261001.md；旧 handoff/goal 摘要的总体等待条款不再控制执行。票内依赖、G1–G5 与真实破坏性操作执行点单次确认保持。
- **决策链同步纪律（2026-09-02）**：ADR 状态行是决策权威——引用任何规划或决策前，先核对 ADR 索引与状态行的取代关系（如 ADR-0021 取代 ADR-0020 的收入/返还语义、ADR-0024 取代其实例模型）；发现文档间冲突时，先修复文档再继续任务，不得按过时内容工作。决策变更落地时，同步更新 tickets.new.md、CONTEXT.md 与 agents 文档中的关联引用，防止旧表述残留误导。
- 只做当前任务要求的精准修改，不增加未确认功能，不重构无关内容；随任务产生的孤立代码要清理。
- 每项任务完成后，说明下一项推荐任务及理由。
- 服务端交互（签到/OAuth/ExchangeToken）遵循 ADR-0019 的设备身份与幂等判定规则；签到协议事实以 `docs/TECHNICAL_BASELINE.md` 与 `.scratch/checkin-http/` 实测证据为准，禁止凭猜测修改协议行为。
- 界面表达纪律（2026-09-01）：用户界面不得把底层编号、内部标识或错误码当主信息展示（记录 ID、市场 UUID、registry 标识、账号指纹原文、profile_id 等）；主信息只用用户能直接理解的表达——名称、数量、时间、动作结果。技术细节确需保留时收进默认折叠区或悬浮提示，不占主视野。文案用主流产品的自然说法，不发明术语（如「重铸」「随行」「腿」），不用高理解成本的抽象词（如「台账」「对账」「吸收」进 UI 正文）；描述对象对用户是什么就叫什么。

## Agent skills

### Issue tracker

使用本地 Markdown。实施计划位于 `tickets.new.md`（当前含 Phase 0-14，现行唯一计划文件）；Phase 14 PRD/准备评审位于 `.scratch/phase14-prd-20261001.md` / `.scratch/phase14-readiness-review-20261001.md`，当前任务必须读取；架构归属与产品接入检查见 `docs/architecture/`，调查和临时证据位于 `.scratch/`。旧 T01-T21 计划与旧规范文档已于 2026-08-22 清理删除；旧版 `tickets.md`（Phase 0-4 结构，含被 ADR-0021/0024 废止的主库规划）已于 2026-09-02 删除（git 历史可查）。

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
