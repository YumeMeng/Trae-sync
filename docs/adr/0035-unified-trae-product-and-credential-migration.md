# ADR-0035：融合版 TRAE 产品合并与账号迁移

## 状态

Accepted，2026-10-13。**取代** ADR-0030 的产品目标定位（旧独立 Trae CN 已被官方融合版吸收，`OAuthClient::TraeCn` 形态与 CN 协议层被融合版适配继续使用）；**局部取代** ADR-0029/0032/0034 中「Work CN + Trae CN 双产品」的产品边界（合并为融合版 TRAE 单一目标；操作级验收、进程/数据/凭据隔离、五步事务与能力三态纪律继续有效）；签到、设备与凭据维护纪律**继续以 ADR-0019 v6 / ADR-0027 / ADR-0031 为准**。

## 背景

TRAE 官方将 TraeWork CN 升级为融合版 TRAE（3.4.2），TraeWork 客户端 2026-10-23 停止服务；旧独立 Trae CN 已被融合版吸收。用户 16 个 Work 账号的手机号多数无法再次接收验证码，停服即失效。2026-10-13 实测确立的关键事实：

1. 融合版登录通道与旧独立 Trae CN 完全同形态（`OAuthClient::TraeCn`：ClientID `ono9krqynydwx5` / PlatformCode `IDE_PC`）；登录 blob 格式与 Work 同构（AES-128-CBC 自包含），工具 `construct_auth_identity` / `switch_auth_identity` 直接兼容。
2. 融合版客户端设备身份为机器级 ICDRS 三元组（rdid、`telemetry.machineId`、`iCubeAuthInfo://icube-dc:{rdid}` 键内 PEM 密钥对），本机全部实例共享；客户端以该身份对 storage.json 中的登录态执行 `exchangeTokenByRefreshToken` 续期。
3. 融合版主库与 Work 主库 schema 完全同构（61 表、列级零差异）；官方迁移已把 Work 库全部内容搬入融合版目录。
4. 用 Work SOLO 凭据的 access token 以 TraeCn 形态走 GetPCAuthCode → ExchangeToken 可为客户端设备身份铸造全新凭据代次（隔离环境全链路实测通过：登录加载、在线身份验证、冷启动留存）；该路径不触碰 refresh token，签到链路零影响。
5. 旧独立 Trae CN 产品池的 4 个账号与 16 个 Work 账号身份完全重叠（无独立资源）；LY 在 TRAE 通道设备配额可能耗尽（20401，用户已授权自动退出最老设备）。

## 决策

1. **单一产品目标**：融合版 TRAE 取代「Work CN + 旧独立 Trae CN」成为工具唯一产品适配目标。产品抽象层（适配器、能力清单、进程/数据/凭据隔离）保留；UI 产品切换器随单产品隐藏。
2. **双通道凭据模型**：每账号两套认证材料并行独立——登录代次（TRAE 通道，绑定融合版客户端设备身份，存 `fused-login` 加密容器）与签到代次（SOLO 通道，现有 .checkin 凭据包，虚拟设备身份）。两套代次独立续期、独立写回、互不覆盖；跨通道复制禁令（ADR-0029/0034 精神）继续有效。
3. **迁移路径（阶段一先行）**：全部 16 账号用铸造路径迁移——凭据包 access token → TraeCn 形态 GetPCAuthCode → ExchangeToken（DeviceInfo 完全对齐融合版真实续期请求、统一使用主环境设备身份）→ 构造的登录代次 DPAPI 加密存入工具存储，**不写任何客户端目录**；切号时才写入融合版主环境目录。迁移确认（用户逐账号核验）先于工具改造。
4. **切号模型**：与 Work 切号（E2 构造）同构——关客户端（进程守卫沿用 P14-2 成果）→ 目标账号登录 blob 写入 `Roaming\Trae CN`（construct_auth_identity 同链路）→ 重启客户端 → 官方界面确认。五步事务、回滚与档案指针语义沿用 P14-3 成果。
5. **主库目标切换**：主库环境绑定目录改为 `Roaming\Trae CN`（env_id=master 官方目录绑定语义不变）。库操作（归属随行/收编/归档/归并/体检）零适配复用；切换前完整备份。
6. **旧模块移除**：旧 CN 产品层（账号池命令、产品适配分支、导航页分支）与 `data/products/trae_cn/` 数据删除——数据删除前完整备份压缩包落盘 `%LOCALAPPDATA%\Trae Sync Backups\` 并经用户单次确认；协议层（`OAuthClient::TraeCn`、GetPCAuthCode/ExchangeToken、CN 签到客户端）保留复用。
7. **登录代次维护边界**：登录代次续期由融合版客户端自动完成，工具不抢跑；工具提供只读健康检测（GetUserInfo）与过期提示。签到续期后的 blob keepalive 按融合版目录扩展（仅写当前切号账号）需设计确认后实施。

## 后果

- 用户在 Work 停服前完成全部 16 账号迁移，零重新登录；签到与历史资产管理不中断。
- 工具从双产品收敛为单产品，旧 trae_cn 半成品尾巴清除（数据有备份）。
- 每次铸造消耗账号在 TRAE 通道 1 个设备位（限额 10/账号，不回收）；迁移报告须记录配额水位。
- 阶段一期间主环境只读；首次真实切号是「主环境设备身份铸造 → 客户端续期」闭环的最终协议验证点，须单次确认执行。
- Phase 14 的 Work 侧成果（进程守卫、跨进程租约、回滚安全）全部继承；CN 侧按旧 CN 设计的未完成项（P14-8b/8c 等）剥离不再执行，并入本期。

## 实施备注（2026-10-13 更新）

- 阶段一完成：16/16 Work 账号融合版登录凭据铸造成功（DPAPI 容器 `fused-login`，零失败，LY 未触发设备配额），双通道边界由测试锁定。
- 阶段二代码侧完成：切号编排融合版接线（P15-5 离线矩阵 28/28）、主库目标切换（P15-6，`master_data_dir` 单点改指融合版目录）、旧 trae_cn 模块移除（P15-7：旧池命令/CN 签到区块/凭据刷新清除，协议层与归并命令族保留；工具选择指针迁 `{storage_root}/product-state`；远程设备路由改源 Work 签到注册表）。
- 回归证据：cargo 串行分段（根 220/0/1 + 3 集成 + doc、infra 898/0/7、小 crate 142/0；trybuild 段因环境死锁遗留重跑）、typecheck、Vitest 402/402、Playwright E2E 43/43。
- 待办：首次真实切号（阶段一遗留的协议验证闭环）、真实启停验收、数据备份压缩包确认后删除旧池目录；产品模型收口（切换器/三态）与融合版新登录闭环为已知债项（tickets P15-9）。
