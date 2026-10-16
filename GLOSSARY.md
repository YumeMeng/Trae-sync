# GLOSSARY — Trae Sync 领域词汇表

## 账号与身份

**账号（Account）**：一个 TRAE 融合版登录身份，由 `account_id`（平台用户 ID）唯一标识。工具通过 AccountRegistry（Work 档案登记表）管理账号清单。

**档案（Profile / Work 档案）**：账号在工具侧的登记记录（AccountRegistry 中一行），含 `profile_id`（工具侧标识，如 `checkin-xxxx`）、`screen_name`、`display_name`。一个账号恰好一个档案。

**账号池（Account Pool）**：全部已登记档案的集合。当前 16 个（迁移铸造产生）；Phase 16 起通过「捕获」开放增长（16+N）。

**捕获（Capture）**：从 Trae 客户端的登录态（storage.json 登录 blob）提取凭据并铸造入池的动作。区别于「OAuth 登录」（工具内浏览器流程）。Phase 16 新增能力。

## 凭据材料

**.fused 容器（FusedLoginStore）**：账号的融合版凭据容器（`{fused-login 根}/{profile_id}.fused`，TSFUSED1 + DPAPI）。切号与历史身份判定的数据源。铸造 = 从客户端 blob 提取 → GetPCAuthCode → ExchangeToken → 容器化。

**.checkin 凭据包（CheckinCredentialStore）**：签到链路的凭据包（`{checkin 根}/` 下，同源 DPAPI）。自动签到（SOLO 通道）的凭据来源。入池即全功能要求 .fused 与 .checkin 同时铸造。

**登录 blob（Login Blob）**：Trae 客户端 storage.json 中 `iCubeAuthInfo://icube-dc:` 键下的加密登录态。捕获的原始材料；也是切号写入的目标。

**设备身份（Device Identity）**：主环境铸造的 device_id + publicKeyPEM + machineId。GetPCAuthCode/ExchangeToken 必须与其对齐（P15-5 闭环验证的核心）。

## 库与操作

**主库（Master Library）**：融合版客户端的对话主库（`Roaming\Trae CN\ModularData\ai-agent\database.db`，SQLCipher 加密）。账号切换/归档/归并/删除/收编的作用对象。

**库实例引用（library_id）**：库上下文标识（`master` 主库 / `trae_cn` 只读遗产库 / 副环境 env_id），ADR-0025。

**写操作闸门（Operation Gate）**：product_operations 注册表对破坏性操作的就绪判定（Pending 拒绝 / Ready 放行）。进程闸门（客户端运行中拒绝）是其运行时层，独立于就绪状态。

**防逃逸判定（Link Guard）**：对库路径链的 reparse point 检查（符号链接 + junction 统一识别，`link_guard::is_reparse_link`）。阻止目录逃逸攻击。

## 备份与恢复

**三件套（Trio）**：主库的 database.db + -wal + -shm 三文件。一切写操作前必须完整备份（ADR-0018 铁律）。

**备份链（Backup Chain）**：三件套备份的时间序列（`switch-bak-*` 命名）。Phase 16 起默认落软件本体旁 `backups/` 目录并 zstd 压缩。

**保留策略（Retention / prune）**：备份链的份数治理（prune_backups_if_enabled）。

## 运行模式

**真实模式（RealReadPreview）**：工具操作真实客户端数据根的运行模式（与 fixture 测试模式相对）。所有写操作仅在此模式放行。

**单次确认（Single Confirmation，ADR-0018）**：破坏性真实操作执行前必须获得用户的显式一次性确认；自动化护航（备份/守卫/回执）围绕该确认构建。
