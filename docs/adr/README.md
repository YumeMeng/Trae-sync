# ADR 索引

> 2026-08-22 建立并清理。本目录只保留与现行方向一致的决策文档。

## 现行 ADR（共 20 份，全部必读）

| ADR | 主题 | 约束什么 |
|-----|------|---------|
| [0018](0018-data-redline-release.md) | 数据红线解除 | 可直接读写真实数据；数据安全 = 手动备份入口 + 破坏性操作单次确认；铁律不自动删证据 |
| [0019](0019-checkin-virtual-device-and-account-source.md) | 签到虚拟设备与账号来源 | 每账号持久虚拟设备（随机 ID + EC 密钥对 + 随机 MachineID）；仅 OAuth 登录；打开时自动签；业务码不得压平；凭据原子写回（第 7 条）；同设备续期见 ADR-0027 |
| [0020](0020-master-library-and-instance-model.md) | 主库与实例模型 | 主库 = 目录级切换分两阶段；纯本地；并行多开（2026-08-23 修订），主库模式强制单实例。阶段 A/B 与收入/返还语义已被 ADR-0021 部分取代；决策 4 并行多开承载已由 ADR-0024 改为「环境」 |
| [0021](0021-master-single-ownership.md) | 主库记录单一归属 | 主库 = 环境内全部对话集合，正常记录单一归属当前登录账号；切号即归一；收编 = 环境页体检 + 一键归入；归档边界见 ADR-0028 |
| [0022](0022-session-archive-channel.md) | 会话级归档通道 | 出库 = 借用 hidden_status 原生枚举（voice_discussion）；恢复 = 还原 NULL；归档抽屉为对话列表子视图；编辑后统一提交；归档切号边界由 ADR-0028 定义 |
| [0023](0023-environment-plugin-manifest.md) | 环境插件清单模型 | 环境持插件基线；切号对账 = 吸收后应用（确认语义已被 ADR-0026 取代）；工具内管理即时改云端；差异识别仅用户触发；同步 fail-soft |
| [0024](0024-account-environment-decoupling.md) | 账号-环境解耦 | 实例目录转型登录凭据存档（三件套即全部消费面，E1 实证）；启动实例功能退役；环境 = 独立目录 + 当前账号（多对多，主库环境默认不可删）；工具链缓存为可清理派生数据 |
| [0025](0025-library-abstraction.md) | Library 抽象 | 主库/副库基地是「库」：前端注入 libraryId 引用（不注入密钥），后端命令按 id 解析库实例；HistoryWorkbench 改名 LibrarySessionsPanel |
| [0026](0026-plugin-sync-policy.md) | 插件同步策略 | 新增零确认、移除单确认且全账号传播、切号静默应用（含移除才单确认）；取代 ADR-0023 决策 2 确认语义；对账条移除 |
| [0027](0027-first-login-credential-acceptance.md) | 首次登录凭据验收与一次 OAuth 约束 | access 未过期时同设备 AuthCode 滚动换发；不自动重铸签到设备；禁止静默第二次 OAuth；失败保留证据 |
| [0028](0028-archived-session-freeze-boundary.md) | 归档会话冻结与账号切换边界 | 正常会话随账号交接；归档会话留在主库且不改写；归档/取消归档先编辑后一次提交；不可即时完成时必须显式刷新并回执 |
| [0029](0029-multi-product-workbench-boundary.md) | 多产品工作台边界与产品适配状态 | 产品/账号切换分离；凭据和数据库隔离；页面策略见 ADR-0032，本产品原生激活与验收门槛见 ADR-0034；跨产品复制禁令保留 |
| [0030](0030-trae-cn-credential-maintenance-boundary.md) | Trae CN 凭据维护边界 | Trae CN 使用独立 OAuth/client/凭据根；健康检测与续期进入共享账号模块；不继承 Work 签到/额度；真实续期需独立验收 |
| [0031](0031-account-remote-device-management.md) | 账号级远程设备管理能力 | 设备列表/退出是账号级共享能力（能力清单声明接入）；原生 ListDevices 主路径 + 官方页兜底；本机禁退（UI 隐藏 + 后端拒绝双防线）；退出逐台单次确认；协议独立 `remote_device` 模块；remint 语义不动 |
| [0032](0032-product-ui-capability-tri-state.md) | 产品化界面适配与能力三态 | 导航五页全产品开放；区块三态；ready 门槛见 ADR-0034；主库写入范围见 ADR-0033；局部取代 ADR-0029 决策 4/9 与 ADR-0030 决策 5 |
| [0033](0033-trae-cn-library-selective-write.md) | Trae CN 库选择性写边界 | 修订 ADR-0032 决策 5：CN 库从全只读升级为选择性写（项目归并+归档/恢复两类；删除/合并仍拒）；依据=交接原语已被 CN 客户端真实容忍（09-28 三次授权交接）；三步真机验收为可用性前置门禁；无路径 empty 项目不参与 |
| [0034](0034-product-operation-boundaries-and-native-switch.md) | 产品操作边界、能力验收与真实账号切换 | 保留五层与共享页面；操作级验收、产品进程/数据/凭据隔离；工具选择与官方身份分开且并发按最新有效选择意图决胜；Work 切换提交/恢复安全；CN 原生切换先独立验证且不自动交接历史；Phase 14 |
| [0035](0035-unified-trae-product-and-credential-migration.md) | 融合版 TRAE 产品合并与账号迁移 | 融合版为唯一产品目标（取代旧独立 CN 适配目标）；双通道凭据（登录代次 TRAE 通道 + 签到代次 SOLO 通道）；铸造迁移路径与切号融合版变体；主库目标切换；旧 CN 模块移除（协议层保留）；Phase 15 |
| [0036](0036-cross-product-ui-consistency.md) | 跨产品 UI 一致性架构（骨架模板 × 产品数据） | 三原则：信息架构全产品同一套模板（六页）、组件共享（产品无关组件进 shared/，产品层只喂数据）、能力开关（产品差异只体现在「有没有这个数据/功能」，走 productUiRegistry + sectionStatesFor，禁止 productId 整页分支）；五步迁移路径；PR 门禁禁止骨架外的产品专属分支渲染 |
| [0037](0037-concept-mapping-and-extension-flow.md) | 跨产品概念映射表与扩展强制流程 | GLOSSARY 新增五列「跨产品概念映射」章节（锚定概念 / TRAE 实现 / WB 实现 / 扩展判定问题清单）；新产品接入五步强制流程：映射表 → 逆向验证（absent 结论必须有探针/asar/协议证据）→ 映射矩阵 → 骨架复用（shared/ + 能力开关）→ 登记回填；违反任一步即返工 |

配套决策记录：`docs/DECISIONS-20260822-GRILL.md`（项目方向历史锚点，九问九答全量；部分决策已被本表 ADR 取代，见其头部标注）。

## 2026-08-22 清理说明

原 ADR-0001~0017（Gate 时代与需求确认前的产物，含 4 份已废止、9 份部分失效、4 份现行工程决策）已全部删除，git 历史可查。删除前的处置：

- 仍有约束力的工程事实已并入现行文档：SQLCipher 导出契约 → `docs/TECHNICAL_BASELINE.md`；凭据原子写回规则 → ADR-0019 第 7 条；raw key 分类与 Manager 参考边界 → `CONTEXT.md` 与 `AGENTS.md`；签到编排语义 → ADR-0019。
- 其余内容（Gate 资格体系、位置见证、恢复包、SafeSyncPreview、旧账号切换设计等）随旧定位废止，不迁移。

## 维护规则（2026-08-22 起生效）

1. **任务驱动创建**：后续每个 Phase 任务落地需要架构决策时，创建正式 ADR；不为推测性扩展预建。
2. **一经接受不可废弃**：新 ADR 是"不可废弃"级别——推翻某决策必须新开 ADR 并在旧 ADR 状态行显式标注取代关系，不删除文件。
3. **状态行即权威**：每份 ADR 顶部状态行声明日期、取代/依赖关系；本表与状态行不一致时以状态行为准并修复本表。
4. **必读集合**：本表现行 ADR + `DECISIONS-20260822-GRILL.md`（注意其头部取代标注）；其余技术事实查 `TECHNICAL_BASELINE.md`，计划查 `tickets.new.md`（现行唯一计划文件；旧版 `tickets.md` 已于 2026-09-02 删除，git 历史可查）。
