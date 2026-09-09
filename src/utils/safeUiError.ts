// 前端只显示稳定、可执行的错误提示，不把 Tauri/Rust 底层错误正文交给用户界面。

const KNOWN_ERROR_MESSAGES: ReadonlyArray<readonly [RegExp, string]> = [
  [
    /checkin_http_disabled/i,
    "真实签到当前未启用；历史扫描和账号切换仍可正常使用。",
  ],
  [
    /login_real_mode_required/i,
    "当前运行模式不支持账号登录。",
  ],
  [
    /login_begin_failed|login_join_failed/i,
    "登录会话创建失败，请重新发起登录。",
  ],
  [
    /login_not_started/i,
    "没有进行中的登录，请先点击添加账号。",
  ],
  [
    /login_callback_invalid/i,
    "浏览器登录回调无效，请重新发起登录。",
  ],
  [
    /login_exchange_failed/i,
    "登录凭证换取失败（服务端拒绝）；详细原因已记录在诊断日志（数据目录 checkin/login-diagnostics.log），可重新发起登录再试一次。",
  ],
  [
    /login_exchange_device_limit/i,
    "该账号在服务端的设备数量已达上限，登录暂时被拒；本地删除账号不会释放服务端的设备配额，请隔天再试或通过 TRAE 官方渠道处理旧设备。",
  ],
  [
    /login_token_invalid/i,
    "服务端返回的登录凭证无效，请重新登录。",
  ],
  [
    /login_storage_failed/i,
    "登录信息保存失败，请重试。",
  ],
  [
    /login_cancelled/i,
    "登录已取消；若浏览器已关闭或未完成授权，可重新发起登录。",
  ],
  [
    /checkin_storage_unavailable/i,
    "本机登录存储不可用，请检查磁盘后重试。",
  ],
  [
    /checkin_already_running/i,
    "签到正在进行中，请等待当前批次完成后再操作。",
  ],
  [
    /device_too_new/i,
    "设备刚创建，服务端需要几分钟建立信任，请稍等后重试签到。",
  ],
  [
    /credential_refresh_failed/i,
    "登录凭据已过期或不可用，无法自动续期；请重新登录该账号以更新凭据。",
  ],
  [
    /credential_refresh_busy/i,
    "当前已有签到或凭据维护在进行，请稍后重试。",
  ],
  [
    /binding_mismatch|auth_mismatch/i,
    "账号绑定信息与本机登录凭据不一致，请重新登录该账号。",
  ],
  [
    /credential_missing|credential_unavailable|credential_invalid/i,
    "本机登录凭据不可用，请重新登录该账号。",
  ],
  [
    /business_9074/i,
    "设备身份未获服务端信任（9074）；若刚登录或刚重置设备请稍等几分钟再试，否则请在账号详情重置签到设备。",
  ],
  [
    /business_9095/i,
    "该设备今日名额已用（9095），请明日再试。",
  ],
  [
    /remint_failed/i,
    "重置设备失败（网络或服务端异常），请在账号详情重试后重新签到。",
  ],
  [
    /display_name_invalid/i,
    "备注名过长，请控制在 64 字符以内。",
  ],
  [
    /mobile_format_invalid/i,
    "手机号格式不正确：请输入 11 位大陆手机号（1 开头）。",
  ],
  [
    /mobile_masked_mismatch/i,
    "手机号与该账号的服务端记录不一致（首尾号段不匹配），请核对后重新输入。",
  ],
  [
    /mobile_save_failed/i,
    "手机号保存失败，请稍后重试。",
  ],
  [
    /checkin_registry_invalid/i,
    "账号注册表不可读取，请重启应用后重试；若持续出现请保留数据目录后反馈。",
  ],
  [
    /checkin_profile_empty|checkin_profile_invalid/i,
    "请选择至少一个可用账号档案后再签到。",
  ],
  [
    /checkin_cancelled/i,
    "签到批次已取消，尚未开始的账号不会继续执行。",
  ],
  [
    /account_profile_store_busy/i,
    "另一实例正在更新账号档案，请稍后重试。",
  ],
  [
    /account_fingerprint_salt_unavailable/i,
    "账号安全摘要暂不可用，请检查本地存储权限后重试。",
  ],
  [
    /account_profile_store_salt_mismatch/i,
    "账号档案与当前安装不匹配，已停止读取；请保留现有档案后重试。",
  ],
  [
    /account_profile_store_salt_binding_required/i,
    "账号档案需要在当前安装中重新绑定，现有记录未被覆盖。",
  ],
  [
    /account_profile_store_invalid|account_profile_store_path_invalid|account_fingerprint_salt_invalid/i,
    "账号档案校验失败，已停止读取；请保留现有档案后重试。",
  ],
  [
    /catalog_write_protocol_upgrade_required|不支持的写入协议/i,
    "当前目录库采用了此版本不支持的写入协议，未写入新的历史或修改目录库。",
  ],
  [
    /catalog.?unavailable|目录库不可读|目录库运行材料不可用/i,
    "目录库不可读，请重新扫描。",
  ],
  [
    /not_authorized|未授权扫描|后端未持有显式用户授权/i,
    "当前读取授权已失效，请重新授权。",
  ],
  [
    /authorization_mismatch|授权不匹配|授权范围不一致/i,
    "读取授权与当前数据位置不一致，请重新授权。",
  ],
  [
    /account_evidence_changed|账号证据.*变化|账号指纹漂移/i,
    "当前账号信息已变化，请重新检测后再读取。",
  ],
  [
    /data_location_changed|数据位置.*变化|位置.*发生变化/i,
    "TRAE 数据位置已变化，请重新授权后再读取。",
  ],
  [
    /data_location_unavailable|APPDATA (?:未设置|必须|不可用)|TRAE 默认(?:根目录|数据库)|TRAE 数据位置身份捕获失败/i,
    "TRAE 数据位置暂不可用，请重新授权后再读取。",
  ],
  [
    /process_running|TRAE.*运行中|TRAE 正在运行/i,
    "TRAE 正在运行，请关闭后重试。",
  ],
  [
    /storage_root_unavailable|存储根.*不可用|恢复区.*不可用/i,
    "本地存储暂不可用，请检查磁盘后重试。",
  ],
  [
    /account_evidence_unavailable|账号证据.*不可用|账号证据不足/i,
    "当前账号证据不可用，请重新检测账号。",
  ],
  [
    /schema_incompatible|schema.*不兼容/i,
    "数据结构不兼容，无法完成读取。",
  ],
  [
    /gate_not_qualified|能力.*未开放|写入能力未开放/i,
    "当前能力仍保持只读，相关操作暂不可用。",
  ],
  [
    /another_operation_running|operation_lease_busy|操作正在进行/i,
    "已有操作正在进行，请稍后重试。",
  ],
  [
    /manual_recovery_required/i,
    "当前操作需要人工恢复。",
  ],
  [
    /verification_inconclusive/i,
    "写入后验证未能形成可证明结论。",
  ],
  [
    /evidence_drift/i,
    "提交后目标证据发生变化，结果需要重新核对。",
  ],
  [
    /plan_expired/i,
    "同步计划已失效，目标或账号证据发生变化。",
  ],
  [
    /operation_failed/i,
    "操作在写入前失败，目标未应用本次变更。",
  ],
  [
    /operation_record_invalid/i,
    "操作记录不可用，不能把它当作空记录处理。",
  ],
  [
    /plan_unavailable|没有可执行的同步计划/i,
    "没有可用的同步计划，请重新核对范围后生成。",
  ],
  [
    /master_backup_running/i,
    "主库正在运行，请先关闭 TRAE 再创建备份（与切号同纪律）。",
  ],
  [
    /master_backup_failed/i,
    "主库数据备份未完成，原数据未受影响；请稍后重试。",
  ],
  [
    /backup_retention_invalid/i,
    "备份清理设置读取失败（配置文件可能损坏），已停止自动清理；请保留数据目录后反馈。",
  ],
  [
    /backup_retention_save_failed/i,
    "备份清理设置保存失败，现有设置保持不变；请稍后重试。",
  ],
  [
    /backup_retention_join_failed/i,
    "备份清理设置未能读取或保存，请稍后重试。",
  ],
  [
    /master_delete_running/i,
    "主库正在运行，真实删除需先关闭 TRAE（删除前会自动创建备份）。",
  ],
  [
    /master_archive_no_data/i,
    "主库还没有对话数据，无法执行该操作。",
  ],
  [
    /master_archive_open_failed/i,
    "主库对话库暂时无法打开，请稍后重试。",
  ],
  [
    /master_archive_unsupported/i,
    "当前主库版本不支持会话归档，请升级 TRAE 后重试。",
  ],
  [
    /master_archive_write_failed/i,
    "会话操作未完成，数据保持操作前状态；请稍后重试。",
  ],
  [
    /master_archive_backup_failed/i,
    "操作前的自动备份未完成，已取消本次操作；请稍后重试。",
  ],
  [
    /master_merge_running/i,
    "主库正在运行，合并分组需先关闭 TRAE（合并前会自动创建备份）。",
  ],
  [
    /master_merge_invalid/i,
    "分组选择已失效（分组可能已被移动或删除），请刷新列表后重试。",
  ],
  [
    /library_not_found/i,
    "未找到该对话库。",
  ],
  [
    /master_incorporate_busy/i,
    "主库正在生成回复，归入需等它完成；稍后重试即可。",
  ],
  [
    /incorporate_nothing_to_do/i,
    "主库内没有待归入的其他账号记录，无需操作。",
  ],
  [
    /incorporate_no_current_account/i,
    "当前账号信息不可用，请先在环境页确认账号后再归入。",
  ],
  [
    /master_incorporate_close_failed/i,
    "主库窗口未能关闭，本次未做任何修改；请手动关闭后重试。",
  ],
  [
    /master_incorporate_read_failed/i,
    "主库对话库暂时无法读取，请稍后重试。",
  ],
  [
    /master_incorporate_conflict/i,
    "存在同名项目的记录冲突，已停止归入以保护数据；请先在主库详情的对话列表中合并同名分组后再试。",
  ],
  [
    /master_incorporate_integrity_failed/i,
    "归入后数据校验未通过，已保留操作前备份；请重启工具后用备份恢复并反馈。",
  ],
  [
    /master_incorporate_join_failed/i,
    "归入操作未能完成，请稍后重试。",
  ],
  [
    /master_current_account_unavailable/i,
    "主库当前登录账号未确认，请先在环境页确认账号后重试。",
  ],
  [
    /plugin_tab_no_account/i,
    "主库当前没有已登录账号，无法读取插件状态；请先在环境页确认账号。",
  ],
  [
    /plugin_tab_credential_unavailable/i,
    "当前账号登录凭据暂不可用，请稍后重试或重新登录账号。",
  ],
  [
    /plugin_tab_cloud_unavailable/i,
    "插件云端列表暂时不可读取，请检查网络后重试。",
  ],
  [
    /plugin_tab_state_join_failed|plugin_market_join_failed|plugin_install_join_failed|plugin_uninstall_join_failed|plugin_absorb_join_failed/i,
    "插件操作未能完成，请稍后重试。",
  ],
  [
    /plugin_manifest_invalid/i,
    "主库插件清单文件损坏，已停止操作；请保留数据目录后反馈，不要手动编辑清单文件。",
  ],
  [
    /plugin_manifest_write_failed/i,
    "插件清单写入未完成，云端状态不受影响；请稍后重试。",
  ],
  [
    /plugin_market_unavailable/i,
    "插件市场目录暂时不可读取，请检查网络后重试。",
  ],
  [
    /plugin_install_failed/i,
    "安装未完成，云端状态不受影响；请稍后重试。",
  ],
  [
    /plugin_uninstall_failed/i,
    "移除未完成，云端状态不受影响；请稍后重试。",
  ],
  [
    /plugin_not_found/i,
    "该插件已不在当前账号的已装列表中，请刷新后重试。",
  ],
  [
    /plugin_builtin_uninstallable/i,
    "内置插件随客户端提供，无需也无法移除。",
  ],
  // ===== P6-4 环境管理 V2 =====
  [
    /environment_name_invalid/i,
    "环境名称不可用：请使用 1-64 个字符的名称（「主库」为保留名）。",
  ],
  [
    /environment_name_taken/i,
    "已有同名环境，请换一个名称。",
  ],
  [
    /environment_not_found/i,
    "该环境已不存在，请刷新列表后重试。",
  ],
  [
    /environment_master_immutable/i,
    "主库是默认环境，不支持改名或删除。",
  ],
  [
    /environment_master_use_switch/i,
    "主库内切换账号请在「账号」页发起。",
  ],
  [
    /environment_registry_invalid/i,
    "环境档案不可读取，请重启应用后重试；若持续出现请保留数据目录后反馈。",
  ],
  [
    /master_self_check_instance_running|master_self_check_process_unavailable/i,
    "主库仍在运行，请先关闭 TRAE 后再执行这项修复。",
  ],
  [
    /master_observed_account_unavailable/i,
    "暂时无法确认主库实际登录账号，请在 TRAE 内重新登录后再自检。",
  ],
  [
    /master_self_check_join_failed/i,
    "主库自检未能完成，请稍后重试。",
  ],
  [
    /environment_dir_unavailable/i,
    "环境目录创建失败，请检查磁盘后重试。",
  ],
  [
    /environment_delete_running/i,
    "该环境正在运行，请先关闭它的窗口再删除。",
  ],
  [
    /environment_delete_failed/i,
    "环境删除未完成，请稍后重试。",
  ],
  [
    /environment_login_busy/i,
    "该环境正在生成回复，请等它完成后再切换账号。",
  ],
  [
    /environment_login_close_failed/i,
    "环境窗口未能关闭，本次未做任何修改；请手动关闭后重试。",
  ],
  [
    /environment_login_backup_failed/i,
    "切换前的自动备份未完成，已取消本次操作；请稍后重试。",
  ],
  [
    /environment_login_conflict/i,
    "该环境内存在同名项目的记录冲突，已停止切换以保护数据；请先处理同名内容后再试。",
  ],
  [
    /environment_login_db_missing/i,
    "该环境还没有对话数据，无需处理记录。",
  ],
  [
    /environment_login_db_open_failed/i,
    "该环境的对话库暂时无法打开，请稍后重试。",
  ],
  [
    /environment_login_integrity_failed/i,
    "切换后数据校验未通过，已自动还原到切换前状态；请稍后重试。",
  ],
  [
    /environment_login_rollback_failed/i,
    "切换未完成且自动还原失败；请不要启动该环境，保留数据目录后反馈。",
  ],
  [
    /environment_seed_unavailable/i,
    "该账号还没有可用的登录存档，请先在账号页完成一次登录。",
  ],
  [
    /switch_donor_login_missing/i,
    "目标账号的登录凭据不可用（在线验证与本地存档均未通过）：请在账号页重新登录该账号后再试。",
  ],
  [
    /environment_login_failed/i,
    "环境账号切换未完成，请稍后重试。",
  ],
  [
    /environment_(list|create|rename|preview|delete|launch|login)_join_failed/i,
    "环境操作未能完成，请稍后重试。",
  ],
];

function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  if (typeof error === "object" && error !== null) {
    const candidate = error as { code?: unknown; message?: unknown };
    // Tauri 包装错误可能同时带有通用 code 和真正的中文 message；两者都要参与判定。
    const parts = [candidate.code, candidate.message].filter(
      (part): part is string => typeof part === "string",
    );
    if (parts.length > 0) return parts.join("\n");
  }
  return "";
}

/** 将后端异常映射为不会暴露路径、密钥或认证正文的界面提示。 */
export function safeUiErrorMessage(error: unknown, fallback: string): string {
  const text = errorText(error);
  for (const [pattern, message] of KNOWN_ERROR_MESSAGES) {
    if (pattern.test(text)) return message;
  }
  return fallback;
}

/** 只返回固定的后续动作，避免把后端 recommended_action 原文带到 UI。 */
export function safeUiRecommendedAction(error: unknown, fallback: string): string {
  const text = errorText(error);
  if (/checkin_http_disabled/i.test(text)) {
    return "继续使用历史扫描或账号切换；真实签到需等待独立能力 Gate 开放。";
  }
  if (/account_profile_store_busy/i.test(text)) {
    return "等待另一实例完成账号档案更新后再重试。";
  }
  if (/account_fingerprint_salt_unavailable/i.test(text)) {
    return "检查本地存储权限和磁盘状态；不要删除账号档案或安全摘要文件。";
  }
  if (/account_profile_store_salt_mismatch|account_profile_store_salt_binding_required/i.test(text)) {
    return "保留现有账号档案，不要手动覆盖或删除；使用同一安装环境重试。";
  }
  if (/account_profile_store_invalid|account_profile_store_path_invalid|account_fingerprint_salt_invalid/i.test(text)) {
    return "保留现有账号档案和失败证据，不要手动编辑档案文件。";
  }
  if (/catalog_write_protocol_upgrade_required|不支持的写入协议/i.test(text)) {
    return "保留目录库及其 sidecar；不要手动删除 WAL/SHM，等待后续目录库旁路升级功能。";
  }
  if (/storage_root_unavailable/i.test(text)) return "检查存储根和恢复区后重试。";
  if (/operation_state_unavailable/i.test(text)) {
    return "保留现有证据，稍后重新打开操作页重试。";
  }
  if (/another_operation_running|operation_lease_busy/i.test(text)) {
    return "等待当前操作结束后再重试。";
  }
  if (/plan_unavailable|没有可执行的同步计划/i.test(text)) {
    return "重新核对目标账号和范围，再生成计划。";
  }
  if (/gate_not_qualified/i.test(text)) {
    return "当前保持只读；待对应 Gate 通过后再使用。";
  }
  if (/manual_recovery_required/i.test(text)) {
    return "不要覆盖现有数据库，使用保留的备份和失败证据恢复。";
  }
  if (/evidence_drift/i.test(text)) {
    return "不要启动 TRAE；保留备份并重新读取操作状态。";
  }
  if (/plan_expired/i.test(text)) {
    return "重新确认目标账号和数据位置，再生成计划。";
  }
  if (/verification_inconclusive/i.test(text)) {
    return "不要启动 TRAE；保留失败现场并进入人工恢复。";
  }
  if (/operation_failed/i.test(text)) {
    return "保留备份并检查操作记录后重试。";
  }
  if (/operation_record_invalid/i.test(text)) {
    return "保留恢复区内容并进入人工恢复。";
  }
  return fallback;
}

/** 判断读取上下文是否已经失效；失效后 UI 必须清空旧内容并撤销本地授权。 */
export function isReadAuthorizationInvalidationError(error: unknown): boolean {
  return /authorization_mismatch|授权不匹配|请求范围与授权范围不一致|data_location_changed|data_location_unavailable|APPDATA (?:未设置|必须|不可用)|TRAE 默认(?:根目录|数据库)|TRAE 数据位置身份捕获失败|account_evidence_changed|account_evidence_unavailable|not_authorized|未授权扫描|后端未持有显式用户授权/i.test(
    errorText(error),
  );
}
