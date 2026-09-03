/**
 * P5-4 主库轻量统计与备份链（get_master_library_stats /
 * get_master_backup_chain / create_master_backup 返回）。
 * 字段与 src-tauri/src/lib.rs 的 MasterLibraryStatsDto /
 * MasterBackupChainDto（serde 命名）一致。
 */

/** 主库聚合统计（总览页统计卡 + 环境卡会话胶囊数据源）。 */
export interface MasterLibraryStatsDto {
  /** ready=读取成功；no_master_data=主库从未启动过；no_current_account=尚未登记账号；read_failed=读取失败。 */
  readonly status: "ready" | "no_master_data" | "no_current_account" | "read_failed";
  /** 主库当前登录账号的 TRAE user_id；未登记为 null。 */
  readonly current_user_id: string | null;
  readonly project_count: number;
  readonly session_count: number;
  /** 当前账号会话的消息总数（P5-8a-2 详情页头部，与历史页同口径）。 */
  readonly message_count: number;
  /** 主库内出现过的全部账号数（含非当前账号历史归属）。 */
  readonly participating_account_count: number;
  /** 当前账号会话最近活跃时间（秒）；无会话为 null。 */
  readonly last_active_unix_seconds: number | null;
  /** 主库三件套合计字节（详情页头部；主库从未启动为 0）。 */
  readonly size_bytes: number;
}

/** 一份主库数据备份（切换账号与手动备份都会生成）。 */
export interface MasterBackupEntryDto {
  /** 备份时间戳（秒级 UNIX 时间）。 */
  readonly stamp_unix_seconds: number;
  /** 备份合计字节（含存在的附属件）。 */
  readonly total_bytes: number;
  /** 是否带 wal 附属件。 */
  readonly has_wal: boolean;
}

/** 主库备份链（设置页备份分区数据源）。 */
export interface MasterBackupChainDto {
  readonly backups: readonly MasterBackupEntryDto[];
  /** 备份所在目录（展示与人工恢复定位）。 */
  readonly backup_dir: string;
  /** 建议保留份数；自动清理行为由备份保留设置（P5-9）决定。 */
  readonly keep_policy: number;
}

/**
 * P5-9 备份保留设置（get_backup_retention 返回）。
 * 字段与 src-tauri/src/lib.rs 的 BackupRetentionDto（serde 命名）一致。
 */
export interface BackupRetentionDto {
  /** 自动清理开关；false = 永不自动删除。 */
  readonly enabled: boolean;
  /** 保留最近 N 份（含刚生成的最新备份；范围 1-50）。 */
  readonly keep: number;
}

/**
 * P5-7 主库两层校验（get_master_verification 返回）。
 * 字段与 src-tauri/src/lib.rs 的 MasterVerificationDto 系（serde 命名）一致。
 */

/** 台账核对异常行（kind 分组展示；detail 为界面主信息）。 */
export interface LedgerIssueDto {
  /** missing=会话不存在 / owner_mismatch=归属不一致 / message_loss=消息减少 / stale_leg=旧会话残留。 */
  readonly kind: "missing" | "owner_mismatch" | "message_loss" | "stale_leg";
  /** 技术标识；收悬浮提示，不占主视野。 */
  readonly session_id: string;
  readonly detail: string;
}

/** 第一层结果：接力台账核对。 */
export interface LedgerVerificationDto {
  /** ready / no_ledger（从未切号，正常空态）/ read_failed / no_master_data。 */
  readonly status: "ready" | "no_ledger" | "read_failed" | "no_master_data";
  /** 核对过的当前腿数。 */
  readonly checked_count: number;
  /** 已被后续接力接走的中间腿数（接力链衔接证据）。 */
  readonly relayed_away_count: number;
  readonly issues: readonly LedgerIssueDto[];
}

/** 丢失候选会话行（备份里还在、现在没有）。 */
export interface MissingSessionDto {
  /** 备份库中的会话标题；无标题时前端降级为「未命名会话」。 */
  readonly title: string | null;
  /** 备份时该会话的消息数。 */
  readonly message_count: number;
}

/** 第二层结果：备份对比。 */
export interface BackupComparisonDto {
  /** ready / no_backups / backup_missing / read_failed / no_master_data。 */
  readonly status: "ready" | "no_backups" | "backup_missing" | "read_failed" | "no_master_data";
  /** 实际使用的基准备份时间戳；无备份为 null。 */
  readonly backup_stamp: number | null;
  /** 备份链全部可选基准点（时间戳倒序，切换对比基准用）。 */
  readonly backup_stamps: readonly number[];
  /** 两边都有的会话数。 */
  readonly common_count: number;
  /** 备份有、现在没有、台账解释为换腿（正常接力）的会话数。 */
  readonly relayed_away_count: number;
  /** 现在有、备份没有（新增，正常）。 */
  readonly added_count: number;
  /** 丢失候选（只报告，附人工恢复指引）。 */
  readonly missing: readonly MissingSessionDto[];
}

/** 两层校验合并结果（「数据校验」tab 数据源）。 */
export interface MasterVerificationDto {
  readonly ledger: LedgerVerificationDto;
  readonly backup: BackupComparisonDto;
}

/**
 * P5-5 主库体检（get_master_checkup 返回）。
 * 字段与 src-tauri/src/lib.rs 的 MasterCheckupDto / MasterCheckupAccountDto
 * （serde 命名）一致。
 */

/** 体检报告中的账号分布行。 */
export interface MasterCheckupAccountDto {
  /** TRAE user_id（技术标识；UI 主信息只用账号名，user_id 收悬浮提示）。 */
  readonly user_id: string;
  /** 账号展示名；未在账号页登记为 null。 */
  readonly account_name: string | null;
  /** 是否已在账号页登记。 */
  readonly registered: boolean;
  /** 是否当前账号（收编目标）。 */
  readonly current: boolean;
  /** 项目行数（全量含软删，与归属改写口径一致）。 */
  readonly project_count: number;
  /** 会话数（全量含软删）。 */
  readonly session_count: number;
}

/** 主库体检报告（环境页体检区块数据源，只读）。 */
export interface MasterCheckupDto {
  /** ready / no_current_account / no_master_data / read_failed。 */
  readonly status: "ready" | "no_current_account" | "no_master_data" | "read_failed";
  /** 当前账号账号名（回执与引导文案用）；未登记为 null。 */
  readonly current_account_name: string | null;
  /** 全库账号分布（按会话数降序）。 */
  readonly accounts: readonly MasterCheckupAccountDto[];
  /** 无归属项目行数（只报告，收编不动）。 */
  readonly orphan_project_count: number;
  /** 挂在无归属项目行上的会话数。 */
  readonly orphan_session_count: number;
}

/** P5-5 一键收编回执（incorporate_master_records 返回）。 */
export interface MasterIncorporateResultDto {
  /** 归入的账号数。 */
  readonly merged_accounts: number;
  /** 归属随行的 project 行数。 */
  readonly transferred_projects: number;
  /** 自动清理的空镜像行数。 */
  readonly removed_mirror_rows: number;
  /** 换腿（交接）的会话数。 */
  readonly switched_sessions: number;
  /** 收编前自动创建的备份路径（人工恢复定位）。 */
  readonly backup_path: string;
  /** 接力台账写入结果（false 只影响轨迹展示）。 */
  readonly relay_ledger_written: boolean;
  /** 主库重启结果。 */
  readonly relaunch_outcome: string;
}

/** 收编进度事件负载（`master-incorporate-progress`）。 */
export interface MasterIncorporateProgressEvent {
  /** closing / backing_up / incorporating / restarting / done。 */
  readonly stage:
    | "closing"
    | "backing_up"
    | "incorporating"
    | "restarting"
    | "done";
}
