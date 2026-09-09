// ManagedAccountSwitch 前端 DTO：字段与 Rust domain/account_switch.rs 的 serde 结果一致。
// 只包含非敏感显示信息、不可逆指纹和状态；不接收 token、cookies 或认证正文。

/** 交接意图的同步范围（domain SyncScope 的 serde wire 格式，外部标签）。 */
export type SyncScopeDto =
  | "AllHistory"
  | {
      Custom: {
        account_ids: readonly string[];
        project_ids: readonly string[];
        session_ids: ReadonlyArray<{
          product_history_namespace: string;
          original_session_id: string;
        }>;
      };
    };

export type AccountVerificationState =
  | "unknown"
  | "verified"
  | "single_source"
  | "conflict"
  | "expired"
  | "fingerprint_changed"
  | "manual_recovery_required";

export type AccountSwitchState =
  | "idle"
  | "preflight"
  | "waiting_for_trae_closed"
  | "applying"
  | "verifying"
  | "completed"
  | "restored"
  | "manual_recovery_required";

export type CredentialState = "missing" | "saved" | "stale" | "invalid";

export type AccountRegistryAuthorizationState =
  | "current_verified"
  | "credential_saved"
  | "profile_saved"
  | "credential_stale"
  | "credential_invalid"
  | "reverification_required"
  | "conflict"
  | "unbound";

// 仅用于账号中心的展示分组，不是后端状态机的新状态。
export type ManagedAccountSwitchPhase =
  | "idle"
  | "preflight_ready"
  | "waiting_for_trae_closed"
  | "switching"
  | "completed"
  | "failed"
  | "manual_recovery_required"
  | "recovered";

export type AccountTimestampDto = string;

export type HandoffIntentState =
  | "prepared"
  | "switching"
  | "target_verified"
  | "preview_ready"
  | "expired"
  | "manual_recovery_required";

export interface AccountProfileDto {
  readonly profile_id: string;
  readonly display_name: string;
  readonly region: string | null;
  readonly data_location_id: string;
  readonly last_verified_at: AccountTimestampDto | null;
  readonly verification_state: AccountVerificationState;
}

export interface CurrentAccountEvidenceDto {
  readonly profile_id: string | null;
  readonly display_name: string | null;
  readonly region: string | null;
  readonly data_location_id: string | null;
  readonly verification_state: AccountVerificationState;
  readonly observed_at: AccountTimestampDto | null;
  readonly reason: string | null;
}

export interface AccountSwitchPreflightDto {
  readonly source_verified: boolean;
  readonly target_known: boolean;
  readonly target_verified: boolean;
  readonly target_location_known: boolean;
  readonly pending_sync_plan_cleared: boolean;
  readonly trae_closed: boolean;
  readonly ready: boolean;
  readonly reason: string | null;
}

export interface AccountSwitchPlanDto {
  readonly plan_id: string;
  readonly source_profile_id: string | null;
  readonly target_profile_id: string;
  readonly source_data_location_id: string | null;
  readonly target_data_location_id: string;
  readonly preflight: AccountSwitchPreflightDto;
  readonly state: AccountSwitchState;
  readonly created_at: AccountTimestampDto;
  readonly failure_reason: string | null;
}

export interface ManagedAccountVerificationDto {
  readonly verification_state: AccountVerificationState;
  readonly checked_at: string | null;
  readonly reason: string | null;
}

export interface ManagedAccountsViewDto {
  readonly saved_accounts: readonly AccountProfileDto[];
  readonly current_account: CurrentAccountEvidenceDto;
  readonly recent_verification: ManagedAccountVerificationDto;
  readonly switch_state: AccountSwitchPlanDto | null;
  /** 旧开发包可能没有该字段；缺失按没有承接意图处理。 */
  readonly handoff_intent?: HandoffIntentDto | null;
  readonly history_is_separate: boolean;
}

export interface HandoffIntentDto {
  readonly intent_id: string;
  readonly source_profile_id: string | null;
  readonly target_profile_id: string;
  readonly data_location_id: string;
  readonly scope: SyncScopeDto;
  readonly state: HandoffIntentState;
  readonly created_at: AccountTimestampDto;
  readonly updated_at: AccountTimestampDto;
  readonly failure_reason: string | null;
}

export interface ManagedCredentialStatusDto {
  readonly profile_id: string;
  readonly state: CredentialState;
  readonly data_location_id: string;
  readonly format_version: number | null;
}

/** 历史发现账号与本机账号档案的只读合并结果。 */
export interface AccountRegistryEntryDto {
  readonly history_account_id: string;
  readonly display_label: string;
  readonly project_count: number;
  readonly session_count: number;
  readonly profile_id: string | null;
  readonly profile_display_name: string | null;
  readonly profile_verification_state: AccountVerificationState | null;
  readonly credential_state: CredentialState | null;
  readonly authorization_state: AccountRegistryAuthorizationState;
}

export interface AccountRegistryViewDto {
  readonly accounts: readonly AccountRegistryEntryDto[];
}

export interface KeyStatusDto {
  readonly source_key_configured: boolean;
  readonly source_key_version: string;
  /** 候选 key 已验证但等待下次启动激活。 */
  readonly source_key_pending_version?: string | null;
  readonly source_key_activation_pending?: boolean;
  readonly catalog_key_configured: boolean;
  readonly catalog_key_generation: number | null;
  readonly probe_state: "not_probed" | "verified" | "rejected" | "verified_pending";
}

export interface CheckinCapabilityDto {
  readonly enabled: boolean;
  readonly transport: "fixture" | "disabled" | string;
  readonly real_http_enabled: boolean;
  readonly message: string;
}

export type CheckinOutcomeDto =
  | "claimed"
  | "already_checked_in"
  | "not_eligible"
  | "auth_mismatch"
  | "credential_refresh_failed"
  | "profile_busy"
  | "network_error"
  | "runtime_error"
  | "verification_failed";

export interface CheckinResultDto {
  readonly profile_id: string;
  readonly outcome: CheckinOutcomeDto;
  readonly state: string;
  readonly claim_attempted: boolean;
  readonly before: { readonly enabled: boolean; readonly checked_in: boolean; readonly credits: number | null; readonly business_code: number | null } | null;
  readonly after: { readonly enabled: boolean; readonly checked_in: boolean; readonly credits: number | null; readonly business_code: number | null } | null;
  readonly detail_code: string | null;
  readonly started_at: string;
  readonly finished_at: string;
}

export interface CheckinBatchSummaryDto {
  readonly total: number;
  readonly completed: number;
  readonly failed: number;
  readonly cancelled: number;
  readonly results: readonly CheckinResultDto[];
}

/** OAuth 登录成功的账号档案（非敏感白名单，不含设备/令牌材料）。 */
export interface CheckinAccountDto {
  readonly profile_id: string;
  readonly account_id: string;
  readonly screen_name: string;
  readonly avatar_url: string;
  /** 是否归档（U-6 W4 资产库三态筛选：全部/活跃/已归档的数据源）。 */
  readonly archived: boolean;
  readonly last_verified_at: string | null;
}

/** get_account_storage_footprint 逐账号条目（U-6 W4：归档占用展示与彻底删除确认）。 */
export interface AccountStorageFootprintDto {
  readonly profile_id: string;
  /** 实例数据目录（`{storage_root}/trae-instances/{profile_id}`）递归大小（字节）；目录缺失记 0。 */
  readonly instance_dir_bytes: number;
  /** 原生账号目录（`{APPDATA}/TRAE SOLO CN_{account_id}`）递归大小（字节）；缺失记 0。 */
  readonly native_dir_bytes: number;
}

/**
 * 账号总览条目（get_checkin_overview 只读聚合，非敏感白名单）。
 * 积分为上次签到后的本机缓存值（P1-3：离线显示缓存并标注时间，
 * 不为展示消耗 status API 风控预算）。
 */
export interface CheckinOverviewEntryDto {
  readonly profile_id: string;
  readonly screen_name: string;
  /** 服务端用户 ID（非敏感数字 ID）；详情页技术细节区展示。 */
  readonly account_id: string;
  readonly created_at: string | null;
  readonly last_verified_at: string | null;
  readonly credits: number | null;
  readonly credits_cached_at: string | null;
  /** 真实模型额度剩余（ide_user_ent_usage 汇总，非签到活动积分）。 */
  readonly usage_remaining_credits: number | null;
  /** 额度缓存时间（RFC3339）。 */
  readonly usage_cached_at: string | null;
  readonly checked_in: boolean | null;
  readonly access_token_expires_at_unix_seconds: number | null;
  readonly refresh_token_expires_at_unix_seconds: number | null;
  /** 设备 ID 尾 4 位；与签到结果“兜底设备 …XXXX”同一展示口径。 */
  readonly device_tail: string | null;
  /** 完整设备 ID：仅详情页“技术细节”折叠区展示（本机排障用）。 */
  readonly device_id: string | null;
  /** 本地备注名（用户自定义别名）；null = 使用服务端 screen_name。展示口径：display_name ?? screen_name。 */
  readonly display_name: string | null;
  /** 脱敏手机号（登录/刷新额度时自动采集）；空串 = 未采集。 */
  readonly masked_mobile: string;
  /** 完整手机号（G11 手工补录，凭据包 DPAPI 加密存储）；null = 未补录（展示回退脱敏号）。 */
  readonly mobile_full: string | null;
  /** 该账号是否参与自动签到（详情页复选框数据源）。 */
  readonly auto_checkin_enabled: boolean;
  /** 最近一次额度刷新失败原因码；成功后清除（卡片持续显示“刷新失败”标记）。 */
  readonly refresh_error_code: string | null;
  /** 凭据包属于已退役旧通道（client_id 非 SOLO）：登录态直接按“登录失效”处理，需重新登录。 */
  readonly credential_legacy: boolean;
  /**
   * 今日最近一次签到尝试的结果码（G10 签到槽状态机输入）：
   * "business:9074" 形态 = 业务性失败（红·签到失败）；"transport:network_error" 形态 =
   * 传输性失败（琥珀·待重试）；"not_eligible" = 服务端业务拒绝（灰·不可领取）。
   * null = 今日尚无尝试记录（旧数据缺失时按 null 防御）。
   */
  readonly last_attempt_outcome: string | null;
  /** 最近一次签到尝试的本地日期（YYYY-MM-DD，与 outcome 同一日界过滤输出）；用于判定“今日尝试”，跨日不残留。null = 今日无尝试。 */
  readonly last_attempt_date: string | null;
}

/** get_auto_checkin_settings 返回：设置 + 今日台账。 */
export interface AutoCheckinStatusDto {
  readonly enabled: boolean;
  readonly daily_time_hhmm: string;
  readonly ledger: AutoCheckinLedgerDto | null;
}

/** 今日自动批次台账；running=true 表示错峰执行中或中断未完成。 */
export interface AutoCheckinLedgerDto {
  readonly date: string;
  readonly running: boolean;
  readonly total: number;
  readonly completed: number;
  readonly failed: number;
  readonly skipped: number;
}

/** refresh_checkin_credits 逐账号回执。 */
export interface CreditsRefreshEntryDto {
  readonly profile_id: string;
  readonly screen_name: string;
  readonly credits: number | null;
  readonly checked_in: boolean | null;
  /** 真实模型额度剩余；查询失败时为 null（保留旧缓存值）。 */
  readonly usage_remaining_credits: number | null;
  /** 失败原因码（network_error / business_XXXX / credential_*）；成功为 null。 */
  readonly error_code: string | null;
}

/** refresh_checkin_credentials 手动凭据刷新逐账号回执。 */
export interface CredentialRefreshEntryDto {
  readonly profile_id: string;
  readonly screen_name: string;
  /** 是否已完成同设备换发并安全写回。 */
  readonly refreshed: boolean;
  /** 失败原因码；成功为 null。 */
  readonly error_code: string | null;
}

/** 开始 OAuth 登录的返回：登录页地址（后端已同时打开系统浏览器）。 */
export interface CheckinLoginBeginDto {
  readonly login_url: string;
}

/** 登录完成回执：入库后的账号概要。 */
export interface CheckinLoginReceiptDto {
  readonly profile_id: string;
  readonly account_id: string;
  readonly screen_name: string;
  readonly avatar_url: string;
}

/**
 * 登录存档健康度（后端读登录存档 storage.json 的 usertag 键 + 最近启动日志证据判定）：
 * stale=登录键仍在但最近启动日志含服务端拒绝证据（会话已过期，U-7 保活在下次签到续期时写回自动恢复）。
 */
export type TraeInstanceLoginState =
  | "uninitialized"
  | "logged_in"
  | "logged_out"
  | "stale";

/** get_trae_instance_states 逐账号条目（P7-5 凭据包实调判定：
 *  login_state 为凭据包 token 实调 GetUserInfo 的结果，断网降级存档本地证据；
 *  archive_available 为登录存档登录键是否存在，即 E1 存档移植降级路径可用性，
 *  仅作徽章悬浮提示的次要信息，不占主视野）。 */
export interface TraeInstanceStateDto {
  readonly profile_id: string;
  readonly login_state: TraeInstanceLoginState;
  readonly archive_available: boolean;
}

/** P3-1 单条会话摘要（账号实例库只读聚合：标题、时间、消息数）。 */
export interface SessionSummaryDto {
  readonly session_id: string;
  readonly title: string;
  readonly message_count: number;
  /** unix 秒（源库毫秒已归一化）；缺失为 null。 */
  readonly updated_at_unix_seconds: number | null;
  readonly deleted: boolean;
}

/** U-6 W3 单文件 stat 指纹（mtime 秒/纳秒 + size）；null = 文件不存在。 */
export interface FileStatFingerprintDto {
  readonly mtime_secs: number;
  readonly mtime_nanos: number;
  readonly size: number;
}

/** U-6 W3 账号实例库三件套指纹（db + wal + shm）；null = 对应文件不存在。 */
export interface InstanceFingerprintDto {
  readonly db: FileStatFingerprintDto | null;
  readonly wal: FileStatFingerprintDto | null;
  readonly shm: FileStatFingerprintDto | null;
}

/** P3-1 单账号会话索引状态：ready=读取成功；no_instance_data=该账号从未启动过实例；read_failed=打开或读取失败。 */
export type AccountSessionIndexStatus = "ready" | "no_instance_data" | "read_failed";

/** get_account_session_index 逐账号条目。 */
export interface AccountSessionIndexEntryDto {
  readonly profile_id: string;
  readonly display_name: string;
  readonly status: AccountSessionIndexStatus;
  readonly sessions: readonly SessionSummaryDto[];
  /** ready 状态下的会话总数（其余状态为 0）。 */
  readonly total: number;
  /** 读取完成后的三件套指纹（U-6 W3）——轮询变化检测的 previous 基线。 */
  readonly fingerprint: InstanceFingerprintDto;
}

/** get_account_session_changes 返回（U-6 W3 变化检测轮询）。 */
export interface AccountSessionChangesDto {
  /** 各账号最新三件套指纹（调用方只推进读取成功账号的基线）。 */
  readonly fingerprints: Readonly<Record<string, InstanceFingerprintDto>>;
  /** 与 previous 比对后发生变化的 profile_id 列表。 */
  readonly changed: readonly string[];
}

/** P3-2 消息内容（平铺形态）：kind 区分文本正文 / 任务执行轨迹摘要。 */
export interface SessionMessageContentDto {
  /** "text"=纯文本正文；"task_trace"=任务执行轨迹。 */
  readonly kind: "text" | "task_trace";
  /** text 形态正文（内容块拼接）；task_trace 形态为空串。 */
  readonly text: string;
  /** task_trace 形态步骤数；text 形态为 0。 */
  readonly step_count: number;
  /** task_trace 形态各步 thought 摘要；text 形态为空数组。 */
  readonly thoughts: readonly string[];
}

/** P3-2 单条会话消息（元数据 + 解析后内容）。 */
export interface SessionMessageDto {
  readonly message_id: string;
  /** "user" / "assistant"（透传源库值）。 */
  readonly role: string;
  /** "general" / "task" / "chat"（决定内容解析路径）。 */
  readonly message_type: string;
  /** unix 秒；缺失为 null（旧数据行可能无时间）。 */
  readonly created_at_unix_seconds: number | null;
  readonly content: SessionMessageContentDto;
}

/** P3-2 单会话消息读取状态（语义与 P3-1 索引一致）。 */
export type AccountSessionMessagesStatus = "ready" | "no_instance_data" | "read_failed";

/** get_account_session_messages 返回。 */
export interface AccountSessionMessagesDto {
  readonly profile_id: string;
  readonly session_id: string;
  readonly status: AccountSessionMessagesStatus;
  readonly messages: readonly SessionMessageDto[];
}

// ===== P5-1 主库切号（switch_master_account，Q1.1 全自动五步事务）=====

/** 切号阶段（`master-switch-progress` 事件负载；Q10 修正后的顺序）。 */
export type MasterSwitchStage =
  | "closing" // 关闭主库实例
  | "backing_up" // 主库数据备份
  | "switching_login" // 写入目标账号登录凭据
  | "handing_over" // 交接对话记录
  | "syncing_plugins" // 云端插件预同步（切换前账号 → 目标账号）
  | "restarting" // 重启主库
  | "done"; // 就绪

/** `master-switch-progress` 事件负载。 */
export interface MasterSwitchProgressEvent {
  readonly profile_id: string;
  readonly stage: MasterSwitchStage;
  /** handing_over 阶段的细粒度进度（P7-3，可选，向后兼容）。 */
  readonly progress?: MasterSwitchHandoverProgress;
}

/** 交接内部进度（P7-3：mapping / executing / verifying 逐项上报）。 */
export interface MasterSwitchHandoverProgress {
  /** 交接内部阶段：mapping（整理）/ executing（写入）/ verifying（校验）。 */
  readonly phase: "mapping" | "executing" | "verifying";
  readonly current: number;
  readonly total: number;
  /** 用户可读标签（项目名或数据类别名）。 */
  readonly label: string;
}

/** `master-switch-rolled-back` 事件负载（P7-2：切号失败自动还原结果）。 */
export interface MasterSwitchRolledBackEvent {
  readonly profile_id: string;
  /** true = 主库登录态已还原到切换前，失败文案附「可安全重试」。 */
  readonly rolled_back: boolean;
}

/** 云端插件同步回执（fail-soft：失败只影响插件市场显示，不影响切号）。 */
export interface PluginCloudSyncDto {
  /** 切换前账号云端已装插件数。 */
  readonly source_count: number;
  /** 成功同步到目标账号云端的插件数。 */
  readonly installed: number;
  /** 成功从目标账号云端移除的插件数（清单外多余插件）。 */
  readonly removed: number;
  /** 同步失败数（可在 TRAE 插件市场手动重装）。 */
  readonly failed: number;
  /** 跳过数（用户自装插件，无法跨账号复制）。 */
  readonly skipped: number;
  /** 整体未执行（账号凭据或网络不可用）。 */
  readonly aborted: boolean;
}

/** preview_master_switch_plugins 返回（ADR-0026：remove_names 非空才弹
 * 一次移除确认；纯新增差异静默应用）。 */
export interface MasterSwitchPluginPreviewDto {
  /** 当前账号云端市场插件数。 */
  readonly source_count: number;
  /** 目标账号云端市场插件数。 */
  readonly target_count: number;
  /** 待安装到目标账号的插件名（纯新增，静默应用不确认）。 */
  readonly install_names: readonly string[];
  /** 待从目标账号移除的插件名（非空才弹确认，列明移除哪些）。 */
  readonly remove_names: readonly string[];
  /** 预检未完成（凭据/网络不可用）：前端静默直过切号，不弹确认。 */
  readonly aborted: boolean;
}

/** switch_master_account 返回（五步完成回执）。 */
export interface MasterAccountSwitchDto {
  readonly profile_id: string;
  /** 切换后主库登录账号的 TRAE user_id。 */
  readonly to_user_id: string;
  /** 切换前主库实际登录账号；首次切号（主库刚登录）时为 null。 */
  readonly from_user_id: string | null;
  /** 归属随行的项目数（全量随行语义）。 */
  readonly transferred_projects: number;
  /** 自动清理的空镜像行数。 */
  readonly removed_mirror_rows: number;
  /** 本次交接（换腿）的会话数。 */
  readonly switched_sessions: number;
  /** 主库数据备份文件路径（人工恢复定位用）。 */
  readonly backup_path: string;
  /** 接力台账写入结果：false = 交接完成但台账追加失败（仅影响轨迹展示）。 */
  readonly relay_ledger_written: boolean;
  /** 云端插件预同步回执。 */
  readonly plugin_sync: PluginCloudSyncDto;
  /** 主库重启结果：launched=已重启。 */
  readonly relaunch_outcome: string;
}

/** switch_master_account 错误码（前端按码给出独立文案，含弹窗分支）。 */
export type MasterSwitchErrorCode =
  | "master_switch_busy" // 生成中（DB 活跃）：弹窗「等待完成 / 强制切换」
  | "master_switch_db_missing" // 主库尚未启动登录过（先启动主库并登录一次）
  | "master_login_missing" // 主库无登录凭据（先在主库内登录一次）
  | "switch_donor_login_missing" // 目标账号登录凭据不可用（在线验证与本地存档均未通过）
  | "switch_same_account" // 目标账号已是主库当前账号
  | "master_switch_conflict" // 目标账号名下存在同名项目非空记录，需人工决策
  | "master_switch_close_failed"
  | "master_switch_backup_failed"
  | "switch_verify_failed"
  | "master_switch_integrity_failed"
  | "master_switch_db_open_failed"
  | "master_switch_rollback_failed" // 切换失败且自动还原未完成（指向备份链）
  | "switch_auth_failed"
  | "master_switch_join_failed"
  | "trae_real_mode_required"
  | "source_key_unavailable"
  | "checkin_registry_invalid"
  | "trae_profile_invalid"
  | "environment_registry_invalid";
