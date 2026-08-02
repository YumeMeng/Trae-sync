//! T02 当前账号证据读取实现：白名单日志、最新会话选择、明文兼容认证对象、
//! 认证字段 SHA-256 指纹、Local Storage 逻辑读取。
//!
//! 对应 Gate B：把已完成的真实格式调查实现为不保存凭证的生产解析器。
//!
//! 安全约束（handoff 第 86-95 行）：
//! - 只解析白名单事件：fetchLogTask、User info loaded、updateUserInfo/getUserInfo
//! - 严格区分 `userId` 与 `deviceId`：deviceId 绝不能成为 current_user_id
//! - 只持久化 SHA-256 指纹，不持久化认证正文
//! - Local Storage 使用逻辑读取接口，不扫描原始字节末次命中
//! - 未知/过期/冲突状态进入只读
//! - 手工账号选择不能改变可写状态（在 application 层 noop 体现）
//!
//! R2 修复（最新启动会话选择 + 真实白名单格式 + 合成账号 ID）：
//! - 不再聚合 logs 下全部历史 session；按会话目录 mtime 选择最新启动会话
//! - 解析器覆盖文档中的真实最小形态：
//!     `fetchLogTask { ..., "userId":"<synthetic-id>" }`
//!     `[RouteService] User info loaded { "userId":"<synthetic-id>" }`
//!     `[updateUserInfo]` / `[getUserInfo]` 中的 userId
//! - 仅白名单事件可产生账号证据；任意其他 `userId`、历史 session 和 `deviceId` 不得被采用
//! - 使用完全合成账号 ID；清除实现与测试中从技术调查文档复制的真实账号 ID
//!
//! R3 修复（完整账号状态机）：
//! - Verified 必须绑定 user_id、认证指纹和允许的证据组合；仅两类日志一致但无认证指纹时不得成为完整 Verified
//! - Expired 判定基于最新会话目录 mtime 与 `now` 比较，时间来源可注入
//!
//! R4 修复（明文兼容认证对象）：
//! - 当 storage.json 中 `iCubeAuthInfo://icube.cloudide` 的值可解析为 JSON 对象时，
//!   只提取 `userId`；Token、cookies 和其他认证字段不得进入 DTO、持久化、日志或证据
//! - 明文对象可作为规格允许的账号来源；格式错误或未知格式保守失败
//!
//! R2-1 修复（symlink/junction fixture 逃逸封闭）：
//! - 每个实际打开的目录与文件必须 canonical 后位于 canonical `fixture_root` 内
//! - session 目录本身也必须验证；根外 symlink/junction 不参与"最新 session"选择
//! - 根外路径保守忽略，绝不读取其内容
//! - 复用 `fixture_paths::path_strictly_inside`，避免跨层反向依赖
//!
//! Fixture 约定（仅 fixture 测试用）：
//! ```text
//! <fixture_root>/
//!   logs/<session_id>/
//!     alog.log
//!     renderer.log
//!     main.log
//!   globalStorage/storage.json
//!   Local Storage/local_storage.json   # 逻辑读取接口，非原始 leveldb
//!   product_version.txt                # 可选
//! ```

use crate::fixture_paths::path_strictly_inside;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use traesync_domain::{
    AccountEvidence, AuthFingerprint, EvidenceState, SourceEventSummary, UserId,
};
use traesync_ports::AccountEvidenceReaderPort;

/// 白名单事件名：ACCOUNT_DETECTION_FEASIBILITY.md 第 4 节确认的来源。
const WHITELIST_EVENTS: &[&str] = &[
    "fetchLogTask",
    "User info loaded",
    "updateUserInfo",
    "getUserInfo",
];

/// 来源类型分类
const SOURCE_ALOG: &str = "alog";
const SOURCE_RENDERER: &str = "renderer";
const SOURCE_MAIN: &str = "main";

/// R3 Expired 判定阈值：最新会话目录 mtime 早于此阈值时，证据被视为 Expired。
///
/// 选择 24 小时作为合理上限——TRAE 长时间未启动时，旧会话不能可靠反映当前登录状态。
/// 该值仅供 T02 fixture 测试；真实环境 Qualification 由用户单独批准。
const SESSION_EXPIRY_THRESHOLD: Duration = Duration::from_secs(24 * 60 * 60);

/// 账号证据读取器：实现 `AccountEvidenceReaderPort`。
///
/// 注意：本结构体不持有任何状态——`read_account_evidence` 每次重新扫描 fixture_root。
/// 真实环境中会按会话范围扫描，但 T02 阶段只扫描 fixture。
pub struct AccountEvidenceReader;

impl Default for AccountEvidenceReader {
    fn default() -> Self {
        Self::new()
    }
}

impl AccountEvidenceReader {
    pub fn new() -> Self {
        Self
    }
}

impl AccountEvidenceReaderPort for AccountEvidenceReader {
    /// 读取账号证据：扫描最新启动会话并组合证据。
    ///
    /// R2：不再聚合全部历史 session；只解析最新启动会话目录中的日志。
    /// R3：`now` 用于 `observed_at` 与 Expired 判定。
    fn read_account_evidence(&self, fixture_root: &Path, now: SystemTime) -> AccountEvidence {
        read_evidence_inner(fixture_root, now)
    }

    /// TRAE 关闭后重新读取——逻辑与初次读取相同，application 层比较指纹。
    ///
    /// R3：`now` 用于 `observed_at` 与 Expired 判定。
    fn re_read_after_close(&self, fixture_root: &Path, now: SystemTime) -> AccountEvidence {
        read_evidence_inner(fixture_root, now)
    }
}

/// 内部读取实现：选择最新启动会话并组合三类证据来源。
///
/// R2：扫描 `logs/<session_id>` 目录，按 mtime 降序选择最新会话；只解析该会话的日志文件。
/// 历史 session 中的事件不进入证据——避免跨历史聚合冲突。
///
/// 状态机优先级（R2-5/R2-6 修复）：
/// 1. **Conflict** 最高优先级——数据可信度问题比时间问题更严重。
///    - 日志事件 userId 冲突（R2-2/R2-3）
///    - Local Storage user_id 与日志/明文 userId 不一致（R2-7/R2-8，无条件）
///    - Conflict 状态下 user_id 必须为 None（P1：避免前端误用）
/// 2. **Expired** 次之——会话目录 mtime 早于 now - 阈值
///    - Expired 不再覆盖 Conflict（P2 修复）
/// 3. **Missing/SingleSource/Verified** 最后——由 `decide_evidence_state` 决定
///
/// R2-1：所有读取路径（session 目录、alog.log、renderer.log、main.log、
/// storage.json、local_storage.json）必须 canonical 后位于 canonical fixture_root 内；
/// 根外 symlink/junction 保守忽略，绝不读取其内容。
fn read_evidence_inner(fixture_root: &Path, now: SystemTime) -> AccountEvidence {
    // R2-1：fixture_root 必须可 canonicalize——失败时返回默认 Missing 证据
    // 上层 application 已通过 FixturePathGuard 验证 fixture_root，此处仍防御性 canonicalize
    let canonical_root = match fixture_root.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            return AccountEvidence {
                user_id: None,
                source_events: Vec::new(),
                auth_fingerprint: None,
                local_storage_user_id: None,
                product_version: None,
                observed_at: now,
                evidence_state: EvidenceState::Missing,
            };
        }
    };

    // 1. 选择最新启动会话目录（R2 + R2-1：session 目录 canonical 必须在 fixture_root 内）
    let latest_session = pick_latest_session(&canonical_root);

    // 2. 只解析最新会话中的白名单日志事件（R2 + R2-1：每个日志文件 canonical 检查）
    let mut events_with_uid: Vec<(SourceEventSummary, Option<UserId>)> = Vec::new();
    if let Some((session_path, session_id, _session_mtime)) = &latest_session {
        parse_log_file(
            &session_path.join("alog.log"),
            &canonical_root,
            SOURCE_ALOG,
            session_id,
            &mut events_with_uid,
        );
        parse_log_file(
            &session_path.join("renderer.log"),
            &canonical_root,
            SOURCE_RENDERER,
            session_id,
            &mut events_with_uid,
        );
        parse_log_file(
            &session_path.join("main.log"),
            &canonical_root,
            SOURCE_MAIN,
            session_id,
            &mut events_with_uid,
        );
    }

    // 3. 解析 storage.json 生成认证指纹与可能的明文 userId（R4 + R2-1：canonical 检查）
    let storage_path = canonical_root.join("globalStorage").join("storage.json");
    let (auth_fingerprint, product_version, plaintext_user_id) =
        parse_storage_for_fingerprint(&storage_path, &canonical_root);

    // 4. Local Storage 逻辑读取（R2-1：canonical 检查）
    let local_storage_user_id = read_local_storage_logical(&canonical_root);

    // 5. 决定 user_id：白名单事件一致优先；其次明文兼容认证对象的 userId（R4）
    //    R2-2/R2-3：日志事件 userId 冲突时 pick_consistent 返回 None，
    //    但 events 中存在 Some(uid) —— 这种情况必须在状态机中标记为 Conflict，
    //    不能让明文 userId 覆盖冲突判定。
    let log_user_id = pick_consistent_user_id(&events_with_uid);
    let log_events_have_uid = events_with_uid.iter().any(|(_, uid)| uid.is_some());
    // R2-2/R2-3：有 userId 证据但不一致——无论来源类型数量，都视为 Conflict
    let log_events_conflict = log_events_have_uid && log_user_id.is_none();
    let user_id = log_user_id.or(plaintext_user_id);

    // 6. 提取事件摘要（不含 userId 正文）
    let source_events: Vec<SourceEventSummary> =
        events_with_uid.into_iter().map(|(e, _)| e).collect();

    // 7. R2-5/R2-6：Conflict 优先于 Expired
    //    - 日志事件 userId 冲突 → Conflict（user_id 必须为 None）
    //    - Local Storage user_id 与日志/明文 userId 不一致 → Conflict（user_id 必须为 None）
    //    P1：Conflict 状态下 user_id 不应被填充——避免前端 UI 显示"账号 X（冲突）"误导用户
    //    P2：会话过期但 events 冲突时优先返回 Conflict——数据可信度比时间问题更严重
    let ls_uid_mismatch = matches!(&user_id, Some(uid) if matches!(&local_storage_user_id, Some(ls_uid) if uid != ls_uid));
    if log_events_conflict || ls_uid_mismatch {
        return AccountEvidence {
            user_id: None, // P1：Conflict 状态下 user_id 必须为 None
            source_events,
            auth_fingerprint,
            local_storage_user_id,
            product_version,
            observed_at: now,
            evidence_state: EvidenceState::Conflict,
        };
    }

    // 8. R3 Expired 判定：会话目录 mtime 早于 now - 阈值
    //    P2 修复：仅在非 Conflict 时检查 Expired
    if let Some((_, _, session_mtime)) = latest_session {
        if let Ok(age) = now.duration_since(session_mtime) {
            if age > SESSION_EXPIRY_THRESHOLD {
                // Expired 状态可显示 user_id 摘要——会话过老但账号证据本身一致
                return AccountEvidence {
                    user_id,
                    source_events,
                    auth_fingerprint,
                    local_storage_user_id,
                    product_version,
                    observed_at: now,
                    evidence_state: EvidenceState::Expired,
                };
            }
        }
    }

    // 9. 决定 evidence_state（Missing/SingleSource/Verified）
    //    R3：Verified 必须绑定指纹
    //    ls_uid 不一致已在步骤 7 拦截，这里 ls_uid 与 user_id 必然一致或其一为 None
    let evidence_state = decide_evidence_state(
        &source_events,
        &user_id,
        &local_storage_user_id,
        &auth_fingerprint,
    );

    AccountEvidence {
        user_id,
        source_events,
        auth_fingerprint,
        local_storage_user_id,
        product_version,
        observed_at: now,
        evidence_state,
    }
}

/// R2：选择最新启动会话目录。
///
/// 扫描 `fixture_root/logs/` 下的所有子目录，按 mtime 降序返回最新的一个。
/// 返回 (path, session_id, mtime) 或 None（无会话目录）。
///
/// R2-1：每个 session 目录必须 canonical 后严格位于 `canonical_root` 内。
/// 根外 symlink/junction 不参与"最新 session"选择——canonicalize 会跟随链接，
/// 随后 `path_strictly_inside` 检查会拒绝逃逸目录。
fn pick_latest_session(canonical_root: &Path) -> Option<(PathBuf, String, SystemTime)> {
    // R2-1：防御性 canonicalize root——调用方可能传入非 canonical 路径（如 Windows tempdir
    // 无 `\\?\` 前缀，而 session 目录 canonicalize 后带前缀，导致 containment 比较失败）。
    // canonicalize 对已 canonical 路径幂等；失败时回退原值（上层 read_evidence_inner 已防御）。
    let canonical_root = canonical_root
        .canonicalize()
        .unwrap_or_else(|_| canonical_root.to_path_buf());
    let logs_dir = canonical_root.join("logs");
    let mut best: Option<(PathBuf, String, SystemTime)> = None;
    if !logs_dir.is_dir() {
        return None;
    }
    let entries = std::fs::read_dir(&logs_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // R2-1：session 目录 canonical 必须严格位于 canonical_root 内
        // symlink/junction 逃逸目录被 canonicalize 解析后落在 root 外，直接跳过
        let canonical_session = match path.canonicalize() {
            Ok(c) => c,
            Err(_) => continue,
        };
        if !path_strictly_inside(&canonical_session, &canonical_root) {
            continue;
        }
        let session_id = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let mtime = match std::fs::metadata(&canonical_session).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        match &best {
            None => best = Some((canonical_session, session_id, mtime)),
            Some((_, _, best_mtime)) => {
                if mtime > *best_mtime {
                    best = Some((canonical_session, session_id, mtime));
                }
            }
        }
    }
    best
}

/// 解析单个日志文件，提取白名单事件的 userId。
///
/// R2：支持文档中的真实最小形态：
/// - `fetchLogTask { ..., "userId":"<synthetic-id>" }`（JSON 嵌套形式）
/// - `[RouteService] User info loaded { "userId":"<synthetic-id>" }`
/// - `[updateUserInfo]` / `[getUserInfo]` 中的 `userId`
/// - 兼容旧式 `fetchLogTask.userId = <digits>` 与 `User info loaded userId=<digits>`
///
/// 不匹配 deviceId 字段——deviceId 在白名单事件中不出现。
///
/// R2-1：`path` 必须 canonical 后严格位于 `canonical_root` 内；否则保守忽略，
/// 绝不读取其内容。symlink/junction 逃逸文件被 canonicalize 解析后落在 root 外。
fn parse_log_file(
    path: &Path,
    canonical_root: &Path,
    source_kind: &str,
    session_id: &str,
    out: &mut Vec<(SourceEventSummary, Option<UserId>)>,
) {
    // R2-1：防御性 canonicalize root（见 pick_latest_session 同名注释）
    let canonical_root = canonical_root
        .canonicalize()
        .unwrap_or_else(|_| canonical_root.to_path_buf());
    // R2-1：canonical containment 检查——失败保守忽略，不读取内容
    let canonical_path = match path.canonicalize() {
        Ok(c) => c,
        Err(_) => return,
    };
    if !path_strictly_inside(&canonical_path, &canonical_root) {
        return;
    }

    let content = match std::fs::read_to_string(&canonical_path) {
        Ok(s) => s,
        Err(_) => return,
    };

    for line in content.lines() {
        for event_name in WHITELIST_EVENTS {
            if let Some(uid) = extract_user_id_for_event(line, event_name) {
                let summary = SourceEventSummary {
                    source_kind: source_kind.to_string(),
                    event_name: event_name.to_string(),
                    log_session_id: Some(session_id.to_string()),
                };
                out.push((summary, Some(uid)));
            }
        }
    }
}

/// 从日志行提取白名单事件对应的 userId。
///
/// R2：覆盖文档列出的真实最小形态：
/// 1. `fetchLogTask { ..., "userId":"<digits>" }`（JSON 嵌套形式）
/// 2. `[RouteService] User info loaded { "userId":"<digits>" }`
/// 3. `[updateUserInfo] userId=<digits>` / `[getUserInfo] userId=<digits>`
/// 4. 兼容旧式 `fetchLogTask.userId = <digits>`、`updateUserInfo.userId: <digits>`
///
/// 不匹配 deviceId——白名单事件不含 deviceId 字段。
///
/// R2-4：事件名前后必须为词边界（行首/行尾或非字母数字且非下划线），
/// 防止 `somefetchLogTask` 或 `fetchLogTaskExtra` 这类非白名单事件被子串匹配。
fn extract_user_id_for_event(line: &str, event_name: &str) -> Option<UserId> {
    let line_lower = line.to_lowercase();
    let event_lower = event_name.to_lowercase();
    let bytes = line_lower.as_bytes();

    // R2-4：在 line_lower 中查找 event_lower 的词边界出现位置
    // event_name 前一个字符必须是行开头或非字母数字且非下划线（词边界）
    // event_name 后一个字符必须是行尾或非字母数字且非下划线（词边界）
    let mut search_from = 0;
    let pos;
    loop {
        let rel_pos = line_lower[search_from..].find(&event_lower)?;
        let abs_pos = search_from + rel_pos;

        // 检查 event_name 前一个字符是否为词边界
        let prev_is_boundary = abs_pos == 0 || {
            let prev_byte = bytes[abs_pos - 1];
            !(prev_byte.is_ascii_alphanumeric() || prev_byte == b'_')
        };

        // 检查 event_name 后一个字符是否为词边界
        let after_event_pos = abs_pos + event_lower.len();
        let next_is_boundary = after_event_pos >= bytes.len() || {
            let next_byte = bytes[after_event_pos];
            !(next_byte.is_ascii_alphanumeric() || next_byte == b'_')
        };

        if prev_is_boundary && next_is_boundary {
            pos = abs_pos;
            break;
        }
        search_from = abs_pos + event_lower.len();
    }

    let after = &line[pos + event_lower.len()..];

    // 尝试两种格式：
    // (a) JSON 嵌套形式：` { ..., "userId":"<digits>" }` —— 查找 "userid":"<digits>"
    // (b) 旧式：`.userId = <digits>` / ` userId=<digits>` / `.userId: <digits>`
    if let Some(uid) = extract_json_user_id(after) {
        return Some(uid);
    }
    extract_plain_user_id(after)
}

/// 从 JSON 嵌套形式提取 userId。
///
/// 匹配 `"userId":"<digits>"` 或 `"userId": <digits>`（容忍空格与引号变体）。
fn extract_json_user_id(after: &str) -> Option<UserId> {
    let lower = after.to_lowercase();
    let key_pos = lower.find("\"userid\"")?;
    let after_key = &after[key_pos + "\"userid\"".len()..];
    // 跳过空白与冒号
    let after_colon = after_key.trim_start();
    let after_colon = after_colon.strip_prefix(':').unwrap_or(after_colon);
    let after_colon = after_colon.trim_start();
    // 跳过可选引号
    let after_quote = after_colon.strip_prefix('"').unwrap_or(after_colon);
    // 提取连续数字
    let digits_end = after_quote
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit())
        .map(|(i, _)| i)
        .unwrap_or(after_quote.len());
    if digits_end == 0 {
        return None;
    }
    UserId::from_verified(&after_quote[..digits_end]).ok()
}

/// 从旧式 `userId = <digits>` / `userId: <digits>` / `userId=<digits>` / `.userId = <digits>` 提取。
///
/// R2：支持 event_name 之后跟随多种前缀组合：
/// - 直接 `userid` 开头：`userId=123`、`userId: 123`、`userId = 123`
/// - `.userid` 开头：`.userId = 123`、`.userId:123`
/// - ` userid` 开头（带前导空白）：` userId=123`
/// - 任意非字母数字前缀后跟 `userid`：在 lower 中查找 "userid" 子串位置，
///   但要求前一个字符为词边界（非字母数字）——防止从 `deviceId_userId=...`
///   这类连体字段名中误提取（R2-1 修复）
fn extract_plain_user_id(after: &str) -> Option<UserId> {
    let lower = after.to_lowercase();
    let bytes = lower.as_bytes();
    // 在 lower 中查找 "userid" 子串位置——要求词边界
    // R2-1：前一个字符必须为非字母数字且非下划线（或字符串开头），
    // 防止匹配 `deviceId_userId` 这类连体字段名中的 `userId` 子串
    let mut search_from = 0;
    let mut userid_pos: Option<usize> = None;
    while let Some(rel_pos) = lower[search_from..].find("userid") {
        let abs_pos = search_from + rel_pos;
        let is_word_boundary = abs_pos == 0 || {
            // 词边界：前一个字符不是 ASCII 字母、数字或下划线
            // 防止匹配 `deviceId_userId` 这类连体字段名（`_` 是标识符连接符，视为非词边界）
            let prev_byte = bytes[abs_pos - 1];
            !(prev_byte.is_ascii_alphanumeric() || prev_byte == b'_')
        };
        if is_word_boundary {
            userid_pos = Some(abs_pos);
            break;
        }
        search_from = abs_pos + "userid".len();
    }
    let userid_pos = userid_pos?;
    let after_userid = &lower[userid_pos + "userid".len()..];
    // 跳过分隔符 `.`、`=`、`:`、空白（支持 `userId.=123`、`userId = 123` 等组合）
    let after_sep = after_userid.trim_start_matches(['.', '=', ':', ' ', '\t']);
    // 提取连续数字（13-19 位）
    let digits_end = after_sep
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit())
        .map(|(i, _)| i)
        .unwrap_or(after_sep.len());
    if digits_end == 0 {
        return None;
    }
    UserId::from_verified(&after_sep[..digits_end]).ok()
}

/// 从所有白名单事件中提取一致 userId。
///
/// 收集每个事件的 userId，若全部一致则返回 Some；若任一不一致返回 None（标记 Conflict）。
fn pick_consistent_user_id(events: &[(SourceEventSummary, Option<UserId>)]) -> Option<UserId> {
    let mut first: Option<UserId> = None;
    for (_, uid_opt) in events {
        match (first.as_ref(), uid_opt) {
            (None, Some(uid)) => first = Some(uid.clone()),
            (Some(existing), Some(uid)) => {
                if existing != uid {
                    return None; // 冲突
                }
            }
            _ => {}
        }
    }
    first
}

/// 决定 `EvidenceState`（R3）：
/// - 0 个白名单事件且无明文 userId → Missing
/// - 0 个白名单事件但有明文 userId → SingleSource（明文兼容降级）
/// - 1 个来源类型 → SingleSource
/// - 2+ 来源类型且 userId 一致且 **绑定认证指纹** → Verified
/// - 2+ 来源类型但 userId 不一致 → Conflict
/// - 2+ 来源类型一致但无认证指纹 → SingleSource（R3：无指纹不得成为完整 Verified）
/// - Local Storage user_id 与日志/明文 userId 不一致 → Conflict（R2-7/R2-8，无条件）
///
/// R3 关键修复：`auth_fingerprint` 必须为 Some 才能成为 Verified。
///
/// R2-7/R2-8 修复：ls_uid 不一致检查移到顶部（无条件）。
/// 原实现仅在 source_kinds>=2 时检查，单一来源或明文 userId 候选下漏检。
/// 规格第 436 行注释明确要求"Local Storage user_id 与日志 userId 不一致 → Conflict"
/// 是无条件的——任何 candidate（含 SingleSource、明文 userId）都应升级为 Conflict。
///
/// 注意：`read_evidence_inner` 步骤 7 已拦截 ls_uid 不一致并返回 Conflict，
/// 此处的检查是防御性兜底——若未来调用者绕过 `read_evidence_inner` 直接调用
/// `decide_evidence_state`，仍能保证 ls_uid 不一致时返回 Conflict。
fn decide_evidence_state(
    events: &[SourceEventSummary],
    user_id: &Option<UserId>,
    local_storage_user_id: &Option<UserId>,
    auth_fingerprint: &Option<AuthFingerprint>,
) -> EvidenceState {
    // R2-7/R2-8：ls_uid 与 user_id 不一致 → Conflict（无条件，防御性兜底）
    if let (Some(uid), Some(ls_uid)) = (user_id, local_storage_user_id) {
        if uid != ls_uid {
            return EvidenceState::Conflict;
        }
    }

    if events.is_empty() {
        // R4：无白名单事件但有明文 userId 时降级为 SingleSource
        if user_id.is_some() {
            return EvidenceState::SingleSource;
        }
        return EvidenceState::Missing;
    }

    // 来源类型去重计数
    let mut source_kinds = std::collections::HashSet::new();
    for e in events {
        source_kinds.insert(e.source_kind.as_str());
    }

    if source_kinds.len() >= 2 {
        // 两类以上来源一致 → Verified（user_id 已通过 pick_consistent 校验）
        // ls_uid 不一致已在顶部检查
        if user_id.is_some() {
            // R3：必须绑定认证指纹才能成为 Verified；否则降级 SingleSource
            if auth_fingerprint.is_some() {
                EvidenceState::Verified
            } else {
                EvidenceState::SingleSource
            }
        } else {
            // 来源类型多个但 userId 不一致
            EvidenceState::Conflict
        }
    } else {
        // 单一来源类型
        EvidenceState::SingleSource
    }
}

/// 解析 storage.json 生成认证字段 SHA-256 指纹与可能的明文兼容 userId（R4）。
///
/// 返回 (fingerprint, product_version, plaintext_user_id)：
/// - fingerprint：iCubeAuthInfo://* 字段的 SHA-256 指纹（不保留正文）
/// - product_version：从 JSON 顶层或 iCubeAuthInfo 字段中提取（若有）
/// - plaintext_user_id：R4 明文兼容认证对象的 userId；密文格式返回 None
///
/// R4：当 `iCubeAuthInfo://icube.cloudide` 的值可解析为 JSON 对象时，
/// 只提取 `userId`；Token、cookies 和其他字段不进入返回值。
fn parse_storage_for_fingerprint(
    path: &Path,
    canonical_root: &Path,
) -> (Option<AuthFingerprint>, Option<String>, Option<UserId>) {
    // R2-1：防御性 canonicalize root（见 pick_latest_session 同名注释）
    let canonical_root = canonical_root
        .canonicalize()
        .unwrap_or_else(|_| canonical_root.to_path_buf());
    // R2-1：canonical containment 检查——失败保守返回 None，不读取内容
    let canonical_path = match path.canonicalize() {
        Ok(c) => c,
        Err(_) => return (None, None, None),
    };
    if !path_strictly_inside(&canonical_path, &canonical_root) {
        return (None, None, None);
    }

    let content = match std::fs::read_to_string(&canonical_path) {
        Ok(s) => s,
        Err(_) => return (None, None, None),
    };

    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return (None, None, None),
    };

    // 提取 iCubeAuthInfo://* 字段（Base64 包装的 tc 二进制密文 或 明文 JSON 字符串）
    let mut found_values: Vec<(String, String)> = Vec::new();
    let mut plaintext_user_id: Option<UserId> = None;
    let mut product_version: Option<String> = None;

    if let Some(obj) = json.as_object() {
        for (k, v) in obj {
            if k.starts_with("iCubeAuthInfo://") {
                if let Some(s) = v.as_str() {
                    found_values.push((k.clone(), s.to_string()));
                    // R4：尝试将值解析为 JSON 对象，提取 userId（明文兼容格式）
                    if plaintext_user_id.is_none() {
                        if let Ok(inner) = serde_json::from_str::<serde_json::Value>(s) {
                            if let Some(uid_str) = inner.get("userId").and_then(|u| u.as_str()) {
                                plaintext_user_id = UserId::from_verified(uid_str).ok();
                            }
                            // 提取嵌套 productVersion（若有）
                            if product_version.is_none() {
                                if let Some(pv) =
                                    inner.get("productVersion").and_then(|v| v.as_str())
                                {
                                    product_version = Some(pv.to_string());
                                }
                            }
                        }
                        // 解析失败（密文 Base64）保守返回 None——不抛错
                    }
                }
            }
            // 顶层 productVersion
            if k == "productVersion" {
                if let Some(pv) = v.as_str() {
                    product_version = Some(pv.to_string());
                }
            }
        }
    }

    if found_values.is_empty() {
        return (None, product_version, plaintext_user_id);
    }

    // 生成 SHA-256：按字段名排序后哈希 (name|value|version)
    found_values.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (name, value) in &found_values {
        hasher.update(name.as_bytes());
        hasher.update(b"|");
        hasher.update(value.as_bytes());
        hasher.update(b"|");
        if let Some(pv) = &product_version {
            hasher.update(pv.as_bytes());
        }
        hasher.update(b"\n");
    }
    let fingerprint = hex::encode(hasher.finalize());
    (
        Some(AuthFingerprint(fingerprint)),
        product_version,
        plaintext_user_id,
    )
}

/// Local Storage 逻辑读取接口。
///
/// 不扫描原始 leveldb 字节——直接读取 fixture 提供的 `local_storage.json`。
/// 该 JSON 表示 leveldb 的逻辑视图，避免引入 leveldb 依赖与原始字节扫描。
///
/// 文件格式：
/// ```json
/// {
///   "current_user_id": "<synthetic-id>"
/// }
/// ```
fn read_local_storage_logical(canonical_root: &Path) -> Option<UserId> {
    // R2-1：防御性 canonicalize root（见 pick_latest_session 同名注释）
    let canonical_root = canonical_root
        .canonicalize()
        .unwrap_or_else(|_| canonical_root.to_path_buf());
    let path = canonical_root
        .join("Local Storage")
        .join("local_storage.json");
    // R2-1：canonical containment 检查——失败保守返回 None，不读取内容
    let canonical_path = path.canonicalize().ok()?;
    if !path_strictly_inside(&canonical_path, &canonical_root) {
        return None;
    }
    let content = std::fs::read_to_string(&canonical_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    let uid_str = json.get("current_user_id")?.as_str()?;
    UserId::from_verified(uid_str).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;
    use traesync_domain::{EvidenceState, SourceEventSummary, UserId};

    // R2：所有测试使用合成账号 ID，不使用真实账号 ID
    const SYNTHETIC_USER_ID_A: &str = "1000000000000001";
    const SYNTHETIC_USER_ID_B: &str = "1000000000000002";
    const SYNTHETIC_DEVICE_ID: &str = "2000000000000001";

    /// 测试辅助：写入日志文件
    fn write_log(dir: &Path, session: &str, file: &str, content: &str) {
        let session_dir = dir.join("logs").join(session);
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join(file), content).unwrap();
    }

    /// R2 测试辅助：写入日志文件并设置会话目录 mtime
    ///
    /// 使用 `filetime` crate 设置目录 mtime——Windows 上需要
    /// `FILE_FLAG_BACKUP_SEMANTICS` 才能打开目录并设置时间戳，
    /// `std::fs::File::options().write(true).open(&dir)` 会失败。
    fn write_log_with_mtime(
        dir: &Path,
        session: &str,
        file: &str,
        content: &str,
        mtime: SystemTime,
    ) {
        let session_dir = dir.join("logs").join(session);
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join(file), content).unwrap();
        // 使用 filetime 设置目录 mtime——跨平台且支持 Windows 目录
        let file_time = filetime::FileTime::from_system_time(mtime);
        filetime::set_file_mtime(&session_dir, file_time).expect("set session dir mtime failed");
    }

    /// 测试辅助：写入 storage.json
    fn write_storage(dir: &Path, content: &str) {
        let storage_dir = dir.join("globalStorage");
        fs::create_dir_all(&storage_dir).unwrap();
        fs::write(storage_dir.join("storage.json"), content).unwrap();
    }

    /// 测试辅助：写入 Local Storage 逻辑 JSON
    fn write_local_storage(dir: &Path, current_user_id: Option<&str>) {
        let ls_dir = dir.join("Local Storage");
        fs::create_dir_all(&ls_dir).unwrap();
        let content = match current_user_id {
            Some(uid) => format!(r#"{{"current_user_id": "{}"}}"#, uid),
            None => r#"{}"#.to_string(),
        };
        fs::write(ls_dir.join("local_storage.json"), content).unwrap();
    }

    /// 测试用 `now` 时间——使用 wall clock 当前时间。
    ///
    /// R3 要求 `now` 可注入（`read_account_evidence` 接收 `now` 参数），这里用
    /// `SystemTime::now()` 是因为 `write_log` 创建的会话目录 mtime 也是当前时间，
    /// 两者差值 < 1s 远小于 SESSION_EXPIRY_THRESHOLD(24h)，不会误触发 Expired。
    /// Expired 专用测试用 `write_log_with_mtime` 设置很老的 mtime 验证判定逻辑。
    fn fixed_now() -> SystemTime {
        SystemTime::now()
    }

    // ============== AC1：白名单事件解析（R2 真实格式） ==============

    #[test]
    fn parse_log_file_extracts_fetchlogtask_json_form() {
        let dir = tempdir().unwrap();
        // R2：真实 JSON 嵌套形式（严格 JSON，键名带双引号，符合 ACCOUNT_DETECTION_FEASIBILITY.md）
        write_log(
            dir.path(),
            "session-1",
            "alog.log",
            &format!(
                r#"2026-07-30 fetchLogTask {{ "machineId": "abc", "deviceId": "{}", "userId": "{}" }}"#,
                SYNTHETIC_DEVICE_ID, SYNTHETIC_USER_ID_A,
            ),
        );
        let mut events: Vec<(SourceEventSummary, Option<UserId>)> = Vec::new();
        parse_log_file(
            &dir.path().join("logs").join("session-1").join("alog.log"),
            dir.path(),
            SOURCE_ALOG,
            "session-1",
            &mut events,
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0.event_name, "fetchLogTask");
        assert_eq!(events[0].0.source_kind, SOURCE_ALOG);
        assert_eq!(events[0].1.as_ref().unwrap().as_str(), SYNTHETIC_USER_ID_A);
    }

    #[test]
    fn parse_log_file_extracts_user_info_loaded_json_form() {
        let dir = tempdir().unwrap();
        write_log(
            dir.path(),
            "session-1",
            "renderer.log",
            &format!(
                r#"[RouteService] User info loaded {{ "userId": "{}" }}"#,
                SYNTHETIC_USER_ID_A
            ),
        );
        let mut events: Vec<(SourceEventSummary, Option<UserId>)> = Vec::new();
        parse_log_file(
            &dir.path()
                .join("logs")
                .join("session-1")
                .join("renderer.log"),
            dir.path(),
            SOURCE_RENDERER,
            "session-1",
            &mut events,
        );
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0.event_name, "User info loaded");
        assert_eq!(events[0].1.as_ref().unwrap().as_str(), SYNTHETIC_USER_ID_A);
    }

    #[test]
    fn parse_log_file_extracts_updateuserinfo_and_getuserinfo() {
        let dir = tempdir().unwrap();
        write_log(
            dir.path(),
            "session-1",
            "main.log",
            &format!(
                "[updateUserInfo] userId={}\n[getUserInfo] userId={}",
                SYNTHETIC_USER_ID_A, SYNTHETIC_USER_ID_A
            ),
        );
        let mut events: Vec<(SourceEventSummary, Option<UserId>)> = Vec::new();
        parse_log_file(
            &dir.path().join("logs").join("session-1").join("main.log"),
            dir.path(),
            SOURCE_MAIN,
            "session-1",
            &mut events,
        );
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn parse_log_file_ignores_non_whitelist_events() {
        let dir = tempdir().unwrap();
        write_log(
            dir.path(),
            "session-1",
            "main.log",
            &format!(
                "deviceId={}\nsomeOtherEvent.userId=1234567890123",
                SYNTHETIC_DEVICE_ID
            ),
        );
        let mut events: Vec<(SourceEventSummary, Option<UserId>)> = Vec::new();
        parse_log_file(
            &dir.path().join("logs").join("session-1").join("main.log"),
            dir.path(),
            SOURCE_MAIN,
            "session-1",
            &mut events,
        );
        // 非白名单事件不被解析——deviceId 字段从不被读取
        assert_eq!(events.len(), 0);
    }

    // ============== AC2：来源一致/缺失/冲突/单来源 ==============

    #[test]
    fn two_source_consistent_returns_verified_with_fingerprint() {
        let dir = tempdir().unwrap();
        write_log(
            dir.path(),
            "session-1",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_A),
        );
        write_log(
            dir.path(),
            "session-1",
            "renderer.log",
            &format!(
                r#"[RouteService] User info loaded {{ "userId": "{}" }}"#,
                SYNTHETIC_USER_ID_A
            ),
        );
        write_storage(
            dir.path(),
            &format!(
                r#"{{"iCubeAuthInfo://icube.cloudide":"dGVzdC1jaXBoZXItdGV4dA==","productVersion":"1.107.1"}}"#
            ),
        );
        write_local_storage(dir.path(), Some(SYNTHETIC_USER_ID_A));

        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), fixed_now());

        // R3：Verified 必须绑定认证指纹
        assert_eq!(evidence.evidence_state, EvidenceState::Verified);
        assert!(evidence.auth_fingerprint.is_some());
        assert_eq!(evidence.product_version.as_deref(), Some("1.107.1"));
    }

    #[test]
    fn two_source_consistent_without_fingerprint_returns_single_source() {
        // R3：仅两类日志一致但无认证指纹时不得成为完整 Verified
        let dir = tempdir().unwrap();
        write_log(
            dir.path(),
            "session-1",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_A),
        );
        write_log(
            dir.path(),
            "session-1",
            "renderer.log",
            &format!(
                r#"[RouteService] User info loaded {{ "userId": "{}" }}"#,
                SYNTHETIC_USER_ID_A
            ),
        );
        // 无 storage.json，无指纹

        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), fixed_now());

        assert_eq!(evidence.evidence_state, EvidenceState::SingleSource);
        assert!(evidence.auth_fingerprint.is_none());
    }

    #[test]
    fn single_source_returns_single_source() {
        let dir = tempdir().unwrap();
        write_log(
            dir.path(),
            "session-1",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_A),
        );
        // 只有一类来源
        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), fixed_now());
        assert_eq!(evidence.evidence_state, EvidenceState::SingleSource);
    }

    #[test]
    fn missing_returns_missing() {
        let dir = tempdir().unwrap();
        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), fixed_now());
        assert_eq!(evidence.evidence_state, EvidenceState::Missing);
        assert!(evidence.user_id.is_none());
    }

    #[test]
    fn conflict_returns_conflict_when_local_storage_differs() {
        let dir = tempdir().unwrap();
        write_log(
            dir.path(),
            "session-1",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_A),
        );
        write_log(
            dir.path(),
            "session-1",
            "renderer.log",
            &format!(
                r#"[RouteService] User info loaded {{ "userId": "{}" }}"#,
                SYNTHETIC_USER_ID_A
            ),
        );
        write_storage(
            dir.path(),
            r#"{"iCubeAuthInfo://icube.cloudide":"dGVzdA==","productVersion":"1.107.1"}"#,
        );
        write_local_storage(dir.path(), Some(SYNTHETIC_USER_ID_B));

        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), fixed_now());
        assert_eq!(evidence.evidence_state, EvidenceState::Conflict);
    }

    // ============== R2-2：日志事件 userId 冲突不被明文覆盖 ==============

    #[test]
    fn conflict_in_log_events_yields_conflict_even_with_plaintext_user_id() {
        // R2-2：两类来源 userId 不一致时，即使明文认证对象有 userId 也不能覆盖冲突判定
        let dir = tempdir().unwrap();
        // alog.log: fetchLogTask userId=A
        write_log(
            dir.path(),
            "session-1",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_A),
        );
        // renderer.log: User info loaded userId=B（不同 userId）
        write_log(
            dir.path(),
            "session-1",
            "renderer.log",
            &format!(
                r#"[RouteService] User info loaded {{ "userId": "{}" }}"#,
                SYNTHETIC_USER_ID_B
            ),
        );
        // storage.json: 明文 userId=A（与 alog 一致，但与 renderer 冲突）
        let plaintext_obj = format!(r#"{{"userId":"{}"}}"#, SYNTHETIC_USER_ID_A);
        let storage_content = format!(
            r#"{{"iCubeAuthInfo://icube.cloudide":{}}}"#,
            serde_json::to_string(&plaintext_obj).unwrap()
        );
        write_storage(dir.path(), &storage_content);

        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), fixed_now());
        // 必须返回 Conflict，不能因为明文 userId=A 而误判 Verified
        assert_eq!(
            evidence.evidence_state,
            EvidenceState::Conflict,
            "日志事件 userId 冲突时，明文 userId 不应覆盖冲突判定"
        );
    }

    #[test]
    fn conflict_in_single_source_kind_yields_conflict() {
        // R2-3：单一 source_kind 内 userId 冲突也应返回 Conflict
        let dir = tempdir().unwrap();
        // alog.log 两行：fetchLogTask userId=A + fetchLogTask userId=B
        write_log(
            dir.path(),
            "session-1",
            "alog.log",
            &format!(
                r#"fetchLogTask {{ "userId": "{}" }}
fetchLogTask {{ "userId": "{}" }}"#,
                SYNTHETIC_USER_ID_A, SYNTHETIC_USER_ID_B
            ),
        );

        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), fixed_now());
        // 单一来源内部 userId 冲突也应返回 Conflict，不能误判 SingleSource
        assert_eq!(
            evidence.evidence_state,
            EvidenceState::Conflict,
            "单一 source_kind 内 userId 冲突应返回 Conflict"
        );
    }

    // ============== AC3：明文兼容认证对象只提取 userId（R4） ==============

    #[test]
    fn parse_storage_extracts_plaintext_user_id_from_icube_cloudide() {
        // R4：明文兼容认证对象的 iCubeAuthInfo://icube.cloudide 值可解析为 JSON，
        // 只提取 userId；Token、cookies 等字段不进入返回值
        let dir = tempdir().unwrap();
        // R4：format! 中所有字面 `{` 必须写成 `{{`、字面 `}` 必须写成 `}}`，
        // 只保留 `{}` 作为占位符。否则 `"cookies":{"session":...}` 会被当作非法占位符。
        let plaintext_obj = format!(
            r#"{{"userId":"{}","token":"secret-token","cookies":{{"session":"secret"}},"refreshToken":"secret-refresh"}}"#,
            SYNTHETIC_USER_ID_A
        );
        // R4：在 raw string 内不能使用 \" 转义——raw string 保留反斜杠字面量，
        // 且 format! 会把 \" 当作非法格式串语法。这里用普通字符串拼接构造 storage.json。
        let storage_content = format!(
            "{{\"iCubeAuthInfo://icube.cloudide\":{},\"productVersion\":\"1.107.1\"}}",
            serde_json::to_string(&plaintext_obj).unwrap()
        );
        write_storage(dir.path(), &storage_content);

        let (_, _, plaintext_uid) = parse_storage_for_fingerprint(
            &dir.path().join("globalStorage").join("storage.json"),
            dir.path(),
        );
        assert!(plaintext_uid.is_some());
        assert_eq!(plaintext_uid.unwrap().as_str(), SYNTHETIC_USER_ID_A);
    }

    #[test]
    fn parse_storage_no_plaintext_uid_for_ciphertext() {
        // 密文格式（Base64 包装的 tc 二进制）不能解析为 JSON，返回 None
        let dir = tempdir().unwrap();
        write_storage(
            dir.path(),
            r#"{"iCubeAuthInfo://icube.cloudide":"dGVzdC1jaXBoZXItdGV4dA==","productVersion":"1.107.1"}"#,
        );
        let (_, _, plaintext_uid) = parse_storage_for_fingerprint(
            &dir.path().join("globalStorage").join("storage.json"),
            dir.path(),
        );
        assert!(plaintext_uid.is_none());
    }

    #[test]
    fn plaintext_user_id_yields_single_source_when_no_logs() {
        // R4：明文对象可作为规格允许的账号来源；无日志时降级为 SingleSource
        let dir = tempdir().unwrap();
        let plaintext_obj = format!(r#"{{"userId":"{}"}}"#, SYNTHETIC_USER_ID_A);
        let storage_content = format!(
            r#"{{"iCubeAuthInfo://icube.cloudide":{}}}"#,
            serde_json::to_string(&plaintext_obj).unwrap()
        );
        write_storage(dir.path(), &storage_content);

        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), fixed_now());
        assert_eq!(evidence.evidence_state, EvidenceState::SingleSource);
        assert!(evidence.user_id.is_some());
        assert_eq!(
            evidence.user_id.as_ref().unwrap().as_str(),
            SYNTHETIC_USER_ID_A
        );
    }

    #[test]
    fn plaintext_user_id_format_error_returns_none() {
        // R4：格式错误保守失败——明文对象 userId 字段格式不合法时返回 None
        let dir = tempdir().unwrap();
        let plaintext_obj = r#"{"userId":"not-a-valid-id"}"#;
        let storage_content = format!(
            r#"{{"iCubeAuthInfo://icube.cloudide":{}}}"#,
            serde_json::to_string(plaintext_obj).unwrap()
        );
        write_storage(dir.path(), &storage_content);

        let (_, _, plaintext_uid) = parse_storage_for_fingerprint(
            &dir.path().join("globalStorage").join("storage.json"),
            dir.path(),
        );
        assert!(plaintext_uid.is_none());
    }

    #[test]
    fn parse_storage_generates_fingerprint_without_persisting_content() {
        let dir = tempdir().unwrap();
        write_storage(
            dir.path(),
            r#"{"iCubeAuthInfo://default":"dGVzdC1jaXBoZXItdGV4dA==","productVersion":"1.107.1"}"#,
        );

        let (fingerprint, version, _) = parse_storage_for_fingerprint(
            &dir.path().join("globalStorage").join("storage.json"),
            dir.path(),
        );
        assert!(fingerprint.is_some());
        assert_eq!(version.as_deref(), Some("1.107.1"));
        // 指纹是 SHA-256 hex（64 字符）
        assert_eq!(fingerprint.unwrap().0.len(), 64);
    }

    #[test]
    fn parse_storage_fingerprint_deterministic_for_same_content() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        let content = r#"{"iCubeAuthInfo://default":"dGVzdA==","productVersion":"1.107.1"}"#;
        write_storage(dir1.path(), content);
        write_storage(dir2.path(), content);

        let (fp1, _, _) = parse_storage_for_fingerprint(
            &dir1.path().join("globalStorage").join("storage.json"),
            dir1.path(),
        );
        let (fp2, _, _) = parse_storage_for_fingerprint(
            &dir2.path().join("globalStorage").join("storage.json"),
            dir2.path(),
        );
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn parse_storage_fingerprint_changes_with_different_value() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        write_storage(dir1.path(), r#"{"iCubeAuthInfo://default":"dGVzdA=="}"#);
        write_storage(dir2.path(), r#"{"iCubeAuthInfo://default":"ZGlmZmVyZW50"}"#);

        let (fp1, _, _) = parse_storage_for_fingerprint(
            &dir1.path().join("globalStorage").join("storage.json"),
            dir1.path(),
        );
        let (fp2, _, _) = parse_storage_for_fingerprint(
            &dir2.path().join("globalStorage").join("storage.json"),
            dir2.path(),
        );
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn parse_storage_returns_none_when_no_icube_field() {
        let dir = tempdir().unwrap();
        write_storage(dir.path(), r#"{"otherField":"value"}"#);
        let (fp, _, plaintext_uid) = parse_storage_for_fingerprint(
            &dir.path().join("globalStorage").join("storage.json"),
            dir.path(),
        );
        assert!(fp.is_none());
        assert!(plaintext_uid.is_none());
    }

    // ============== AC5：deviceId 与 userId 区分 ==============

    #[test]
    fn device_id_never_becomes_user_id() {
        // 即使日志中出现 deviceId 字段，白名单事件解析也不会捕获它
        let dir = tempdir().unwrap();
        write_log(
            dir.path(),
            "session-1",
            "main.log",
            &format!(
                "deviceId={}\nfetchLogTask {{ \"userId\": \"{}\" }}",
                SYNTHETIC_DEVICE_ID, SYNTHETIC_USER_ID_A
            ),
        );
        let mut events = Vec::new();
        parse_log_file(
            &dir.path().join("logs").join("session-1").join("main.log"),
            dir.path(),
            SOURCE_MAIN,
            "session-1",
            &mut events,
        );
        // 只解析 fetchLogTask 事件，不解析 deviceId
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0.event_name, "fetchLogTask");
    }

    #[test]
    fn user_id_from_verified_rejects_too_short_device_id_format() {
        // 短于 13 位的数字串不能成为 UserId
        let result = UserId::from_verified("249");
        assert!(result.is_err());
    }

    // ============== AC6：Local Storage 逻辑读取 ==============

    #[test]
    fn local_storage_logical_read_returns_user_id() {
        let dir = tempdir().unwrap();
        write_local_storage(dir.path(), Some(SYNTHETIC_USER_ID_A));
        let uid = read_local_storage_logical(dir.path());
        assert!(uid.is_some());
        assert_eq!(uid.unwrap().as_str(), SYNTHETIC_USER_ID_A);
    }

    #[test]
    fn local_storage_logical_read_returns_none_when_missing() {
        let dir = tempdir().unwrap();
        write_local_storage(dir.path(), None);
        let uid = read_local_storage_logical(dir.path());
        assert!(uid.is_none());
    }

    #[test]
    fn local_storage_logical_read_returns_none_when_file_absent() {
        let dir = tempdir().unwrap();
        let uid = read_local_storage_logical(dir.path());
        assert!(uid.is_none());
    }

    // ============== AC7：认证指纹变化使结果只读 ==============

    #[test]
    fn fingerprint_changes_when_storage_value_differs() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        write_storage(dir1.path(), r#"{"iCubeAuthInfo://default":"aaaa"}"#);
        write_storage(dir2.path(), r#"{"iCubeAuthInfo://default":"bbbb"}"#);

        let (fp1, _, _) = parse_storage_for_fingerprint(
            &dir1.path().join("globalStorage").join("storage.json"),
            dir1.path(),
        );
        let (fp2, _, _) = parse_storage_for_fingerprint(
            &dir2.path().join("globalStorage").join("storage.json"),
            dir2.path(),
        );
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn re_read_after_close_returns_fresh_evidence() {
        let dir = tempdir().unwrap();
        write_log(
            dir.path(),
            "session-1",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_A),
        );
        write_log(
            dir.path(),
            "session-1",
            "renderer.log",
            &format!(
                r#"[RouteService] User info loaded {{ "userId": "{}" }}"#,
                SYNTHETIC_USER_ID_A
            ),
        );
        write_storage(
            dir.path(),
            r#"{"iCubeAuthInfo://default":"dGVzdA==","productVersion":"1.107.1"}"#,
        );

        let reader = AccountEvidenceReader::new();
        let first = reader.read_account_evidence(dir.path(), fixed_now());
        let second = reader.re_read_after_close(dir.path(), fixed_now());

        // 同一 fixture 应产生相同证据
        assert_eq!(first.evidence_state, second.evidence_state);
        assert_eq!(first.auth_fingerprint, second.auth_fingerprint);
    }

    // ============== R2：最新启动会话选择 ==============

    #[test]
    fn pick_latest_session_returns_newest_mtime_dir() {
        let dir = tempdir().unwrap();
        let older = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let newer = SystemTime::UNIX_EPOCH + Duration::from_secs(1_999_000_000);
        write_log_with_mtime(
            dir.path(),
            "session-old",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_B),
            older,
        );
        write_log_with_mtime(
            dir.path(),
            "session-new",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_A),
            newer,
        );

        let latest = pick_latest_session(dir.path()).expect("应有最新会话");
        assert_eq!(latest.1, "session-new");
    }

    #[test]
    fn latest_session_overrides_historical_sessions() {
        // R2：两个历史账号 session + 一个最新账号 session，结果应来自最新 session
        let dir = tempdir().unwrap();
        let t_old_1 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let t_old_2 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000);
        let t_new = SystemTime::UNIX_EPOCH + Duration::from_secs(1_900_000_000);

        // 历史 session 1：账号 B
        write_log_with_mtime(
            dir.path(),
            "session-old-1",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_B),
            t_old_1,
        );
        // 历史 session 2：账号 B
        write_log_with_mtime(
            dir.path(),
            "session-old-2",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_B),
            t_old_2,
        );
        // 最新 session：账号 A
        write_log_with_mtime(
            dir.path(),
            "session-new",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_A),
            t_new,
        );
        write_log_with_mtime(
            dir.path(),
            "session-new",
            "renderer.log",
            &format!(
                r#"[RouteService] User info loaded {{ "userId": "{}" }}"#,
                SYNTHETIC_USER_ID_A
            ),
            t_new,
        );
        write_storage(
            dir.path(),
            r#"{"iCubeAuthInfo://default":"dGVzdA==","productVersion":"1.107.1"}"#,
        );

        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), fixed_now());

        // 应来自最新 session：账号 A
        assert_eq!(evidence.user_id.unwrap().as_str(), SYNTHETIC_USER_ID_A);
        assert_eq!(evidence.evidence_state, EvidenceState::Verified);
    }

    #[test]
    fn pick_latest_session_returns_none_when_logs_absent() {
        let dir = tempdir().unwrap();
        assert!(pick_latest_session(dir.path()).is_none());
    }

    // ============== R3：Expired 判定（基于 session mtime 与 now） ==============

    #[test]
    fn expired_when_latest_session_too_old() {
        // R3：会话 mtime 早于 now - SESSION_EXPIRY_THRESHOLD 时进入 Expired
        let dir = tempdir().unwrap();
        let very_old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000); // 远早于 now
        write_log_with_mtime(
            dir.path(),
            "session-old",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_A),
            very_old,
        );
        write_log_with_mtime(
            dir.path(),
            "session-old",
            "renderer.log",
            &format!(
                r#"[RouteService] User info loaded {{ "userId": "{}" }}"#,
                SYNTHETIC_USER_ID_A
            ),
            very_old,
        );
        write_storage(
            dir.path(),
            r#"{"iCubeAuthInfo://default":"dGVzdA==","productVersion":"1.107.1"}"#,
        );

        // now 远晚于 session mtime + 阈值
        let now = very_old + SESSION_EXPIRY_THRESHOLD + Duration::from_secs(60);
        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), now);

        assert_eq!(evidence.evidence_state, EvidenceState::Expired);
    }

    #[test]
    fn not_expired_when_session_within_threshold() {
        let dir = tempdir().unwrap();
        let recent = fixed_now() - Duration::from_secs(60); // 1 分钟前
        write_log_with_mtime(
            dir.path(),
            "session-recent",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_A),
            recent,
        );
        write_log_with_mtime(
            dir.path(),
            "session-recent",
            "renderer.log",
            &format!(
                r#"[RouteService] User info loaded {{ "userId": "{}" }}"#,
                SYNTHETIC_USER_ID_A
            ),
            recent,
        );
        write_storage(
            dir.path(),
            r#"{"iCubeAuthInfo://default":"dGVzdA==","productVersion":"1.107.1"}"#,
        );

        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), fixed_now());
        assert_eq!(evidence.evidence_state, EvidenceState::Verified);
    }

    // ============== 解析器格式覆盖（R2） ==============

    #[test]
    fn extract_user_id_supports_multiple_separators() {
        // 旧式格式兼容
        let uid1 = extract_user_id_for_event(
            &format!("fetchLogTask.userId = {}", SYNTHETIC_USER_ID_A),
            "fetchLogTask",
        );
        assert!(uid1.is_some());

        let uid2 = extract_user_id_for_event(
            &format!("User info loaded userId={}", SYNTHETIC_USER_ID_A),
            "User info loaded",
        );
        assert!(uid2.is_some());

        let uid3 = extract_user_id_for_event(
            &format!("updateUserInfo.userId: {}", SYNTHETIC_USER_ID_A),
            "updateUserInfo",
        );
        assert!(uid3.is_some());
    }

    #[test]
    fn extract_user_id_supports_json_nested_form() {
        // R2：JSON 嵌套形式
        let uid1 = extract_user_id_for_event(
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_A),
            "fetchLogTask",
        );
        assert!(uid1.is_some());

        let uid2 = extract_user_id_for_event(
            &format!(
                r#"[RouteService] User info loaded {{ "userId": "{}" }}"#,
                SYNTHETIC_USER_ID_A
            ),
            "User info loaded",
        );
        assert!(uid2.is_some());
    }

    #[test]
    fn extract_user_id_rejects_non_whitelist_event() {
        let uid = extract_user_id_for_event(
            &format!("someOtherEvent.userId = {}", SYNTHETIC_USER_ID_A),
            "fetchLogTask",
        );
        assert!(uid.is_none());
    }

    // ============== R2-1：词边界匹配（防止连体字段名误提取） ==============

    #[test]
    fn extract_plain_user_id_rejects_userid_substring_in_concatenated_field() {
        // R2-1：`deviceId_userId=<digits>` 中的 `userId` 子串不应被提取
        // 前一个字符是 `d`（字母），不是词边界——必须返回 None
        let uid = extract_plain_user_id(&format!("deviceId_userId={}", SYNTHETIC_DEVICE_ID));
        assert!(uid.is_none(), "deviceId_userId 中的 userId 子串不应被提取");
    }

    #[test]
    fn extract_plain_user_id_accepts_word_boundary_prefixes() {
        // R2-1：合法词边界前缀仍应正常提取
        // 空白前缀：` userId=123`
        let uid1 = extract_plain_user_id(&format!(" userId={}", SYNTHETIC_USER_ID_A));
        assert!(uid1.is_some(), "空白前缀应正常提取");

        // `.` 前缀：`.userId=123`
        let uid2 = extract_plain_user_id(&format!(".userId={}", SYNTHETIC_USER_ID_A));
        assert!(uid2.is_some(), ". 前缀应正常提取");

        // `[` 前缀：`[userId=123`
        let uid3 = extract_plain_user_id(&format!("[userId={}", SYNTHETIC_USER_ID_A));
        assert!(uid3.is_some(), "[ 前缀应正常提取");

        // 字符串开头：`userId=123`
        let uid4 = extract_plain_user_id(&format!("userId={}", SYNTHETIC_USER_ID_A));
        assert!(uid4.is_some(), "字符串开头应正常提取");
    }

    #[test]
    fn extract_user_id_for_event_ignores_concatenated_device_id_field() {
        // R2-1 集成验证：fetchLogTask 行中包含 `deviceId_userId=<device-id>` 不应误提取
        let line = format!(
            r#"fetchLogTask {{ "deviceId": "{}", "deviceId_userId": "{}" }}"#,
            SYNTHETIC_DEVICE_ID, SYNTHETIC_DEVICE_ID
        );
        let uid = extract_user_id_for_event(&line, "fetchLogTask");
        // JSON 解析路径会优先尝试 `"userId":"<digits>"`，本行没有 `"userId"` 字段，
        // 旧式路径会查找 `userid` 子串——必须因词边界检查返回 None
        assert!(
            uid.is_none(),
            "fetchLogTask 行中的 deviceId_userId 连体字段不应被误提取为 userId"
        );
    }

    // ============== R2-4：事件名词边界匹配（防止非白名单事件子串误匹配） ==============

    #[test]
    fn extract_user_id_rejects_event_substring_in_non_whitelist_event() {
        // R2-4：`somefetchLogTask userId=123` 不应被当作 `fetchLogTask` 事件处理
        // event_name 前一个字符是字母（`e`），不是词边界——必须返回 None
        let uid = extract_user_id_for_event(
            &format!("somefetchLogTask userId={}", SYNTHETIC_USER_ID_A),
            "fetchLogTask",
        );
        assert!(
            uid.is_none(),
            "somefetchLogTask 中的 fetchLogTask 子串不应被匹配为白名单事件"
        );
    }

    #[test]
    fn extract_user_id_rejects_event_suffix_in_non_whitelist_event() {
        // R2-4：`fetchLogTaskExtra userId=123` 不应被当作 `fetchLogTask` 事件处理
        // event_name 后一个字符是字母（`E`），不是词边界——必须返回 None
        let uid = extract_user_id_for_event(
            &format!("fetchLogTaskExtra userId={}", SYNTHETIC_USER_ID_A),
            "fetchLogTask",
        );
        assert!(
            uid.is_none(),
            "fetchLogTaskExtra 不应被匹配为 fetchLogTask 白名单事件"
        );
    }

    #[test]
    fn extract_user_id_accepts_event_with_word_boundary_prefix() {
        // R2-4：合法词边界前缀仍应正常匹配
        // 行开头：`fetchLogTask userId=123`
        let uid1 = extract_user_id_for_event(
            &format!("fetchLogTask userId={}", SYNTHETIC_USER_ID_A),
            "fetchLogTask",
        );
        assert!(uid1.is_some(), "行开头应正常匹配");

        // 空格前缀：` fetchLogTask userId=123`
        let uid2 = extract_user_id_for_event(
            &format!(" fetchLogTask userId={}", SYNTHETIC_USER_ID_A),
            "fetchLogTask",
        );
        assert!(uid2.is_some(), "空格前缀应正常匹配");

        // `[` 前缀：`[fetchLogTask] userId=123`
        let uid3 = extract_user_id_for_event(
            &format!("[fetchLogTask] userId={}", SYNTHETIC_USER_ID_A),
            "fetchLogTask",
        );
        assert!(uid3.is_some(), "[ 前缀应正常匹配");
    }

    // ============== R2-5：Conflict 状态下 user_id 必须为 None ==============
    // P1：Conflict 表示账号不可信，user_id 字段不应被填充。
    // 若 user_id 含明文 userId，前端 UI 会显示"账号 1000…0001（冲突）"
    // 误导用户认为该 ID 是当前账号。

    #[test]
    fn conflict_state_user_id_is_none_even_with_plaintext_user_id() {
        // P1：日志事件 userId 冲突时，明文 userId 不应进入 user_id 字段
        let dir = tempdir().unwrap();
        write_log(
            dir.path(),
            "session-1",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_A),
        );
        write_log(
            dir.path(),
            "session-1",
            "renderer.log",
            &format!(
                r#"[RouteService] User info loaded {{ "userId": "{}" }}"#,
                SYNTHETIC_USER_ID_B
            ),
        );
        // storage.json 明文 userId=A（与 alog 一致，但与 renderer 冲突）
        let plaintext_obj = format!(r#"{{"userId":"{}"}}"#, SYNTHETIC_USER_ID_A);
        let storage_content = format!(
            r#"{{"iCubeAuthInfo://icube.cloudide":{}}}"#,
            serde_json::to_string(&plaintext_obj).unwrap()
        );
        write_storage(dir.path(), &storage_content);

        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), fixed_now());
        assert_eq!(evidence.evidence_state, EvidenceState::Conflict);
        // P1：user_id 必须为 None，不能含明文 userId
        assert!(
            evidence.user_id.is_none(),
            "Conflict 状态下 user_id 必须为 None，实际：{:?}",
            evidence.user_id
        );
    }

    // ============== R2-6：Expired 不应覆盖 Conflict ==============
    // P2：会话过期但 events 内部 userId 冲突时应优先返回 Conflict。
    // Conflict 表示数据可信度问题，比 Expired（时间问题）更严重。

    #[test]
    fn expired_session_with_log_events_conflict_returns_conflict() {
        // P2：会话目录 mtime 过老 + events userId 冲突 → Conflict（而非 Expired）
        let dir = tempdir().unwrap();
        let very_old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        // alog.log: userId=A
        write_log_with_mtime(
            dir.path(),
            "session-old",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_A),
            very_old,
        );
        // renderer.log: userId=B（与 alog 冲突）
        write_log_with_mtime(
            dir.path(),
            "session-old",
            "renderer.log",
            &format!(
                r#"[RouteService] User info loaded {{ "userId": "{}" }}"#,
                SYNTHETIC_USER_ID_B
            ),
            very_old,
        );
        write_storage(
            dir.path(),
            r#"{"iCubeAuthInfo://default":"dGVzdA==","productVersion":"1.107.1"}"#,
        );

        let now = very_old + SESSION_EXPIRY_THRESHOLD + Duration::from_secs(60);
        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), now);
        // P2：必须返回 Conflict，不能因为会话过期而掩盖冲突
        assert_eq!(
            evidence.evidence_state,
            EvidenceState::Conflict,
            "会话过期但 events 冲突时应优先返回 Conflict"
        );
        // P1：Conflict 状态下 user_id 必须为 None
        assert!(evidence.user_id.is_none());
    }

    // ============== R2-7：SingleSource 候选下 ls_uid 不一致应升级 Conflict ==============
    // P3：规格第 436 行注释说"Local Storage user_id 与日志 userId 不一致 → Conflict"是无条件的。
    // 原实现仅在 source_kinds>=2 时检查，单一来源下漏检。

    #[test]
    fn single_source_with_ls_uid_mismatch_returns_conflict() {
        // P3：单一来源（alog.log）+ ls_uid 不一致 → Conflict
        let dir = tempdir().unwrap();
        write_log(
            dir.path(),
            "session-1",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_A),
        );
        // Local Storage user_id=B（与日志 A 不一致）
        write_local_storage(dir.path(), Some(SYNTHETIC_USER_ID_B));

        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), fixed_now());
        // P3：单一来源下 ls_uid 不一致也应升级为 Conflict
        assert_eq!(
            evidence.evidence_state,
            EvidenceState::Conflict,
            "SingleSource 候选下 ls_uid 不一致应升级为 Conflict"
        );
    }

    // ============== R2-8：明文 userId 与 ls_uid 不一致应 Conflict ==============
    // P9：events 空 + 明文 userId + ls_uid 不一致时返回 SingleSource 是错的。
    // 规格要求 Local Storage user_id 与日志 userId 不一致 → Conflict（无条件）。
    // 明文 userId 也是规格允许的账号来源，与 ls_uid 不一致同样应标记 Conflict。

    #[test]
    fn plaintext_user_id_with_ls_uid_mismatch_returns_conflict() {
        // P9：events 空 + 明文 userId=A + ls_uid=B → Conflict
        let dir = tempdir().unwrap();
        // 无日志文件——events 为空
        let plaintext_obj = format!(r#"{{"userId":"{}"}}"#, SYNTHETIC_USER_ID_A);
        let storage_content = format!(
            r#"{{"iCubeAuthInfo://icube.cloudide":{}}}"#,
            serde_json::to_string(&plaintext_obj).unwrap()
        );
        write_storage(dir.path(), &storage_content);
        // Local Storage user_id=B（与明文 A 不一致）
        write_local_storage(dir.path(), Some(SYNTHETIC_USER_ID_B));

        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), fixed_now());
        // P9：明文 userId 与 ls_uid 不一致应升级为 Conflict
        assert_eq!(
            evidence.evidence_state,
            EvidenceState::Conflict,
            "明文 userId 与 Local Storage user_id 不一致应返回 Conflict"
        );
    }

    #[test]
    fn plaintext_user_id_consistent_with_ls_returns_single_source() {
        // P9 回归测试：明文 userId 与 ls_uid 一致时仍应返回 SingleSource
        let dir = tempdir().unwrap();
        let plaintext_obj = format!(r#"{{"userId":"{}"}}"#, SYNTHETIC_USER_ID_A);
        let storage_content = format!(
            r#"{{"iCubeAuthInfo://icube.cloudide":{}}}"#,
            serde_json::to_string(&plaintext_obj).unwrap()
        );
        write_storage(dir.path(), &storage_content);
        write_local_storage(dir.path(), Some(SYNTHETIC_USER_ID_A));

        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), fixed_now());
        assert_eq!(evidence.evidence_state, EvidenceState::SingleSource);
    }

    // ============== R2-1：symlink/junction fixture 逃逸封闭 ==============
    // handoff 第 56-72 行：账号证据读取路径中的文件/目录若为 symlink/junction
    // 可逃出 fixture_root。每条读取路径必须 canonical 后位于 canonical root 内；
    // 根外 symlink/junction 保守忽略，绝不读取其内容。
    //
    // 测试不得在 symlink 创建失败时静默 return 后宣称 PASS——必须 panic 报告 BLOCKED。

    /// 测试辅助：尽力创建文件 symlink/junction。
    /// Windows 上优先 `symlink_file`，失败时退回 junction（仅目录）。
    /// 两者均失败返回 false——调用方需 panic 报告 BLOCKED，不得静默跳过。
    fn symlink_best_effort(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        {
            if std::os::windows::fs::symlink_file(target, link).is_ok() {
                return true;
            }
            if target.is_dir() {
                if std::os::windows::fs::symlink_dir(target, link).is_ok() {
                    return true;
                }
                // junction 兜底：cmd /c mklink /J "<link>" "<target>"
                let out = std::process::Command::new("cmd")
                    .args([
                        "/C",
                        "mklink",
                        "/J",
                        &link.to_string_lossy(),
                        &target.to_string_lossy(),
                    ])
                    .output();
                if let Ok(o) = out {
                    if o.status.success() {
                        return true;
                    }
                }
            }
            false
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
    }

    /// R2-1 AC1：最新 session 目录 symlink 逃逸——根外 session 不参与"最新"选择。
    #[test]
    fn r2_1_latest_session_symlink_escape_is_ignored() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        // 在 fixture 内创建合法 session-old
        write_log_with_mtime(
            dir.path(),
            "session-old",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_A),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000),
        );
        // 在 fixture 内创建 session-escape 作为 symlink 指向 outside
        let logs_dir = dir.path().join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        let escape_link = logs_dir.join("session-escape");
        let created = symlink_best_effort(outside.path(), &escape_link);
        if !created {
            panic!(
                "BLOCKED: cannot create symlink/junction for r2_1_latest_session_symlink_escape_is_ignored"
            );
        }
        // outside 目录 mtime 较新——若不拦截会被选为最新 session
        // 给 outside 写入文件并设置较新 mtime
        std::fs::write(outside.path().join("alog.log"), b"").unwrap();
        let file_time = filetime::FileTime::from_system_time(SystemTime::now());
        filetime::set_file_mtime(outside.path(), file_time).unwrap();

        let latest = pick_latest_session(dir.path());
        // 必须选 session-old，绝不能选 symlink 逃逸的 session-escape
        assert!(latest.is_some(), "应有合法 session-old 被选中");
        assert_ne!(latest.unwrap().1, "session-escape");
    }

    /// R2-1 AC2：单个日志文件 symlink/junction 逃逸——parse_log_file 不读取根外内容。
    ///
    /// Windows 无开发者模式时文件 symlink 不可靠创建，改用目录 junction
    /// （`mklink /J`，无需提升权限）作为逃逸载体：session 目录本身是 junction 指向
    /// 根外目录，alog.log 路径 canonicalize 后落在 fixture_root 外。
    #[test]
    fn r2_1_log_file_symlink_escape_is_ignored() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        // outside 中放置"陷阱" alog.log，含错误 userId
        std::fs::write(
            outside.path().join("alog.log"),
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_B),
        )
        .unwrap();
        // fixture 内 logs/session-junction 作为 junction 指向 outside
        let logs_dir = dir.path().join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        let junction_path = logs_dir.join("session-junction");
        let created = symlink_best_effort(outside.path(), &junction_path);
        if !created {
            panic!(
                "BLOCKED: cannot create symlink/junction for r2_1_log_file_symlink_escape_is_ignored"
            );
        }
        // alog.log 路径经 junction 后 canonicalize 落在 outside
        let link_path = junction_path.join("alog.log");

        let mut events: Vec<(SourceEventSummary, Option<UserId>)> = Vec::new();
        parse_log_file(
            &link_path,
            dir.path(),
            SOURCE_ALOG,
            "session-junction",
            &mut events,
        );
        // 逃逸 junction 不被读取——events 必须为空
        assert!(
            events.is_empty(),
            "junction 逃逸日志文件不应被读取，但 events 非空"
        );
    }

    /// R2-1 AC3：storage.json symlink/junction 逃逸——parse_storage_for_fingerprint 不读取根外内容。
    ///
    /// 同 AC2：Windows 用目录 junction 作为逃逸载体，globalStorage 目录是 junction
    /// 指向根外，storage.json 路径 canonicalize 后落在 fixture_root 外。
    #[test]
    fn r2_1_storage_json_symlink_escape_is_ignored() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        // outside 中放置"陷阱" storage.json，含明文 userId
        let plaintext_obj = format!(r#"{{"userId":"{}"}}"#, SYNTHETIC_USER_ID_B);
        let trap_content = format!(
            r#"{{"iCubeAuthInfo://icube.cloudide":{}}}"#,
            serde_json::to_string(&plaintext_obj).unwrap()
        );
        std::fs::write(outside.path().join("storage.json"), &trap_content).unwrap();
        // fixture 内 globalStorage-junction 作为 junction 指向 outside
        let junction_path = dir.path().join("globalStorage-junction");
        let created = symlink_best_effort(outside.path(), &junction_path);
        if !created {
            panic!(
                "BLOCKED: cannot create symlink/junction for r2_1_storage_json_symlink_escape_is_ignored"
            );
        }
        // storage.json 路径经 junction 后 canonicalize 落在 outside
        let link_path = junction_path.join("storage.json");

        let (fp, _, plaintext_uid) = parse_storage_for_fingerprint(&link_path, dir.path());
        // 逃逸 junction 不被读取——指纹与明文 userId 必须为 None
        assert!(
            fp.is_none(),
            "junction 逃逸 storage.json 不应被读取，但指纹非 None"
        );
        assert!(
            plaintext_uid.is_none(),
            "junction 逃逸 storage.json 不应暴露明文 userId"
        );
    }

    /// R2-1 AC4：Local Storage logical JSON symlink/junction 逃逸——read_local_storage_logical 不读取根外内容。
    ///
    /// 同 AC2：Windows 用目录 junction 作为逃逸载体，`Local Storage` 目录本身是
    /// junction 指向根外，local_storage.json 路径 canonicalize 后落在 fixture_root 外。
    #[test]
    fn r2_1_local_storage_symlink_escape_is_ignored() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        // outside 陷阱 local_storage.json 含 userId B
        std::fs::write(
            outside.path().join("local_storage.json"),
            &format!(r#"{{"current_user_id":"{}"}}"#, SYNTHETIC_USER_ID_B),
        )
        .unwrap();
        // fixture 内 "Local Storage" 作为 junction 指向 outside
        let junction_path = dir.path().join("Local Storage");
        let created = symlink_best_effort(outside.path(), &junction_path);
        if !created {
            panic!(
                "BLOCKED: cannot create symlink/junction for r2_1_local_storage_symlink_escape_is_ignored"
            );
        }

        let uid = read_local_storage_logical(dir.path());
        // 逃逸 junction 不被读取——uid 必须为 None
        assert!(
            uid.is_none(),
            "junction 逃逸 local_storage.json 不应暴露 userId"
        );
    }

    /// R2-1 AC5：正常 fixture 读取不回归——非 symlink 路径仍能正常读取。
    #[test]
    fn r2_1_normal_fixture_read_not_regressed() {
        let dir = tempdir().unwrap();
        write_log(
            dir.path(),
            "session-1",
            "alog.log",
            &format!(r#"fetchLogTask {{ "userId": "{}" }}"#, SYNTHETIC_USER_ID_A),
        );
        write_log(
            dir.path(),
            "session-1",
            "renderer.log",
            &format!(
                r#"[RouteService] User info loaded {{ "userId": "{}" }}"#,
                SYNTHETIC_USER_ID_A
            ),
        );
        write_storage(
            dir.path(),
            r#"{"iCubeAuthInfo://default":"dGVzdA==","productVersion":"1.107.1"}"#,
        );
        write_local_storage(dir.path(), Some(SYNTHETIC_USER_ID_A));

        let reader = AccountEvidenceReader::new();
        let evidence = reader.read_account_evidence(dir.path(), fixed_now());
        // 正常 fixture 路径仍应返回 Verified
        assert_eq!(evidence.evidence_state, EvidenceState::Verified);
        assert_eq!(
            evidence.user_id.as_ref().unwrap().as_str(),
            SYNTHETIC_USER_ID_A
        );
        assert!(evidence.auth_fingerprint.is_some());
    }
}
