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


## 跨产品概念映射（ADR-0037——扩展新平台的锚定依据）

| 锚定概念 | TRAE 实现 | WB 实现 | 扩展判定问题清单 |
|----------|-----------|---------|------------------|
| 对话库 | 主库 database.db（SQLCipher）+ 归属随行 | workbuddy.db（SQLite WAL，sessions.user_id 隔离）+ projects jsonl 正文 | 会话存储在哪？本地还是云？账号隔离字段？正文与索引是否分离？ |
| 环境 | --user-data-dir 隔离目录多实例，对话库跟随目录 | userData 与数据根分离，多实例隔离性待实验 | 客户端支持多实例参数吗？数据目录跟随哪个参数？ |
| 额度 | 模型积分（checkin 概览） | total_credits + daily_credit + checkin_dates | 签到/积分接口有哪些字段？逐字段核对过吗？ |
| 添加账号 | 隔离浏览器 OAuth 编排 | 网页 OAuth 授权码流（CodeBuddy 参数）+ 捕获入池 | 登录是网页流还是仅客户端？协议参数在哪？ |
| 签到登录态 | begin/complete/cancel_login 会话编排 | 直连（池凭据）无登录会话 | 该平台签到是否需要独立登录会话？ |
| 主库/副库 | 原生库 vs 副环境实例 | 单库，多实例实验后再定 | 存在几个库？如何区分原生与创建？ |
| 收编/过户 | incorporate_master_records | transfer sessions.user_id（P18-B1） | 客户端游离记录如何纳入管理？归属字段是什么？ |
| 归并 | merge_master_projects（主库内项目合并） | cwd 项目归并（sessions 批量 UPDATE + 目录迁移） | 项目维度是什么？如何合并？ |
