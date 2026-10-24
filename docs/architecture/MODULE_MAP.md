# 模块、入口与验收矩阵

> 2026-09-30 执行准备版；源码基线 `b90f12bc0b5998e17373c446757e72eb6c45ac51`。决策权威为 ADR-0034，执行 tracker 为 `tickets.new.md` Phase 14。
> 本表覆盖源码归属与入口，不宣称所有调用关系已逐个动态验证。改行为前仍须读取实际实现、所有相关调用者和测试；静态引用不等于运行覆盖。09-30 首次准备只修改文档；当前差异与验证结果见下方 10-01 准备评审。

## 2026-10-01 当前状态与执行记录

P14-2 最新状态（2026-10-09 后续，细节见 `tickets.new.md`）：凭据捕获/应用的真实 caller 已新增 Unknown 时零 fixture 文件改动测试；产品+目录 matcher 的聚焦 PID 传递已有专用断言，跨产品进程谎报同一目标目录时 fail-closed 且不聚焦。指定审查会话确认 caller 短路验证有效，并复核聚焦 seam 测试样本修订后无剩余缺口；生产聚焦仍接原 `focus_instance_windows`。完整 workspace 串行回归 44 个结果段共 1315 passed/0 failed/25 ignored；根 crate lib 222 passed/1 ignored、infrastructure lib 886 passed/7 ignored。Fake evidence 不证明真实 WorkCnProcessController 如何将 OS 错误映射为 Unknown；完整危险操作矩阵及 P14-9 真机进程启停仍待，P14-2 未关闭。

下方 2026-10-09 同日的早期 P14-2 执行记录是本次 caller/聚焦补测之前的状态快照；其中“仍缺/待测”描述已由上方最新状态更新。

P14-2 最新增量（2026-10-09）：默认产品数据目录只接受绝对 `APPDATA`；缺失、空或相对值返回 `None`，从而使无法证明裸启动 TRAE 与目标目录无关的状态保持 `Unknown`，而不是误报 `NotRunning`。关闭路径另以类型保留进程状态未知与已确认关闭失败；已核 caller 在备份、认证、数据库和注册写入前拒绝 Unknown。关闭编排通过进程查询/taskkill/等待注入缝，以合成快照验证 Work/CN 并存只关闭 Work PID、初始 Unknown 无进程动作、优雅关闭后的复查 Unknown 不升级、强制关闭只对目标 PID；matcher 还验证双产品同时运行各自均可识别及身份字段单缺仍为 Unknown。最新 `trae_instance::tests` 29/29、根 crate `--lib` 214 通过/0 失败/1 忽略。全调用点源码复核已区分 Work-only `ProcessControllerPort` 凭据材料观测，与负责环境状态/启动关闭/库和账号危险写守卫的 `trae_instance` matcher；未知状态在对应副作用前拒绝，详见 `tickets.new.md` 的 2026-10-09 记录。指定审查会话于 2026-10-09 完成只读复核，无新增 P0–P3；聚焦复用 matcher 的匹配 PID，票据记录了来源关系；P14-2 仍缺 ProcessControllerPort 凭据 caller 的 Unknown 零业务写入映射及明确的聚焦覆盖。CN 目标 close 对称测试已补、待测；真实 Windows/TRAE 启停验收留在 P14-9。此项仍不代表真实 TRAE 进程试验或 P14-2 验收完成。

G0 准备与独立执行评审已完成；用户授权按 Phase 14 持续实施并按需询问具体问题，不再等待总体开工确认。执行审查及持续规则见 [结论](../../.scratch/phase14-execution-review-20261001.md)，需求与关口见 [PRD](../../.scratch/phase14-prd-20261001.md)，依赖和文件责任见 `tickets.new.md` Phase 14。2026-10-01 最新集成验证：`pnpm typecheck` 通过、Vitest 23 文件/362 项通过、Rust `cargo test --workspace --lib` 830 项通过/7 项忽略；其后定向 P14-3 身份测试 5/5、infrastructure identity 过滤 38/38、OAuth `oauth_login::` 15/15、账号页/AccountAdapter 59/59 通过。后四项是最新 WIP 的定向验证，不代表全量套件在这些修改后重跑。P14-3 与 P14-6 的最新 WIP 和提交后回执风险见下方；E2E 与真实 OAuth/进程/库写/claim/CN 官方切换仍未验收。

P14-4a 最新增量（2026-10-02）：handover 两条事务路径把提交后读回失败/校验不符归为 `CommittedNeedsAttention`，COMMIT 错误归为 `CommitOutcomeUnknown`；主库切号与环境登录共用 `finalize_handover_failure`，对这两类状态保留认证身份，只对可确认的提交前失败执行回滚。统一归档入口在 commit 后核对持久化状态；未知或不一致时 UI 阻止直接重试并要求刷新。两个延迟外键用例真实触发 COMMIT 错误，断言错误映射及事务释放后的数据库状态；两项测试与 infrastructure crate 全量测试 835 项通过/7 项忽略，之后将 fixture key 规范为 32 字节并复跑两项定向测试 2/2。caller 回滚策略定向测试 4/4；根 crate 稳定错误码测试 1/1、root crate `cargo check --lib`、`pnpm typecheck` 与 safeUiError/库页 79 项亦已通过，但后几项早于本次 COMMIT 测试增量；最新完整 workspace/Vitest 未重跑。完整 caller 阶段故障注入、持久恢复与写租约仍待；CN archive 的 `not_requested` 回执见下方 P14-4b 增量。

Settings 动作 `.then` 与产品请求代次/通知来源、账号刷新确认未绑定产品、库详情产品切换清理缺口纳入 P14-1b/1c/1d。当前 WIP 已让 CN claim 的后端命令在任何网络副作用前检查操作状态并因 pending 拒绝，前端也只在快照为 ready 时允许点击；协议闭环与真实领取验收仍属 P14-7。当前 `OperationLease::acquire` / `acquire_bound` 保留 catalog 锁供目录库元数据使用；`acquire_shared` 是不持 catalog 锁的数据位置租约，新增 `acquire_shared_for_path` 按实际物理目录识别资源。实施与验证状态见下方 P14-4c 增量。

P14-2 最新增量（2026-10-01）：进程身份 matcher 已改为产品 + 数据目录精确匹配并保留 Unknown；启动/聚焦/关闭、环境运行态、账号/库/认证文件危险写路径已接入三态拒绝。账号清理仅接受绝对 APPDATA，并在备份前同时检查托管实例目录和 TRAE 原生账号目录。两个无生产 caller 的旧 archive/restore IPC 与重复直写基础设施 API 已按仓库调用及文档承诺搜索退役，统一使用 `apply_master_archive_changes`。10-01 继续核对了进程查询生产调用者，并为系统枚举错误补充可注入纯判定测试；环境删除也新增 Unknown 时保留注册行和目录的测试。最新定向验证：infra `master_archive::tests` 13/13、进程 matcher 23/23、root `credential_handover` 33 项通过/1 项忽略、`environment_commands_tests` 6/6；`pnpm gen:commands` 得 96 条命令、`commandNames.test.ts` 3/3、`pnpm typecheck` 通过，环境页/safeUiError Vitest 68/68 通过。此前 workspace `--lib` 全量通过是较早源码快照，不替代以上最新定向结果；E2E 与真实操作验收仍未完成。

P14-1a 当前已有共享判定：快照先验证产品/库归属；缺少所选库时 library 操作不报 ready；非真实运行模式会将 ready 降为 pending；执行门禁复用相同判定。CN claim、归档、归并均 pending。Work 的归档、删除与项目归并也保持 pending，直到各自的真实操作验收证据齐备；库读取仍可用。Work 客户端运行时的归档提示和二次确认已实现，但不能替代操作验收。Work 不支持 Trae CN 同目录项目归并，标为 absent。旧 Work 命令的缺省 libraryId 仍依 ADR-0025 兼容映射到 master；当前 UI 显式传库 ID，快照缺库时 fail-closed。CN 健康与自动签到的证据引用已修正。P14-2 已将进程 Unknown 拒绝接入危险 caller，并退役两个旧归档 IPC；仍需逐操作完成完整回归与验收。`evidence_ref` 留作内部依据，不作为 UI 主文案。旧前端断言已改为验证 CN pending 隐藏写入口、保留只读浏览；最新全量 Vitest 23 文件/362 项通过。

本轮只读复核发现 P14-3 E2 写后失败回退与 E1 供体身份校验缺口；最新 WIP 已在 E1 同一解密材料写入前绑定目标账号、阻止 E2 写后/不确定错误继续 E1，并给环境首登播种加同账号检查，定向测试 5/5 通过。该证据尚未覆盖真实 OAuth 或主库/环境完整编排。P14-4 仍须覆盖收编提交后重启失败、环境认证/DB 已变更后当前账号档案写失败、环境页收编/清理成功后 `get_environment_state` 刷新失败仍显示数据未变、库写成功后列表刷新失败等真实 caller 回执。`FixtureWorkCnSyncExecutor` 的人工恢复门槛按整个恢复根扫描，但它受 fixture 路径保护；Tauri 根未注册 `build_sync_plan` / `apply_sync_plan`，当前没有生产 caller。将其列为 P14-0 内部路径分类，不把该全局门槛误算成账号切换线上阻断；若未来接入生产，再明确跨位置恢复隔离范围。

P14-4c 调用闭环复核（2026-10-01 历史基线）：生产目录库初始化/账号运行态恢复会获取短租约，凭据 capture / `try_apply_saved_credential` / credential journal 与 intent 更新链也会获取租约；当时 `switch_master_account_inner`、`login_environment_inner`、环境删除、Work/CN archive 与 CN merge 没有持有租约贯穿真实 DB/认证/目录写。`run_blocking` 只是 `spawn_blocking(...).await`，不限制并发。`EnvironmentRegistry` 与 `RelayLedger` 当时虽原子替换，但没有共享文件锁。该历史缺口的当前落地情况见 P14-4c 增量。

P14-1 迟到上下文复核（当前源码）：账号页外壳不按产品重挂载账号页本体；Work 打开的全量凭据刷新确认在切到 CN 后可由当前 `adapter` 回调执行 CN 刷新，且确认框人数随当前账号池变化。`BatchProgressEvent` 没有 productId，账号页无论产品/页面是否活动都会消费同名 Work/CN 批次事件。`App.handleProductChange` 没清理单一 `libraryDetail`；主库详情和历史面板的成功异步响应无请求代次保护；历史面板切换 `library.id` 只清部分展示态，旧删除/合并确认仍可按新 `library.id` 调用。CN 签到 `loadAccounts` 的账号列表响应晚到后仍会逐账号查询状态。P14-1b/1c/1d 必须覆盖这些实调用与延迟顺序，不能只证明初次挂载隔离。
## 责任与依赖

| 责任代号 | 负责什么 | 产品范围与入口 | 测试/决策归属 |
|---|---|---|---|
| 壳 | 产品上下文、五页导航、详情路由、通知、动作来源 | Work/CN；App/TitleBar/NavigationRail | App/Overview/CapabilitySection 测试；ADR-0029/0032/0034；P14-1 |
| 账号 | 本产品账号登记、OAuth、凭据发布/维护、健康；选择与实测身份 | Work/CN，各用自己的注册表/凭据根/通道；账号页→accountAdapter 或 Work 编排 | 账号页/适配器测试、凭据验收探针；ADR-0019/0027/0030/0034；P14-3/6/8 |
| 签到 | status/claim/status 结果及 Work 自动计划 | Work 完整；CN claim 当前 pending。当前命令仍是直接 claim，Ok 即报 Claimed；status 缺 `enable`/`checked_in` 时被解析成 false，status/claim/status 与必需字段校验未实现 | CheckinPage、checkin_schedule、trae_cn_checkin parser、CN 探针；ADR-0019/0030/0034；P14-1/7 |
| 环境 | 官方数据根、副环境、启动/关闭/聚焦及真实账号切换 | Work 完整；CN 原生激活待实现，环境功能缺乏支持证据时不谎称已支持 | Environment/MasterSwitch、进程/身份/失败注入；ADR-0024/0034；P14-2~5/8 |
| 库 | Library 引用、只读历史/统计、归档草稿、归并与交接 | Work 主/副库；CN 读取已验证，选择性写待操作验收 | LibrarySessions/MasterLibrary、合成 SQLCipher、真实库探针；ADR-0021/0022/0025/0028/0033/0034；P14-1/4/9 |
| 插件 | 环境插件清单、账号云端应用、增删确认及 fail-soft | Work 已接线；入口暂留库详情，不为这轮搬家 | 插件页/相关 Rust 测试；ADR-0023/0026；P14-3~5 回归 |
| 设备 | 服务端账号级设备列表与显式退出、本机保护 | Work/CN 凭据访问账号级列表；不把列表拆成产品专属 | RemoteDevices、remote_device 测试；ADR-0031；P14-9 回归 |
| 备份 | 手动备份、配置、链/保留、恢复证据、原子文件发布 | Work 已接线；CN 按能力与具体操作取本产品备份；管理工具公共配置另标明 | Settings/backup_retention/恢复测试；ADR-0018/0034；P14-1/4/5 |
| 基础 | 路径、SQLCipher、文件身份、产品适配、脱敏、共享事件等 | 由调用者注入产品/库上下文，禁止未知上下文回退 Work | 定向单测、dependency_direction；ADR-0034；按对应切片 |
| 内部 | 旧原语、fixture、禁用 Gate/恢复入口、工具命令；包括当前未由 Tauri 根接线的 `build_sync_plan` / `apply_sync_plan` 与受 fixture guard 约束的 `FixtureWorkCnSyncExecutor` | 不算生产页面功能；保留/退役需要调用与兼容证据 | 原有单测/fixture；P14-0 分类，后续按明确需求处理 |

五层依赖继续沿用已有 Cargo 声明和 `src-tauri/tests/dependency_direction.rs`。该测试验证依赖声明；Cargo 本身不禁止未来添加合法但不符合本项目分层的依赖。根 crate 可以注入基础设施，不代表业务流程可以永远无归属地堆在根模块。

## 关键生产链与已确认缺口

| 链路 | 当前入口 → 核心实现/状态源 | 失败/验证缺口 | 任务 |
|---|---|---|---|
| Work OAuth/取消 | 账号页 → oauth_login → CheckinLoginClient/凭据发布 | 当前 WIP 的 begin 回执、complete/cancel 参数均含 attemptId，并在产品与 ID 匹配后才消费会话/取消/清理；账号页传递代次，`login_cancel_too_late` 提示进入收尾，`login_already_finished` / `login_not_started` 提示已结束或正在收尾。profile 路径包含秒、亚秒、进程和 attempt 标识，过期清理兼容新旧目录名。账号页最新全文件 57/57、`pnpm typecheck` 通过；缺真实 Tauri IPC transport 集成验收。首次/重复 OAuth 仍先以无 journal 的 `store.save` 覆盖凭据包、再写 registry、最后做续期验收，可能破坏旧代次或留下跨代次绑定，由 P14-5 继续处理 | P14-5/6 |
| Work 主库批量归档/恢复 | 库页 → `library_archive::apply_master_archive_changes` → `master_archive::apply_master_archive_changes` | 旧直写 IPC 与重复基础设施 API 已退役；P14-2 已接入进程 Unknown 拒绝。P14-4a WIP 区分 commit 未知与提交后读回不符，草稿异常后刷新核对；CN archive 回执已改为 `not_requested`。Work/CN mutation 成功后的列表刷新失败发 warning，不覆盖已完成结果；全量验收仍待 P14-4b/9 | P14-2/4 |
| Work 主库切号 | MasterSwitchDialog → credential_handover::switch_master_account_inner → switch_login_identity_dual_path → blob_keepalive/master_handover/plugins → 实际登录材料+DB+进程 | 身份 WIP 已修：E1 按解密供体 userId 写前匹配注册目标；共享 E2/E1 编排强制回退白名单，写后 `VerifyFailed` 与 `WriteOutcomeUnknown` 均不调用 E1。P14-3 故障注入通过真实主库 caller 验证未知写入结果时认证回滚、档案指针与会话库字节不变；定向 7/7、全量 Rust workspace 通过。指定窄审指出的编排级 E1 计数 P3 已补测并确认关闭。P14-4a 已区分 handover 的提交后校验失败与 commit 未知，两种状态均不 auth-only rollback；P14-4b 回执表达 `current_profile_saved` 与 `relaunch_outcome`；P14-4c 已在切号写流程持有主库物理目录租约 | P14-2~5 |
| Work 环境登录 | 环境页 → environment::login_environment_inner → 同一身份切换 helper+handover | 首登播种已先验证 donor userId 与 registry 目标一致。P14-3 故障注入通过真实环境登录 caller 验证写后 `VerifyFailed` 时认证回滚、档案指针与会话库字节不变，E1 未调用；P14-3 指定窄审未发现剩余问题。P14-4a 两类 handover 提交后状态不会触发认证文件单独回滚；P14-4b 回执表达当前账号档案写入结果，环境登录刷新失败会保留成功并单独提醒。恢复材料与同目录写租约仍待 P14-5/4c | P14-3~5 |
| 进程观察与控制 | 凭证捕获/应用/恢复标记 → `ProcessControllerPort` / `WorkCnProcessController`；instance_tools/环境/切号/主库强制重置/库写 → `trae_instance` 与 `process_guard` | `ProcessControllerPort` 仍按目标数据库文件占用做 Work CN 凭据材料的只读观察，不负责启动/聚焦/关闭，也不替代统一 matcher。产品 + 目录三态 matcher 已进入启动/聚焦/关闭、环境状态、库写、环境删除、账号清理、强制重置与 blob keepalive；Unknown/枚举失败不放行危险写。强制重置缺命令行 caller 用例已断言 registry 不变；新增 CN 目标关闭测试证明只向 CN PID 发 taskkill、并行 Work 仍保留（定向 1/1，基础设施 crate 全测通过）。P14-2 仍待 ProcessController caller 级 Unknown/查询失败零写断言、focus PID 关系断言及 P14-9 真实验收 | P14-2 |
| CN 选择/切换 | 账号页 → `AccountAdapter.selectToolAccount` → `switch_trae_cn_account` → ProductAccountStateStore | 页面契约明确叫 `toolSelection` / `isToolSelected`，只写 Trae Sync 自有指针，不改变官方客户端认证；账号快照保留产品来源以拒绝切产品后的迟到读取。“官方客户端身份”是从 Trae CN 客户端数据读取的观察结果，不能当成实时登录状态或工具选择结果。CN 原生身份切换及 CN 切换前后 Work 会话是否保持仍须 P14-8/9 验证 | P14-1/8 |
| 共享账号页 | 账号页外壳 → 账号页本体（Work 保留成熟原生事务 / CN AccountAdapter）→ 账号详情页 | 共用账号行映射选择状态、忙碌态、动作入口与标签；Work 保持原账号事务及原文案，详情的“使用中”只用本次 `observed_profile_id`，环境缓存 `current_profile_id` 仅作最后已知记录；未能实测时显示中性值。两产品详情共同字段顺序为登录状态→产品内身份/工具选择→最近验证，文案与来源按产品区分；CN 的选择只读写工具指针。详情复用设备模块与可选选择动作。完整字段/动作/回执矩阵及其余生命周期隔离仍在 P14-1c；platform 当前还从组件导入 LibraryRef | P14-1 |
| CN 库写入 | 库页/CN 详情 → library_archive → master_handover/master_archive → CN DB | Pending 快照关闭未验收的 CN archive/merge 写入口；命令执行阶段另有产品进程 Unknown/枚举失败拒绝。CN 项目归并继承 handover 提交状态错误码，成功后列表刷新失败使用 warning；CN archive 返回 `not_requested` 并说明需手动启动客户端。P14-4b 全量 caller/故障注入和 P14-9 真实操作仍待 | P14-1/2/4/9 |
| CN 签到 | CheckinPage → checkin_schedule::run_trae_cn_checkin_inner → trae_cn_checkin | 当前 WIP 以 pending 快照和命令门禁关闭普通 claim，并保留 status 查询；`run_trae_cn_checkin_inner` 直接 claim 且把任意 Ok 当 Claimed，不读取 claim.checked_in；status parser 把缺失/错类型 enable、checked_in 归 false。协议/必需字段校准与最终 status 复核未完成，不能把任何 Ok 当真实已领取 | P14-1a/7/9 |
| CN 页面隔离 | App → SettingsPanel/CheckinPage | 当前 WIP 让 Settings 只在 Work 读写 Work 设置，并将 Work/CN Checkin 分开挂载；仍需验证切产品时 Settings 已开始动作的迟到回执/通知与 CN 账号状态查询的代次隔离 | P14-1 |
| 库/账号异步 | App/账号页/库详情页/库页 | 当前 WIP 已按产品传 Settings/Checkin 上下文并在 Library 读取操作快照；库/详情迟到成功响应、旧库确认动作及账号页确认来源仍需逐路径核验，不能只看 effect cleanup | P14-1 |
| 归档/收编/合并兄弟调用 | library_archive/环境 → master_archive/master_handover | P14-4a WIP 已让 handover caller 使用提交后需处理/提交未知专用错误码；统一归档做提交后读回。P14-4b 已为切号/环境登录/收编保留启动与档案回执，并为 Work/CN 合并及环境页后续刷新失败单独提醒；P14-4c 已接入目标库物理目录租约，caller 级并发故障注入/全量回归尚未完成 | P14-4/5 |
| 远程设备 | 两种账号详情 → RemoteDevicesPanel → remote_devices → remote_device | 账号级操作例外；保留 CurrentDevice/回退、本机拒绝和退出影响全部绑定产品的单次确认 | P14-9 回归 |

P14-4b 最新增量（2026-10-02）：主库切号/环境登录回执带 `current_profile_saved`；切号/收编在此前数据操作完成后把启动失败返回为 `relaunch_outcome="failed"`，不再丢弃提交回执。账号页在档案写入、启动或刷新失败时保留成功结果并单列提醒；Work 与 CN 合并的 mutation 成功后刷新失败只发 warning。CN archive 已返回 `not_requested` 并提示用户手动启动。对应三份前端测试 94 项通过（库页在最后一处 CN 刷新调整后复跑 45/45），`pnpm typecheck`、root crate `cargo test -p trae-sync --lib`（155 通过/1 忽略）、三份 Rust 文件格式检查及 diff 检查通过。完整 caller 故障注入、workspace/infrastructure 全量重跑、E2E、持久恢复与 P14-4c 写租约仍待；P14-4 和 G2 未完成。

## 2026-10-02 P14-4c 写资源锁 WIP

`OperationLease::acquire` / `acquire_bound` 仍为目录库元数据路径保留 catalog 锁；`acquire_shared` 只锁逻辑数据位置；新增 `acquire_shared_for_path` 以物理父目录身份和资源名称派生 OS 独占锁，规范化路径别名后同资源互斥、不同目录可并行。生产调用覆盖主库切号、主库备份/迁移/账号数据清理、副环境登录/删除/启动/关闭、六个 Library 写入口（含 CN 归档与项目归并）；创建/重命名/删除副环境以及副环境库写使用容器短锁，再取得单环境锁后释放容器锁。EnvironmentRegistry 的 `ensure_master/create/rename/remove/set_current_profile`、RelayLedger 追加和主库切号 JSONL 追加使用 sidecar 短锁。后续存储审计又确认 `AccountRegistry`、`AutoCheckinStore`、`CredentialMaintenanceStateStore` 的 JSON 读改写此前无锁；最新 WIP 已为其加 sidecar 锁。`CheckinCredentialStore` 的每账号凭据包写入/恢复/删除使用 profile 锁，真实与 fixture 续期覆盖读取、换发到写回，自动维护在同一 profile 租约内完成退避检查和状态记账。OAuth 包与账号档案的共同候选发布和崩溃恢复仍由 P14-5a/5b 负责。读路径不获取短文件锁；锁冲突有脱敏 UI 提示。

新增检查包括物理路径别名、第二进程竞争、持锁进程被终止后可重取、独立数据目录并行、环境锁层级、注册表与台账并发更新，以及持 sidecar 锁时读仍可用；本增量新增账号档案/自动签到/维护状态文件锁等待与续期屏障用例。infrastructure lib 全量回归 859 通过/0 失败/7 忽略（58.36 秒）；首次运行的续期屏障用例在 5 秒完成等待超时，定向 1/1 通过后将完成等待延长到 30 秒，全量复跑通过。该批次根 crate lib 为 157 通过/0 失败/1 忽略；之后环境删除屏障增量将根 crate 最新计数推进至 158（见下一节）。7 个相关 Rust 文件 `rustfmt --check` 通过，`git diff --check` 退出码 0。整 crate `cargo fmt --check` 曾报告工作树其他 Rust 文件格式差异，未批量格式化无关文件。真实 caller 注入屏障、完整 workspace/Vitest 回归、E2E 或真实 TRAE 写入验收仍待，因此 P14-4c、P14-4、G2 均未勾完成。项目说明要求的 code-review-graph 工具当前不可用，本次仍以源调用搜索、源码读取和测试为依据。

## 2026-10-02 定向执行增量

环境删除现已在等待进程检查时释放容器锁，并继续持有目标目录租约；删 registry 行前重新读取目标档案。屏障测试暂停环境 A 的进程检查，验证 A 的同目录仍被锁、环境 B 可并行删除且 registry 更新未丢；定向 1/1，根 crate lib 全量 158 通过/1 忽略。该用例验证删除 caller 的生命周期锁边界，不等于数据库/认证 writer 并发验收。只读并发审计确认主库切号与另一个主库 writer、同副环境登录与 Library 归档/归并/删除尚无真实副作用屏障测试；P14-4c 未完成。

P14-6 将 `login_already_finished` 与 `login_not_started` 作为登录已结束或收尾中的中性提示。账号页延迟总览刷新用例先以旧实现红测，再以修复后定向 2/2、全文件 57/57 验证；`pnpm typecheck` 通过。Work/CN 命令层仍缺真实 IPC transport 和与 P14-5 凭据发布边界的端到端竞态验收。

## 2026-10-02 适配边界复核

本节是当前代码形态的审计快照，不把后续平台预先实现出来；目标是让新产品沿稳定流程接入，并让每个业务域的共享范围与产品差异可见。

| 领域 | 当前实际边界 | 对后续产品接入的含义 |
|---|---|---|
| 产品标识/能力 | `ProductId` 与 `productRegistry` 以 Work/CN 两产品穷举；操作状态快照与执行前门禁是共享策略入口，但操作实现仍按产品和资源判断 | 第三个产品需要显式更新注册/穷举点；当前没有即插即用插件协议。继续把 capability 当“操作状态”使用，导航存在不代表功能已实现 |
| 账号页 | 账号页外壳共用账号页本体；Work 仍走 `null` 特例和成熟编排，CN 走 `AccountAdapter`。共同字段、顺序、动作位置仍有待 P14-1 统一 | 先固定共享账号视图与动作契约，再按产品能力映射；无需为了对称而立刻重写 Work，但迁移后应消除长期 null 特例 |
| 签到 | Work/CN 页面与后端路径显式分支；共用结果判定是接入目标，不是当前两条路径都已复用的事实 | 只有状态机、结果语义确实一致时抽共享 domain 规则；产品 HTTP、设备与 claim 协议留在各自适配 |
| 环境/账号切换 | Work 有完整环境及主库切号流程；CN 环境/原生激活仍未证实，账号页选择目前不等于官方登录 | 未证实功能应明确 absent/pending，不提前建 CN 空实现冒充能力；Work 的专用动作保留其所有权 |
| Library | 页面与 `LibraryRef` 共享；产品、数据根、读写资格及具体 SQL 操作仍由后端分支承接 | 共用页面/草稿契约与共用底层协议是不同层次，按产品单独验证写权限和事务 |
| 其他模块 | 插件与主要本机管理流程目前偏 Work；远程设备和部分凭据/基础操作是账号级共享；fixture、旧 Gate 和未接 Tauri 根的路径已在责任矩阵单列 | 按实际责任分类为共享、产品专用、内部/fixture、退役候选；无 UI caller 本身不足以证明可删 |

推荐的扩展规则是“稳定共性进共享契约，真实变化留产品适配，未实现能力显式 absent/pending”；不要预建五个平台占位类，也不要强迫所有功能都有同样的 adapter。接入步骤仍以 `PRODUCT_ONBOARDING.md` 为准，但该清单是目标流程，不代表当前每个领域已经完成适配迁移。

覆盖基线限制：本文全量源码清单仍标注 2026-09-30 `b90f12b`；后续 WIP 已变更多个模块并退役两条命令。虽然增量表记录了近期责任变化，P14-0/P14-9 收口前仍需对当前 HEAD 重新生成/核对文件、命令和实际调用归属；静态数量和字符串命中不能替代 caller/行为覆盖。

## 当前真实证据

执行前交叉复核：2026-09-30 原实施会话 `01a0f1c7-8b18-75a3-b189-28152864a916` 独立只读检查，主要风险与任务顺序一致；补充“账号页共同字段、信息顺序与操作位置”的验收要求，已并入 ADR-0034 决策 11 与 P14-1。双方均未在准备期间改源码、运行真实探针或操作官方客户端。原临时 handoff 早于这些正式文档，执行以新决策/计划为准。

| 操作 | 证据 | 能证明 / 不能证明 |
|---|---|---|
| CN 健康检测 | `.scratch/checkin-http/reports/trae-cn-health-probe-1790684645.json` | 2/2 账号健康与身份一致；不证明官方原生登录 |
| CN 库只读 | `trae_cn_library_read_probe.rs` 与 `.scratch/checkin-http/reports/trae-cn-library-read-probe-1790681305.json` | 实测 17 项目/39 会话/328 消息；不证明应用内写/重启稳定 |
| CN 一次凭据换发 | `.scratch/checkin-http/reports/trae-cn-refresh-acceptance-1790690922.json` | 14 项断言、同设备、完整代次轮换及健康复核；重启恢复未补验，不是 refresh 端点长期有效性的证明 |
| CN 交接原语 | 09-28 授权手动交接、`trae-cn-project-schema-probe-1790690693.json` | 同族 schema 与已执行原语的容忍证据；不能替代应用内归档/归并及重启专项验收 |
| CN status | `.scratch/checkin-http/reports/trae-cn-checkin-probe-20260928.json` | status 端点存在；不能证明推导的 claim 形状、领取成功与额度增量 |
| Rust/Vitest/Playwright 历史结果 | `tickets.new.md` P12/P13 已完成记录 | 仅对应当时版本；Playwright 使用 Vite preview/mock bridge，不能替代真实 Tauri 和官方客户端 |

正式实施需要把报告中的非敏感结论、版本、日期和验收范围关联到操作能力；不要复制 token、密钥、手机号或未经脱敏的全量响应到版本库。

## 22 个无生产前端直接引用命令的处理

| 分类 | 命令 | 当前处理 |
|---|---|---|
| 内部切换原语 4 | prepare_managed_account_switch / switch_account / switch_and_handoff / prepare_handoff_intent | 计划明确保留；标为内部，不默认接 UI 或删除 |
| 旧 Gate/恢复 7 | restore_operation / get_storage_root_state / migrate_storage_root_command / plan_storage_deletion / apply_storage_deletion / export_recovery_package_command / import_recovery_package_command | 有占位/禁用与旧模型；本期不承诺可用，不复活 Gate，不批量删 |
| fixture-only 1 | read_work_cn_state | 保留测试入口，不能误称生产浏览能力 |
| 无 UI 账号能力 3 | set_account_archived / get_account_storage_footprint / purge_account_records | 保留待明确需求与兼容评估；实际危险入口仍需对应守卫 |
| 替代/归属核对 7 | get_account_registry / get_managed_credential_statuses / capture_current_account_credential / refresh_managed_current_account / list_checkin_accounts / set_environment_current_account / get_backup_dir | 逐个追踪实际 caller 后决定；此矩阵不授权批量退役 |

`cancel_checkin_login` 在早期基线是反向断链；当前 WIP 已注册，begin/complete/cancel 均按产品与 attemptId 绑定。定向命令层和组件测试通过，但没有真实 Tauri IPC transport 集成验收。`ProductAccountPage` 已在 P11 删除，不要再次规划清理。前端 `accountLabel.ts` / `workbench_read.ts`、commands crate 未接主路径包装及旧 application 服务列为内部/候选，不按名字自动删除。

## 前端目录职责（2026-10-24 P6-1 回填）

本节补上重构后新目录的职责说明。目标结构见 [UI-CHARTER](UI-CHARTER.md) §2，但**当前树与目标结构尚未完全对齐**：`src/app/` 与 `src/contracts/` 还没有独立目录，壳层组件仍在 `src/components/`（CapabilitySection / NavigationRail / TitleBar / SettingsPanel / MasterSwitchDialog / RemoteDevicesPanel / StatusBadges），DTO 仍在 `src/types/`，事件订阅宿主在 `src/hooks/useTauriEvents.ts`。下表按**实际存在的目录**描述，不按目标结构宣称已完成迁移。

### 六个 feature 目录

每个 feature 是「一个交互只有一份实现」的落点：页面本体 + 声明式视图模型 + 产品 builder + 组件与 hook。产品差异只允许出现在 builder / 登记表里，页面不得判断 productId。

| 目录 | 文件/行数 | 页面入口 | 结构约定与职责 |
|---|---|---|---|
| [src/features/accounts/](../../src/features/accounts/) | 40 / 4,961 | `AccountsPage.tsx`（298 行） | `config.ts` 视图模型 + 能力表 + 文案；`registry.ts` 按 productId 分发的 builder 登记表（未登记产品 fail-closed 到空视图）；`traeAccounts.ts` / `workbuddyAccounts.ts` 两个产品 builder；`detailView.ts` / `detailSnapshot.ts` 详情六区；`pageAdapters.ts` 产品接缝表；`actionCommands` / `detailActionCommands` / `poolActionCommands` 动作封装；`components/` 头部、行、详情、弹层与九个 hook。 |
| [src/features/checkin/](../../src/features/checkin/) | 10 / 1,228 | `CheckinPage.tsx`（294 行） | 同上骨架：`config.ts` 视图模型 + 文案，`traeCheckin.ts` / `workbuddyCheckin.ts` builder，`CheckinRows.tsx` 行渲染，`components/` 批量条、页头、自动签到结果与 `useCheckinRun` / `useCheckinLiveEvents`。 |
| [src/features/environment/](../../src/features/environment/) | 24 / 3,099 | `EnvironmentPage.tsx`（241 行） | `config.ts` / `registry.ts` / `traeEnvironment.ts` / `workbuddyEnvironment.ts`；`components/` 装配（`assembleEnvironment.tsx`）、动作分发（`environmentActionDispatch.ts` + `environmentActionIds.ts`）、卡片、体检、删除/收编/登录/改名弹层及九个 hook。 |
| [src/features/library/](../../src/features/library/) | 36 / 4,563 | `LibraryPage.tsx`（299 行） | `config.ts` / `registry.ts` / `traeLibrary.ts` / `workbuddyLibrary.ts`；纯逻辑先抽再迁：`sessionTree.ts`（会话树）、`relayLegs.ts`（接力腿）、`infoTab.ts` / `pluginTab.ts`；`components/` 壳、树、查看器、归档条、批量条、删除/归并弹层与十余个 hook。 |
| [src/features/overview/](../../src/features/overview/) | 4 / 571 | `OverviewPage.tsx`（193 行） | 无 `registry.ts`：`stats.ts` 就是声明式登记表，`traeStats.ts` / `workbuddyStats.ts` 提供各自视图；页面只渲染本文件导出的视图模型。 |
| [src/features/settings/](../../src/features/settings/) | 14 / 1,624 | `SettingsPage.tsx`（110 行） | `config.ts` / `registry.ts` / `traeSettings.ts` / `workbuddySettings.ts`；`components/` 自动签到段、备份段与弹层、`settingsActionsTrae.ts` / `settingsActionsWb.ts` 动作、`settingsTypes.ts`、三个 hook。 |

### 五个 platform 模块

`src/platform/` 是唯一允许出现 `invoke(` 的层（宪章 §4.4）；它只做 IPC 转接与产品注册，不实现编排、不吞错误、不改后端契约。平台差异用「同一文件内按 productId 选择实现」表达，不新增目录。

| 模块 | 文件 | 职责与边界 |
|---|---|---|
| ① 产品注册与能力开关 | `productRegistry.ts`（67 行）、`productUiRegistry.ts`（111 行） | 产品条目装载、`SectionKey` 五页导航、`CapabilityState` 三态（ready / pending / absent）、`sectionStatesFor` 与前端静态 UI 能力表 `PRODUCT_UI_REGISTRY`（ADR-0036 能力开关、ADR-0032 三态）。两个文件必须并列表述同一事实，不得出现第三处产品分支。 |
| ② 账号适配器 | `accountAdapter.ts`（173 行）、`workbuddyAdapter.ts`（294 行） | 账号能力清单、账号视图/快照契约与按产品选择实现（`accountAdapterFor`）；WorkBuddy 的登录、健康、凭据刷新、切号与批量领取封装。ADR-0029/0030/0034 的「能力而非命令」边界落在这里。 |
| ③ 账号/签到 IPC 封装 | `accounts.ts`（193 行）、`checkin.ts`（162 行） | 账号域 26 条命令与签到域快照/领取/取消的类型化封装；只做 invoke 直返，错误原文抛出，由调用方按 `safeUiError` 映射文案。 |
| ④ 环境/库/设置/总览 IPC 封装 | `environment.ts`（163 行）、`environmentPlugins.ts`（39 行）、`library.ts`（145 行）、`libraryDelete.ts`（85 行）、`libraryPlugins.ts`（10 行）、`settings.ts`（70 行）、`overview.ts`（78 行） | 环境生命周期、插件市场/清单、库读取与破坏性写（删除/归档/归并）、设置与备份、总览装载。`libraryPlugins.ts` 只做库页 re-export，不重复包装；`environment.ts` 也 re-export `readEnvironmentState` / `readTraeInstanceStates`，避免第二份实现。 |
| ⑤ 通知桥 | `notify.ts`（95 行） | sonner 的唯一入口（宪章口径见模块注释）：任何组件不得直接 import `sonner`，一律经此模块发送通知、取 `Toaster`；对外只暴露 React 类型。事件订阅桥（Tauri 事件 → 前端状态）宿主在 `src/hooks/useTauriEvents.ts`，platform 侧不另设文件。 |

**跨层约束**：`platform/` 不得 import `ui/`、`features/`；`features/` 不得出现 `invoke(`；feature 之间禁止深路径 import——当前无 `public.ts`，实际共享面走 `src/components/shared/`、`src/platform/`、`src/types/`、`src/utils/`（2026-10-24 实测 feature 间零互相 import）。以上由 `scripts/ui-guard.mjs` 规则 4/5 静态拦截。

## 全量源码归属与命令登记

本节从磁盘实际文件生成，覆盖 `src` 的 .ts/.tsx/.css 与六个 Rust 源码位置。tests、examples、scripts 与配置是验证/构建辅助，单独列清单。责任代号按上方「责任与依赖」表的模块职责判定，不按文件路径判定。测试命中只表示字符串引用，不表示测试通过或充分覆盖。

### 源码清单（目录级）

<!-- gen:inventory:begin -->
**统计口径**（P6-2a 重建）：`src/` 只统计 .ts / .tsx / .css，共 200 个文件 / 29,506 行；Rust 只统计 .rs，共 127 个文件 / 121,335 行。行数按可见行计（有尾换行时等价 `wc -l`）。

**为什么只到目录级**：逐文件列举在 200+ 文件规模下维护成本高且必然过期——上一版清单只覆盖 154/200 个文件，另有 12 行指向重构中已删除的文件。目录级统计 + 生成脚本才是可持续口径：改代码后重跑脚本即可，不需要手工改行号或增删行。需要具体文件时以磁盘为准。

> 重跑：`node scripts/gen-module-map-inventory.mjs`；`--check` 只比对不写，退出码 1 表示文档过期。

#### 前端 `src/`（目录级）

| 目录 | 文件数 | 行数 | 职责 |
|---|---|---|---|
| [`src/`](../../src/) | 3 | 405 | 壳入口：App 编排、main 挂载、样式聚合入口 |
| [`src/components/`](../../src/components/) | 7 | 1,663 | 壳层组件（导航、标题栏、能力段、设置、切号、设备、徽章）；目标结构的 src/app/ 尚未拆分 |
| [`src/components/shared/`](../../src/components/shared/) | 3 | 191 | 跨 feature 共享控件（确认框、空态、产品徽章） |
| [`src/components/shared/account/`](../../src/components/shared/account/) | 4 | 492 | 账号行与列表控件、账号偏好（Work 与 CN 共用骨架） |
| [`src/components/shared/checkin/`](../../src/components/shared/checkin/) | 4 | 247 | 签到行、流转行、互斥等待倒计时与结果文案 |
| [`src/components/workbuddy/`](../../src/components/workbuddy/) | 1 | 15 | WorkBuddy 专属小控件 |
| [`src/features/accounts/`](../../src/features/accounts/) | 12 | 2,027 | 账号 feature：页面、视图模型、产品 builder、详情六区与动作封装 |
| [`src/features/accounts/components/`](../../src/features/accounts/components/) | 28 | 2,934 | 账号页组件与 hook（行、详情页、弹层、批量进度、登录与刷新流程） |
| [`src/features/checkin/`](../../src/features/checkin/) | 5 | 917 | 签到 feature：页面、视图模型、产品 builder |
| [`src/features/checkin/components/`](../../src/features/checkin/components/) | 5 | 311 | 签到页组件与 hook（批量条、页头、自动签到结果、运行与事件） |
| [`src/features/environment/`](../../src/features/environment/) | 5 | 809 | 环境 feature：页面、视图模型、产品 builder |
| [`src/features/environment/components/`](../../src/features/environment/components/) | 19 | 2,290 | 环境页装配、动作分发、卡片、体检、删除/收编/登录/改名弹层与 hook |
| [`src/features/library/`](../../src/features/library/) | 9 | 1,681 | 库 feature：页面、视图模型、产品 builder、会话树/接力腿/信息页/插件页纯逻辑 |
| [`src/features/library/components/`](../../src/features/library/components/) | 27 | 2,882 | 库页壳、树、查看器、归档与批量条、删除与归并弹层、hook |
| [`src/features/overview/`](../../src/features/overview/) | 4 | 571 | 总览 feature：页面与 stats 登记表（无 registry） |
| [`src/features/settings/`](../../src/features/settings/) | 5 | 628 | 设置 feature：页面、视图模型、产品 builder |
| [`src/features/settings/components/`](../../src/features/settings/components/) | 9 | 996 | 设置页组件与 hook（自动签到段、备份段与弹层、动作分产品） |
| [`src/hooks/`](../../src/hooks/) | 2 | 282 | 跨页 hook：归档草稿、Tauri 事件订阅宿主 |
| [`src/platform/`](../../src/platform/) | 14 | 2,015 | 唯一允许 invoke( 的 IPC 转接层 + 产品注册与能力开关（宪章 4.4） |
| [`src/styles/`](../../src/styles/) | 5 | 1,432 | L0-L3 样式：聚合入口、令牌、壳层基础、共享控件、通知、标题栏 |
| [`src/styles/pages/`](../../src/styles/pages/) | 7 | 3,557 | L4 各页面样式（按页面拆分） |
| [`src/types/`](../../src/types/) | 13 | 2,029 | DTO / 事件 / 命令契约；不直接触发业务 |
| [`src/ui/`](../../src/ui/) | 5 | 192 | 与业务无关的通用展示原子（徽章、空态、区块标题、统计块） |
| [`src/utils/`](../../src/utils/) | 4 | 940 | 纯工具：账号展示与标签、账号注册表、UI 错误脱敏 |
| **合计** | **200** | **29,506** | 脚本生成，与磁盘一致 |

重构残留空目录（无任何文件）：`src/components/product/trae/`、`src/pages/`。只影响导航心智、不含代码；清理或复用留待后续 UI 票决定，本文只登记事实。

#### Rust 源码（crate 级）

| 位置 | 文件数 | 行数 | 说明 |
|---|---|---|---|
| [`src-tauri/src/`](../../src-tauri/src/) | 22 | 30,863 | Tauri 根：命令注册与编排 |
| [`src-tauri/crates/commands/src/`](../../src-tauri/crates/commands/src/) | 1 | 1,520 | commands：IPC 薄壳 |
| [`src-tauri/crates/application/src/`](../../src-tauri/crates/application/src/) | 7 | 4,283 | application：用例编排 |
| [`src-tauri/crates/domain/src/`](../../src-tauri/crates/domain/src/) | 7 | 3,031 | domain：纯领域模型 |
| [`src-tauri/crates/infrastructure/src/`](../../src-tauri/crates/infrastructure/src/) | 80 | 80,567 | infrastructure：文件 / SQLCipher / HTTP 实现 |
| [`src-tauri/crates/ports/src/`](../../src-tauri/crates/ports/src/) | 10 | 1,071 | ports：端口 trait |
| **合计** | **127** | **121,335** | 五个 crate + Tauri 根，不含 tests / examples |

责任归属按上方「责任与依赖」表的模块职责判定，不按文件路径判定，因此目录级清单足以支撑归属核对。
<!-- gen:inventory:end -->

### 已注册 IPC 清单（Work/CN 范围 96 条）

定义文件定位来自函数名（命令清单以 `generate_handler!` 为准，增删命令请改脚本内的 COMMANDS 表）；引用列由脚本对 `src/`、`tests/`、`e2e/` 做命令名字符串静态扫描得到，只表示存在引用，不表示通过验收。表内「无」不是删除许可，24 条分类见上表。

<!-- gen:commands:begin -->
**覆盖率核对**：`src-tauri/src/lib.rs` 的 `generate_handler!` 当前实际注册 134 条命令，本表登记 96 条；未登记的 42 条集中在 WorkBuddy 域（list_products / get_current_product / set_current_product / get_workbuddy_session_health 等），属于另一条产品线的命令面，本表按 P14 的 Work/CN 范围登记，未展开。

| 命令 | 定义模块 | 生产前端字符串命中 | 测试 / mock 字符串命中 |
|---|---|---|---|
| apply_master_archive_changes | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/libraryDelete.ts](../../src/platform/libraryDelete.ts) | [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| apply_storage_deletion | [src-tauri/src/lib.rs](../../src-tauri/src/lib.rs) | 无 | 无 |
| begin_checkin_login | [src-tauri/src/oauth_login.rs](../../src-tauri/src/oauth_login.rs) | [src/platform/accounts.ts](../../src/platform/accounts.ts) | [e2e/library-sessions.spec.ts](../../e2e/library-sessions.spec.ts) |
| begin_trae_cn_login | [src-tauri/src/oauth_login.rs](../../src-tauri/src/oauth_login.rs) | [src/platform/accountAdapter.ts](../../src/platform/accountAdapter.ts)<br>[src/platform/accounts.ts](../../src/platform/accounts.ts) | [tests/AccountAdapter.test.tsx](../../tests/AccountAdapter.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| browse_plugin_market | [src-tauri/src/plugins.rs](../../src-tauri/src/plugins.rs) | [src/platform/environmentPlugins.ts](../../src/platform/environmentPlugins.ts) | [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| cancel_checkin | [src-tauri/src/checkin_schedule.rs](../../src-tauri/src/checkin_schedule.rs) | [src/platform/checkin.ts](../../src/platform/checkin.ts) | [e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| cancel_checkin_login | [src-tauri/src/oauth_login.rs](../../src-tauri/src/oauth_login.rs) | [src/platform/accounts.ts](../../src/platform/accounts.ts) | 无 |
| cancel_trae_cn_login | [src-tauri/src/oauth_login.rs](../../src-tauri/src/oauth_login.rs) | [src/platform/accountAdapter.ts](../../src/platform/accountAdapter.ts)<br>[src/platform/accounts.ts](../../src/platform/accounts.ts) | [tests/AccountAdapter.test.tsx](../../tests/AccountAdapter.test.tsx) |
| capture_current_account_credential | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | 无 | 无 |
| check_trae_cn_account_health | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | [src/platform/accountAdapter.ts](../../src/platform/accountAdapter.ts)<br>[src/platform/accounts.ts](../../src/platform/accounts.ts) | [tests/AccountAdapter.test.tsx](../../tests/AccountAdapter.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| cleanup_master_empty_projects | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/environment.ts](../../src/platform/environment.ts) | [tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx) |
| clear_remote_device | [src-tauri/src/remote_devices.rs](../../src-tauri/src/remote_devices.rs) | [src/components/RemoteDevicesPanel.tsx](../../src/components/RemoteDevicesPanel.tsx) | [tests/RemoteDevicesPanel.test.tsx](../../tests/RemoteDevicesPanel.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| close_environment_instance | [src-tauri/src/environment.rs](../../src-tauri/src/environment.rs) | [src/platform/environment.ts](../../src/platform/environment.ts) | [tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx) |
| complete_checkin_login | [src-tauri/src/oauth_login.rs](../../src-tauri/src/oauth_login.rs) | [src/platform/accounts.ts](../../src/platform/accounts.ts) | [e2e/library-sessions.spec.ts](../../e2e/library-sessions.spec.ts) |
| complete_trae_cn_login | [src-tauri/src/oauth_login.rs](../../src-tauri/src/oauth_login.rs) | [src/platform/accountAdapter.ts](../../src/platform/accountAdapter.ts)<br>[src/platform/accounts.ts](../../src/platform/accounts.ts) | [tests/AccountAdapter.test.tsx](../../tests/AccountAdapter.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| create_environment | [src-tauri/src/environment.rs](../../src-tauri/src/environment.rs) | [src/platform/environment.ts](../../src/platform/environment.ts) | [tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx) |
| create_master_backup | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/environment.ts](../../src/platform/environment.ts) | [tests/SettingsPanel.test.tsx](../../tests/SettingsPanel.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| delete_environment | [src-tauri/src/environment.rs](../../src-tauri/src/environment.rs) | [src/platform/environment.ts](../../src/platform/environment.ts) | [tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx) |
| delete_master_sessions | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/libraryDelete.ts](../../src/platform/libraryDelete.ts) | [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| export_recovery_package_command | [src-tauri/src/lib.rs](../../src-tauri/src/lib.rs) | 无 | 无 |
| force_reset_master_current_account | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | [src/components/MasterSwitchDialog.tsx](../../src/components/MasterSwitchDialog.tsx)<br>[src/platform/accounts.ts](../../src/platform/accounts.ts) | [tests/MasterSwitchDialog.test.tsx](../../tests/MasterSwitchDialog.test.tsx) |
| get_account_registry | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | 无 | 无 |
| get_account_storage_footprint | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | 无 | 无 |
| get_archive_draft | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/hooks/useArchiveDraft.ts](../../src/hooks/useArchiveDraft.ts)<br>[src/platform/library.ts](../../src/platform/library.ts) | [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[tests/useArchiveDraft.test.tsx](../../tests/useArchiveDraft.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| get_auto_checkin_settings | [src-tauri/src/checkin_schedule.rs](../../src-tauri/src/checkin_schedule.rs) | [src/platform/checkin.ts](../../src/platform/checkin.ts)<br>[src/platform/settings.ts](../../src/platform/settings.ts) | [tests/App.test.tsx](../../tests/App.test.tsx)<br>[tests/CheckinPage.test.tsx](../../tests/CheckinPage.test.tsx)<br>[tests/CheckinShared.test.tsx](../../tests/CheckinShared.test.tsx)<br>[tests/settingsPage.test.tsx](../../tests/settingsPage.test.tsx)<br>[tests/SettingsPanel.test.tsx](../../tests/SettingsPanel.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts)<br>[e2e/visual-contract.mjs](../../e2e/visual-contract.mjs)<br>[e2e/visual-shots.mjs](../../e2e/visual-shots.mjs) |
| get_backup_dir | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | 无 | 无 |
| get_backup_retention | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/settings.ts](../../src/platform/settings.ts) | [tests/App.test.tsx](../../tests/App.test.tsx)<br>[tests/settingsPage.test.tsx](../../tests/settingsPage.test.tsx)<br>[tests/SettingsPanel.test.tsx](../../tests/SettingsPanel.test.tsx) |
| get_checkin_capability | [src-tauri/src/checkin_schedule.rs](../../src-tauri/src/checkin_schedule.rs) | [src/platform/accounts.ts](../../src/platform/accounts.ts)<br>[src/platform/checkin.ts](../../src/platform/checkin.ts)<br>[src/platform/overview.ts](../../src/platform/overview.ts) | [tests/AccountAdapter.test.tsx](../../tests/AccountAdapter.test.tsx)<br>[tests/accountsPage.test.tsx](../../tests/accountsPage.test.tsx)<br>[tests/accountsPageBatchProgress.test.tsx](../../tests/accountsPageBatchProgress.test.tsx)<br>[tests/App.test.tsx](../../tests/App.test.tsx)<br>[tests/CheckinPage.test.tsx](../../tests/CheckinPage.test.tsx)<br>[tests/CheckinShared.test.tsx](../../tests/CheckinShared.test.tsx)<br>[tests/OverviewPage.test.tsx](../../tests/OverviewPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts)<br>[e2e/visual-contract.mjs](../../e2e/visual-contract.mjs)<br>[e2e/visual-shots.mjs](../../e2e/visual-shots.mjs) |
| get_checkin_overview | [src-tauri/src/checkin_schedule.rs](../../src-tauri/src/checkin_schedule.rs) | [src/platform/accounts.ts](../../src/platform/accounts.ts)<br>[src/platform/checkin.ts](../../src/platform/checkin.ts)<br>[src/platform/overview.ts](../../src/platform/overview.ts) | [tests/AccountAdapter.test.tsx](../../tests/AccountAdapter.test.tsx)<br>[tests/accountsPage.test.tsx](../../tests/accountsPage.test.tsx)<br>[tests/accountsPageBatchProgress.test.tsx](../../tests/accountsPageBatchProgress.test.tsx)<br>[tests/CheckinPage.test.tsx](../../tests/CheckinPage.test.tsx)<br>[tests/CheckinShared.test.tsx](../../tests/CheckinShared.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts)<br>[e2e/visual-contract.mjs](../../e2e/visual-contract.mjs)<br>[e2e/visual-shots.mjs](../../e2e/visual-shots.mjs) |
| get_environment_delete_preview | [src-tauri/src/environment.rs](../../src-tauri/src/environment.rs) | [src/platform/environment.ts](../../src/platform/environment.ts) | [tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx) |
| get_environment_state | [src-tauri/src/environment.rs](../../src-tauri/src/environment.rs) | [src/platform/accounts.ts](../../src/platform/accounts.ts)<br>[src/platform/overview.ts](../../src/platform/overview.ts) | [tests/AccountAdapter.test.tsx](../../tests/AccountAdapter.test.tsx)<br>[tests/accountsPage.test.tsx](../../tests/accountsPage.test.tsx)<br>[tests/accountsPageBatchProgress.test.tsx](../../tests/accountsPageBatchProgress.test.tsx)<br>[tests/App.test.tsx](../../tests/App.test.tsx)<br>[tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx)<br>[tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[tests/OverviewPage.test.tsx](../../tests/OverviewPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts)<br>[e2e/visual-contract.mjs](../../e2e/visual-contract.mjs)<br>[e2e/visual-shots.mjs](../../e2e/visual-shots.mjs) |
| get_managed_account_state | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | [src/platform/accounts.ts](../../src/platform/accounts.ts)<br>[src/platform/checkin.ts](../../src/platform/checkin.ts) | [tests/AccountAdapter.test.tsx](../../tests/AccountAdapter.test.tsx)<br>[tests/accountsPage.test.tsx](../../tests/accountsPage.test.tsx)<br>[tests/accountsPageBatchProgress.test.tsx](../../tests/accountsPageBatchProgress.test.tsx)<br>[tests/CheckinPage.test.tsx](../../tests/CheckinPage.test.tsx)<br>[tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts)<br>[e2e/visual-contract.mjs](../../e2e/visual-contract.mjs)<br>[e2e/visual-shots.mjs](../../e2e/visual-shots.mjs) |
| get_managed_credential_statuses | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | 无 | 无 |
| get_master_backup_chain | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/environment.ts](../../src/platform/environment.ts) | [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[tests/settingsPage.test.tsx](../../tests/settingsPage.test.tsx)<br>[tests/SettingsPanel.test.tsx](../../tests/SettingsPanel.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts)<br>[e2e/visual-contract.mjs](../../e2e/visual-contract.mjs) |
| get_master_checkup | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/environment.ts](../../src/platform/environment.ts) | [tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| get_master_history | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/library.ts](../../src/platform/library.ts) | [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts)<br>[e2e/visual-contract.mjs](../../e2e/visual-contract.mjs)<br>[e2e/visual-shots.mjs](../../e2e/visual-shots.mjs) |
| get_master_library_stats | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/library.ts](../../src/platform/library.ts)<br>[src/platform/overview.ts](../../src/platform/overview.ts) | [tests/App.test.tsx](../../tests/App.test.tsx)<br>[tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx)<br>[tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[tests/OverviewPage.test.tsx](../../tests/OverviewPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts)<br>[e2e/visual-contract.mjs](../../e2e/visual-contract.mjs) |
| get_master_self_check | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | [src/platform/environment.ts](../../src/platform/environment.ts) | [tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| get_master_session_messages | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/library.ts](../../src/platform/library.ts) | [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts)<br>[e2e/visual-contract.mjs](../../e2e/visual-contract.mjs) |
| get_master_verification | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/library.ts](../../src/platform/library.ts) | [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx) |
| get_plugin_tab_state | [src-tauri/src/plugins.rs](../../src-tauri/src/plugins.rs) | [src/platform/environmentPlugins.ts](../../src/platform/environmentPlugins.ts) | [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| get_product_identity_state | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | [src/platform/accountAdapter.ts](../../src/platform/accountAdapter.ts)<br>[src/platform/accounts.ts](../../src/platform/accounts.ts) | [tests/AccountAdapter.test.tsx](../../tests/AccountAdapter.test.tsx)<br>[tests/App.test.tsx](../../tests/App.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| get_product_operation_states | [src-tauri/src/product_operations.rs](../../src-tauri/src/product_operations.rs) | [src/platform/library.ts](../../src/platform/library.ts) | [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| get_relay_ledger | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/library.ts](../../src/platform/library.ts) | [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts)<br>[e2e/visual-contract.mjs](../../e2e/visual-contract.mjs)<br>[e2e/visual-shots.mjs](../../e2e/visual-shots.mjs) |
| get_storage_root_state | [src-tauri/src/lib.rs](../../src-tauri/src/lib.rs) | 无 | 无 |
| get_trae_cn_checkin_status | [src-tauri/src/checkin_schedule.rs](../../src-tauri/src/checkin_schedule.rs) | 无 | 无 |
| get_trae_instance_states | [src-tauri/src/instance_tools.rs](../../src-tauri/src/instance_tools.rs) | [src/platform/accounts.ts](../../src/platform/accounts.ts) | [tests/accountsPage.test.tsx](../../tests/accountsPage.test.tsx)<br>[tests/accountsPageBatchProgress.test.tsx](../../tests/accountsPageBatchProgress.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts)<br>[e2e/visual-contract.mjs](../../e2e/visual-contract.mjs)<br>[e2e/visual-shots.mjs](../../e2e/visual-shots.mjs) |
| get_workspace_state | [src-tauri/src/lib.rs](../../src-tauri/src/lib.rs) | [src/App.tsx](../../src/App.tsx)<br>[src/types/workspace.ts](../../src/types/workspace.ts) | [tests/App.test.tsx](../../tests/App.test.tsx)<br>[tests/fakeInvoke.test.ts](../../tests/fakeInvoke.test.ts)<br>[tests/helpers/fakeInvoke.ts](../../tests/helpers/fakeInvoke.ts)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts)<br>[e2e/visual-contract.mjs](../../e2e/visual-contract.mjs)<br>[e2e/visual-shots.mjs](../../e2e/visual-shots.mjs) |
| import_recovery_package_command | [src-tauri/src/lib.rs](../../src-tauri/src/lib.rs) | 无 | 无 |
| incorporate_master_records | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/environment.ts](../../src/platform/environment.ts) | [tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx) |
| install_plugin | [src-tauri/src/plugins.rs](../../src-tauri/src/plugins.rs) | [src/platform/environmentPlugins.ts](../../src/platform/environmentPlugins.ts) | [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| launch_environment | [src-tauri/src/environment.rs](../../src-tauri/src/environment.rs) | [src/platform/environment.ts](../../src/platform/environment.ts) | 无 |
| launch_master_library | [src-tauri/src/environment.rs](../../src-tauri/src/environment.rs) | [src/platform/environment.ts](../../src/platform/environment.ts) | [tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx)<br>[tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| list_checkin_accounts | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | 无 | 无 |
| list_environments | [src-tauri/src/environment.rs](../../src-tauri/src/environment.rs) | [src/platform/environment.ts](../../src/platform/environment.ts) | [tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx) |
| list_remote_devices | [src-tauri/src/remote_devices.rs](../../src-tauri/src/remote_devices.rs) | [src/components/RemoteDevicesPanel.tsx](../../src/components/RemoteDevicesPanel.tsx) | [tests/RemoteDevicesPanel.test.tsx](../../tests/RemoteDevicesPanel.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| list_trae_cn_accounts | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | [src/platform/accountAdapter.ts](../../src/platform/accountAdapter.ts)<br>[src/platform/accounts.ts](../../src/platform/accounts.ts) | [tests/AccountAdapter.test.tsx](../../tests/AccountAdapter.test.tsx)<br>[tests/App.test.tsx](../../tests/App.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| list_trae_cn_foreign_projects | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/library.ts](../../src/platform/library.ts) | [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| login_environment | [src-tauri/src/environment.rs](../../src-tauri/src/environment.rs) | [src/platform/environment.ts](../../src/platform/environment.ts) | [tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx) |
| merge_master_projects | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/libraryDelete.ts](../../src/platform/libraryDelete.ts) | [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| merge_trae_cn_project | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/libraryDelete.ts](../../src/platform/libraryDelete.ts) | [e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| migrate_master_backups | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/settings.ts](../../src/platform/settings.ts) | [tests/SettingsPanel.test.tsx](../../tests/SettingsPanel.test.tsx) |
| migrate_storage_root_command | [src-tauri/src/lib.rs](../../src-tauri/src/lib.rs) | 无 | 无 |
| open_device_management_docs | [src-tauri/src/remote_devices.rs](../../src-tauri/src/remote_devices.rs) | [src/components/RemoteDevicesPanel.tsx](../../src/components/RemoteDevicesPanel.tsx) | [tests/RemoteDevicesPanel.test.tsx](../../tests/RemoteDevicesPanel.test.tsx) |
| plan_storage_deletion | [src-tauri/src/lib.rs](../../src-tauri/src/lib.rs) | 无 | 无 |
| prepare_handoff_intent | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | 无 | 无 |
| prepare_managed_account_switch | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | 无 | 无 |
| preview_master_switch_plugins | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | [src/components/MasterSwitchDialog.tsx](../../src/components/MasterSwitchDialog.tsx)<br>[src/platform/accounts.ts](../../src/platform/accounts.ts) | [tests/MasterSwitchDialog.test.tsx](../../tests/MasterSwitchDialog.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| preview_trae_cn_project_merge | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/libraryDelete.ts](../../src/platform/libraryDelete.ts) | [e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| purge_account_records | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | 无 | 无 |
| read_work_cn_state | [src-tauri/src/lib.rs](../../src-tauri/src/lib.rs) | [src/types/workbench_read.ts](../../src/types/workbench_read.ts) | [tests/App.test.tsx](../../tests/App.test.tsx) |
| refresh_checkin_credentials | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | [src/platform/accounts.ts](../../src/platform/accounts.ts) | [e2e/library-sessions.spec.ts](../../e2e/library-sessions.spec.ts)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| refresh_checkin_credits | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | [src/platform/accounts.ts](../../src/platform/accounts.ts) | [tests/accountsPageBatchProgress.test.tsx](../../tests/accountsPageBatchProgress.test.tsx)<br>[tests/CheckinPage.test.tsx](../../tests/CheckinPage.test.tsx) |
| refresh_managed_current_account | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | 无 | [e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| refresh_trae_cn_credentials | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | 无 | 无 |
| remove_checkin_account | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | [src/platform/accounts.ts](../../src/platform/accounts.ts) | 无 |
| rename_environment | [src-tauri/src/environment.rs](../../src-tauri/src/environment.rs) | [src/platform/environment.ts](../../src/platform/environment.ts) | [tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx) |
| repair_master_current_account | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | [src/platform/environment.ts](../../src/platform/environment.ts) | [tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx) |
| reset_checkin_device | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | [src/platform/accounts.ts](../../src/platform/accounts.ts) | 无 |
| restore_operation | [src-tauri/src/lib.rs](../../src-tauri/src/lib.rs) | 无 | 无 |
| run_checkin | [src-tauri/src/checkin_schedule.rs](../../src-tauri/src/checkin_schedule.rs) | [src/platform/accounts.ts](../../src/platform/accounts.ts)<br>[src/platform/checkin.ts](../../src/platform/checkin.ts) | [tests/CheckinPage.test.tsx](../../tests/CheckinPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| run_trae_cn_checkin | [src-tauri/src/checkin_schedule.rs](../../src-tauri/src/checkin_schedule.rs) | 无 | 无 |
| set_account_archived | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | 无 | 无 |
| set_account_auto_checkin | [src-tauri/src/checkin_schedule.rs](../../src-tauri/src/checkin_schedule.rs) | [src/platform/accounts.ts](../../src/platform/accounts.ts) | 无 |
| set_account_display_name | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | [src/platform/accounts.ts](../../src/platform/accounts.ts) | 无 |
| set_account_mobile | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | [src/platform/accounts.ts](../../src/platform/accounts.ts) | 无 |
| set_archive_draft | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/hooks/useArchiveDraft.ts](../../src/hooks/useArchiveDraft.ts)<br>[src/platform/library.ts](../../src/platform/library.ts) | [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[tests/useArchiveDraft.test.tsx](../../tests/useArchiveDraft.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| set_auto_checkin_settings | [src-tauri/src/checkin_schedule.rs](../../src-tauri/src/checkin_schedule.rs) | [src/platform/settings.ts](../../src/platform/settings.ts) | [tests/SettingsPanel.test.tsx](../../tests/SettingsPanel.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| set_backup_dir | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/settings.ts](../../src/platform/settings.ts) | [tests/SettingsPanel.test.tsx](../../tests/SettingsPanel.test.tsx) |
| set_backup_retention | [src-tauri/src/library_archive.rs](../../src-tauri/src/library_archive.rs) | [src/platform/settings.ts](../../src/platform/settings.ts) | [tests/SettingsPanel.test.tsx](../../tests/SettingsPanel.test.tsx) |
| set_environment_current_account | [src-tauri/src/environment.rs](../../src-tauri/src/environment.rs) | 无 | 无 |
| switch_account | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | 无 | 无 |
| switch_and_handoff | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | 无 | 无 |
| switch_master_account | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | [src/components/MasterSwitchDialog.tsx](../../src/components/MasterSwitchDialog.tsx)<br>[src/platform/accounts.ts](../../src/platform/accounts.ts) | [tests/MasterSwitchDialog.test.tsx](../../tests/MasterSwitchDialog.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| switch_trae_cn_account | [src-tauri/src/credential_handover.rs](../../src-tauri/src/credential_handover.rs) | [src/platform/accountAdapter.ts](../../src/platform/accountAdapter.ts)<br>[src/platform/accounts.ts](../../src/platform/accounts.ts) | [tests/AccountAdapter.test.tsx](../../tests/AccountAdapter.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
| uninstall_plugin_everywhere | [src-tauri/src/plugins.rs](../../src-tauri/src/plugins.rs) | [src/platform/environmentPlugins.ts](../../src/platform/environmentPlugins.ts) | [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)<br>[e2e/mock-bridge.ts](../../e2e/mock-bridge.ts) |
<!-- gen:commands:end -->

### 测试、探针与构建辅助

以下是辅助入口，不能当生产已交付功能；真实探针/示例先读授权说明，不随普通测试自动运行。根配置与 Cargo 清单继续由现有构建工具管理，不为本次新增包装。

<!-- gen:aux:begin -->
- [e2e/layout-diagnostics.spec.ts](../../e2e/layout-diagnostics.spec.ts)
- [e2e/library-sessions.spec.ts](../../e2e/library-sessions.spec.ts)
- [e2e/mock-bridge.ts](../../e2e/mock-bridge.ts)
- [e2e/remote-devices.spec.ts](../../e2e/remote-devices.spec.ts)
- [e2e/ui-acceptance.spec.ts](../../e2e/ui-acceptance.spec.ts)
- [e2e/visual-contract.mjs](../../e2e/visual-contract.mjs)
- [e2e/visual-shots.mjs](../../e2e/visual-shots.mjs)
- [e2e/workbuddy-accounts.spec.ts](../../e2e/workbuddy-accounts.spec.ts)
- [e2e/workbuddy-overview.spec.ts](../../e2e/workbuddy-overview.spec.ts)
- [scripts/build-verifier-with-provenance.ps1](../../scripts/build-verifier-with-provenance.ps1)
- [scripts/cleanup-dev-instance.ps1](../../scripts/cleanup-dev-instance.ps1)
- [scripts/desktop-launch.bat](../../scripts/desktop-launch.bat)
- [scripts/dev-tauri.ps1](../../scripts/dev-tauri.ps1)
- [scripts/diag-recyclebin.py](../../scripts/diag-recyclebin.py)
- [scripts/drive-real-inventory-gate.mjs](../../scripts/drive-real-inventory-gate.mjs)
- [scripts/gen-command-union.mjs](../../scripts/gen-command-union.mjs)
- [scripts/gen-module-map-inventory.mjs](../../scripts/gen-module-map-inventory.mjs)
- [scripts/prevalidate-packaged-ui.ps1](../../scripts/prevalidate-packaged-ui.ps1)
- [scripts/real-inventory-gate.ps1](../../scripts/real-inventory-gate.ps1)
- [scripts/record-release-provenance.ps1](../../scripts/record-release-provenance.ps1)
- [scripts/smoke-release-installer.ps1](../../scripts/smoke-release-installer.ps1)
- [scripts/ui-guard.mjs](../../scripts/ui-guard.mjs)
- [scripts/verify-packaged-ui.mjs](../../scripts/verify-packaged-ui.mjs)
- [src-tauri/crates/infrastructure/examples/blob_inspect_probe.rs](../../src-tauri/crates/infrastructure/examples/blob_inspect_probe.rs)
- [src-tauri/crates/infrastructure/examples/e2_donor_probe.rs](../../src-tauri/crates/infrastructure/examples/e2_donor_probe.rs)
- [src-tauri/crates/infrastructure/examples/master_login_probe.rs](../../src-tauri/crates/infrastructure/examples/master_login_probe.rs)
- [src-tauri/crates/infrastructure/examples/migrate_backups_p16.rs](../../src-tauri/crates/infrastructure/examples/migrate_backups_p16.rs)
- [src-tauri/crates/infrastructure/examples/migrate_fused_login_plain_p17.rs](../../src-tauri/crates/infrastructure/examples/migrate_fused_login_plain_p17.rs)
- [src-tauri/crates/infrastructure/examples/migrate_workbuddy_login_plain_p17.rs](../../src-tauri/crates/infrastructure/examples/migrate_workbuddy_login_plain_p17.rs)
- [src-tauri/crates/infrastructure/examples/remint_cooldown_probe.rs](../../src-tauri/crates/infrastructure/examples/remint_cooldown_probe.rs)
- [src-tauri/crates/infrastructure/examples/rolling_backup_master_p17.rs](../../src-tauri/crates/infrastructure/examples/rolling_backup_master_p17.rs)
- [src-tauri/crates/infrastructure/examples/unified_lib_probe.rs](../../src-tauri/crates/infrastructure/examples/unified_lib_probe.rs)
- [src-tauri/crates/infrastructure/examples/unified_mint_probe.rs](../../src-tauri/crates/infrastructure/examples/unified_mint_probe.rs)
- [src-tauri/crates/infrastructure/examples/unified_schema_probe.rs](../../src-tauri/crates/infrastructure/examples/unified_schema_probe.rs)
- [src-tauri/crates/infrastructure/examples/w0_probe.rs](../../src-tauri/crates/infrastructure/examples/w0_probe.rs)
- [src-tauri/crates/infrastructure/examples/wb_switch_experiment_executor.rs](../../src-tauri/crates/infrastructure/examples/wb_switch_experiment_executor.rs)
- [src-tauri/crates/infrastructure/examples/wbipc_probe_p17.rs](../../src-tauri/crates/infrastructure/examples/wbipc_probe_p17.rs)
- [src-tauri/crates/infrastructure/examples/workbuddy_backup_p17.rs](../../src-tauri/crates/infrastructure/examples/workbuddy_backup_p17.rs)
- [src-tauri/crates/infrastructure/examples/workbuddy_capture_mint_p17.rs](../../src-tauri/crates/infrastructure/examples/workbuddy_capture_mint_p17.rs)
- [src-tauri/crates/infrastructure/examples/workbuddy_capture_probe_p17.rs](../../src-tauri/crates/infrastructure/examples/workbuddy_capture_probe_p17.rs)
- [src-tauri/crates/infrastructure/examples/workbuddy_checkin_probe_p17.rs](../../src-tauri/crates/infrastructure/examples/workbuddy_checkin_probe_p17.rs)
- [src-tauri/crates/infrastructure/examples/workbuddy_session_probe_p17.rs](../../src-tauri/crates/infrastructure/examples/workbuddy_session_probe_p17.rs)
- [src-tauri/crates/infrastructure/tests/catalog_sidecar_contract.rs](../../src-tauri/crates/infrastructure/tests/catalog_sidecar_contract.rs)
- [src-tauri/crates/infrastructure/tests/real_account_acceptance.rs](../../src-tauri/crates/infrastructure/tests/real_account_acceptance.rs)
- [src-tauri/crates/infrastructure/tests/real_clone_acceptance.rs](../../src-tauri/crates/infrastructure/tests/real_clone_acceptance.rs)
- [src-tauri/crates/infrastructure/tests/real_device_probe.rs](../../src-tauri/crates/infrastructure/tests/real_device_probe.rs)
- [src-tauri/crates/infrastructure/tests/real_process_observation.rs](../../src-tauri/crates/infrastructure/tests/real_process_observation.rs)
- [src-tauri/crates/infrastructure/tests/recovery_dpapi_current_profile.rs](../../src-tauri/crates/infrastructure/tests/recovery_dpapi_current_profile.rs)
- [src-tauri/crates/infrastructure/tests/recovery_import_fixture.rs](../../src-tauri/crates/infrastructure/tests/recovery_import_fixture.rs)
- [src-tauri/crates/infrastructure/tests/sidecar_upgrade_crash_matrix.rs](../../src-tauri/crates/infrastructure/tests/sidecar_upgrade_crash_matrix.rs)
- [src-tauri/crates/infrastructure/tests/t11_long_task_fixture.rs](../../src-tauri/crates/infrastructure/tests/t11_long_task_fixture.rs)
- [src-tauri/crates/infrastructure/tests/t11_sqlcipher_streaming_fixture.rs](../../src-tauri/crates/infrastructure/tests/t11_sqlcipher_streaming_fixture.rs)
- [src-tauri/crates/infrastructure/tests/trae_cn_checkin_probe.rs](../../src-tauri/crates/infrastructure/tests/trae_cn_checkin_probe.rs)
- [src-tauri/crates/infrastructure/tests/trae_cn_health_check_probe.rs](../../src-tauri/crates/infrastructure/tests/trae_cn_health_check_probe.rs)
- [src-tauri/crates/infrastructure/tests/trae_cn_library_read_probe.rs](../../src-tauri/crates/infrastructure/tests/trae_cn_library_read_probe.rs)
- [src-tauri/tests/compile_fail.rs](../../src-tauri/tests/compile_fail.rs)
- [src-tauri/tests/dependency_direction.rs](../../src-tauri/tests/dependency_direction.rs)
- [src-tauri/tests/fixture_paths_integration.rs](../../src-tauri/tests/fixture_paths_integration.rs)
- [src-tauri/tests/logging_integration.rs](../../src-tauri/tests/logging_integration.rs)
- [src-tauri/tests/ui/log_event_no_deserialize.rs](../../src-tauri/tests/ui/log_event_no_deserialize.rs)
- [src-tauri/tests/ui/log_event_no_deserialize.stderr](../../src-tauri/tests/ui/log_event_no_deserialize.stderr)
- [src-tauri/tests/ui/log_event_no_direct_construction.rs](../../src-tauri/tests/ui/log_event_no_direct_construction.rs)
- [src-tauri/tests/ui/log_event_no_direct_construction.stderr](../../src-tauri/tests/ui/log_event_no_direct_construction.stderr)
- [src-tauri/tests/ui/operation_id_no_direct_construction.rs](../../src-tauri/tests/ui/operation_id_no_direct_construction.rs)
- [src-tauri/tests/ui/operation_id_no_direct_construction.stderr](../../src-tauri/tests/ui/operation_id_no_direct_construction.stderr)
- [src-tauri/tests/ui/operation_id_no_from_validated.rs](../../src-tauri/tests/ui/operation_id_no_from_validated.rs)
- [src-tauri/tests/ui/operation_id_no_from_validated.stderr](../../src-tauri/tests/ui/operation_id_no_from_validated.stderr)
- [tests/AccountAdapter.test.tsx](../../tests/AccountAdapter.test.tsx)
- [tests/accountBatchProgressStatus.test.tsx](../../tests/accountBatchProgressStatus.test.tsx)
- [tests/accountsDetailView.test.ts](../../tests/accountsDetailView.test.ts)
- [tests/accountsPage.test.tsx](../../tests/accountsPage.test.tsx)
- [tests/accountsPageBatchProgress.test.tsx](../../tests/accountsPageBatchProgress.test.tsx)
- [tests/accountsPagePoolActions.test.tsx](../../tests/accountsPagePoolActions.test.tsx)
- [tests/accountsPageWorkbuddySwitch.test.tsx](../../tests/accountsPageWorkbuddySwitch.test.tsx)
- [tests/accountsRegistry.test.ts](../../tests/accountsRegistry.test.ts)
- [tests/App.test.tsx](../../tests/App.test.tsx)
- [tests/CapabilitySection.test.tsx](../../tests/CapabilitySection.test.tsx)
- [tests/CheckinPage.test.tsx](../../tests/CheckinPage.test.tsx)
- [tests/checkinPageWorkbuddy.test.tsx](../../tests/checkinPageWorkbuddy.test.tsx)
- [tests/CheckinShared.test.tsx](../../tests/CheckinShared.test.tsx)
- [tests/commandNames.test.ts](../../tests/commandNames.test.ts)
- [tests/EmptyState.test.tsx](../../tests/EmptyState.test.tsx)
- [tests/environmentPage.test.tsx](../../tests/environmentPage.test.tsx)
- [tests/fakeInvoke.test.ts](../../tests/fakeInvoke.test.ts)
- [tests/helpers/fakeInvoke.ts](../../tests/helpers/fakeInvoke.ts)
- [tests/libraryPage.test.tsx](../../tests/libraryPage.test.tsx)
- [tests/libraryRelayLegs.test.ts](../../tests/libraryRelayLegs.test.ts)
- [tests/librarySessionTree.test.ts](../../tests/librarySessionTree.test.ts)
- [tests/MasterSwitchDialog.test.tsx](../../tests/MasterSwitchDialog.test.tsx)
- [tests/notify.test.ts](../../tests/notify.test.ts)
- [tests/OverviewPage.test.tsx](../../tests/OverviewPage.test.tsx)
- [tests/RemoteDevicesPanel.test.tsx](../../tests/RemoteDevicesPanel.test.tsx)
- [tests/safeUiError.test.ts](../../tests/safeUiError.test.ts)
- [tests/settingsPage.test.tsx](../../tests/settingsPage.test.tsx)
- [tests/SettingsPanel.test.tsx](../../tests/SettingsPanel.test.tsx)
- [tests/setup.ts](../../tests/setup.ts)
- [tests/StatusBadges.test.tsx](../../tests/StatusBadges.test.tsx)
- [tests/uiDesignTokens.test.ts](../../tests/uiDesignTokens.test.ts)
- [tests/useArchiveDraft.test.tsx](../../tests/useArchiveDraft.test.tsx)
- [tests/useTauriEvents.test.tsx](../../tests/useTauriEvents.test.tsx)
- [tests/WorkBuddyPages.test.tsx](../../tests/WorkBuddyPages.test.tsx)
<!-- gen:aux:end -->
