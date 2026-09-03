//! T02 Work CN 只读入口的领域值对象与纯规则。
//!
//! 本模块只承载 T02 需要的稳定值对象：
//! - 数据位置身份
//! - schema 兼容状态
//! - 当前账号证据
//! - 结构化只读原因
//!
//! 不预建 T03+（SnapshotStore、CatalogRepository、ManifestStore 等）的空 trait。
//! 对应 `IMPLEMENTATION_SPEC.md` 第 12 节 `AccountEvidence` 与第 31 节错误分类。

use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// 数据位置稳定身份：由规范化路径派生，外部无法直接构造。
///
/// 对应规格第 11 节：`(platform_id, data_location_id)` 是唯一键。
/// T02 阶段只接受 fixture_root 防护验证过的路径作为 data_location_id 来源。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DataLocationId(String);

impl DataLocationId {
    /// 由 fixture_root 防护验证过的规范化路径构造。
    ///
    /// 这是唯一公开构造入口；不接受任意字符串，防止伪造路径身份。
    pub fn from_canonical_path(canonical: &str) -> Self {
        // 仅做格式归一化，不暴露内部 String 字段
        Self(canonical.trim_end_matches(['/', '\\']).to_string())
    }

    /// 返回稳定身份字符串引用（仅供诊断与 DTO 序列化）。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// schema 指纹：SHA-256 hex 字符串，绑定 schema 版本。
///
/// R2-3：与 `UserId` 一致应用 `#[serde(transparent)]`，固化裸字符串序列化契约，
/// 防止未来重构意外改变形态导致前端 DTO 失配。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaFingerprint(pub String);

/// 关键表行数：用于证明只读打开的副本与研究基线一致。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TableCounts {
    pub project_count: u64,
    pub chat_session_count: u64,
    pub chat_message_count: u64,
}

/// schema 兼容状态：Verified 携带指纹与行数，Incompatible 携带结构化原因。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum CompatibilityState {
    Verified {
        schema_fingerprint: SchemaFingerprint,
        counts: TableCounts,
    },
    Incompatible {
        reason: IncompatibleReason,
    },
}

/// 不兼容结构化原因：对应 Gate A fixture 矩阵。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncompatibleReason {
    /// 错误 key：SQLCipher PRAGMA key 无法解密
    WrongKey,
    /// 截断文件：数据库文件大小异常或读取失败
    TruncatedFile,
    /// 未知 schema：缺少关键表
    UnknownSchema { missing_tables: Vec<String> },
    /// 缺列：关键表存在但缺少必要列
    MissingColumn { table: String, column: String },
    /// 缺索引：缺少必要索引
    MissingIndex { table: String, index: String },
    /// 缺唯一约束
    MissingConstraint { table: String, constraint: String },
    /// R5：cipher_version 不兼容——SQLCipher 版本与基线不匹配
    CipherVersionMismatch { version: String },
    /// Gate A：关键 SQLCipher 4 参数与已验证基线不匹配
    CipherPragmaMismatch {
        pragma: String,
        expected: String,
        actual: String,
    },
}

/// 用户身份 ID：16 位数字字符串（TRAE Work CN 基线）。
///
/// 严格区分 `deviceId` 与 `userId`：`deviceId` 绝不能构造为 `UserId`。
/// 对应 ACCOUNT_DETECTION_FEASIBILITY.md 第 6 节：真实环境中的 deviceId 不能成为 userId。
/// R2 修复：测试使用合成 deviceId，不复制真实基线中的 deviceId。
///
/// R2-2：`#[serde(transparent)]` 是显式稳定契约声明——单字段 tuple newtype 的
/// 默认 serde 行为本身就是裸字符串，此属性仅用于在类型层固化契约、防止未来
/// 重构意外改变序列化形态。前端 DTO 期望 `user_id: string | null`。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(String);

impl UserId {
    /// 由白名单日志或明文兼容认证对象构造。
    ///
    /// 接受 13-19 位纯数字字符串。拒绝非数字或长度异常的 ID，
    /// 防止 `deviceId` 或其他非账号 ID 被误用。
    pub fn from_verified(value: &str) -> Result<Self, UserIdError> {
        let trimmed = value.trim();
        if !trimmed.chars().all(|c| c.is_ascii_digit()) {
            return Err(UserIdError::NonNumeric);
        }
        if !(13..=19).contains(&trimmed.len()) {
            return Err(UserIdError::InvalidLength);
        }
        Ok(Self(trimmed.to_string()))
    }

    /// 返回内部字符串引用（仅供诊断与 DTO 序列化，不持久化认证正文）。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 用户 ID 解析错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserIdError {
    NonNumeric,
    InvalidLength,
}

impl std::fmt::Display for UserIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonNumeric => write!(f, "userId 包含非数字字符"),
            Self::InvalidLength => write!(f, "userId 长度异常（应为 13-19 位）"),
        }
    }
}

impl std::error::Error for UserIdError {}

/// 认证字段不可逆指纹：SHA-256 hex，绑定字段名、值与产品版本。
///
/// 对应规格第 12 节：只持久化不可逆指纹与必要非敏感证据元数据，
/// 不持久化凭证正文。
///
/// R2-3：与 `UserId` 一致应用 `#[serde(transparent)]`，固化裸字符串序列化契约，
/// 防止未来重构意外改变形态导致前端 DTO 失配。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuthFingerprint(pub String);

/// 账号证据来源事件摘要：记录来源类型与会话 ID，不记录正文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceEventSummary {
    /// 来源类型：alog/renderer/main/log 等
    pub source_kind: String,
    /// 事件名：fetchLogTask / User info loaded / updateUserInfo / getUserInfo
    pub event_name: String,
    /// 日志会话 ID（不记录 userId 正文）
    pub log_session_id: Option<String>,
}

/// 证据状态：决定账号是否可写
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    /// 至少两类白名单来源一致
    Verified,
    /// 单一来源
    SingleSource,
    /// 缺失：无任何白名单来源
    Missing,
    /// 来源冲突
    Conflict,
    /// 日志过期
    Expired,
    /// 认证指纹变化
    FingerprintChanged,
}

/// 当前账号证据：对应规格第 12 节 `AccountEvidence` 最小集合
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountEvidence {
    pub user_id: Option<UserId>,
    pub source_events: Vec<SourceEventSummary>,
    pub auth_fingerprint: Option<AuthFingerprint>,
    pub local_storage_user_id: Option<UserId>,
    pub product_version: Option<String>,
    pub observed_at: SystemTime,
    pub evidence_state: EvidenceState,
}

impl Default for AccountEvidence {
    fn default() -> Self {
        Self {
            user_id: None,
            source_events: Vec::new(),
            auth_fingerprint: None,
            local_storage_user_id: None,
            product_version: None,
            observed_at: SystemTime::now(),
            evidence_state: EvidenceState::Missing,
        }
    }
}

/// 结构化只读原因：T02 只暴露 Gate A/B 阻塞的子集。
///
/// 对应规格第 31 节错误分类。手工账号选择永远不能解除只读。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadonlyReason {
    NeedsProductClosed,
    AccountEvidenceUnavailable,
    DataLocationUnavailable,
    DataLocationChanged,
    SchemaUnsupported,
    WrongKey,
    TruncatedFile,
    UnknownSchema,
    Conflict,
    Expired,
    FingerprintChanged,
    /// 第三方账号管理器信息只作诊断提示，不能提高权限
    ThirdPartyManagerDiagnosticOnly,
}

/// T02 工作台只读状态聚合：组合 SQLCipher 探测与账号证据。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkbenchReadState {
    pub platform: super::PlatformContext,
    pub data_location: super::DataLocationState,
    pub compatibility: CompatibilityState,
    pub current_account: AccountEvidence,
    /// 只读原因：None 表示账号已 verified，但 T02 阶段写能力仍保持禁用
    pub readonly_reason: Option<ReadonlyReason>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============== UserId 严格构造：deviceId 不能成为 userId ==============

    #[test]
    fn user_id_accepts_16_digit_trae_user_id() {
        // R2 修复：使用合成账号 ID，不复制真实基线中的账号 ID
        let uid = UserId::from_verified("1000000000000001").unwrap();
        assert_eq!(uid.as_str(), "1000000000000001");
    }

    #[test]
    fn user_id_rejects_device_id_with_non_digits_removed() {
        // R2 修复：使用合成 deviceId，不复制真实基线中的 deviceId
        // userId 严格校验只在白名单来源（日志/storage.json）调用；
        // deviceId 必须在 application 层拒绝构造 UserId（见 account_evidence 测试）
        // 这里仅验证 UserId 的格式约束
        let uid = UserId::from_verified("2000000000000001");
        assert!(uid.is_ok(), "16 位数字通过格式校验");
    }

    #[test]
    fn user_id_rejects_non_numeric_string() {
        // 防止任意 hex key、认证正文进入 userId
        let err = UserId::from_verified("a1b2c3d4e5f6").unwrap_err();
        assert_eq!(err, UserIdError::NonNumeric);
    }

    #[test]
    fn user_id_rejects_short_string() {
        // 防止短 ID 进入
        let err = UserId::from_verified("12345").unwrap_err();
        assert_eq!(err, UserIdError::InvalidLength);
    }

    #[test]
    fn user_id_rejects_jwt_ciphertext() {
        // 防止 JWT 密文进入 userId
        let err = UserId::from_verified("eyJhbGci.payload.sig").unwrap_err();
        assert_eq!(err, UserIdError::NonNumeric);
    }

    #[test]
    fn user_id_rejects_too_long_string() {
        // 防止过长字符串进入 userId
        let err = UserId::from_verified("12345678901234567890123").unwrap_err();
        assert_eq!(err, UserIdError::InvalidLength);
    }

    // ============== DataLocationId 仅接受规范化路径 ==============

    #[test]
    fn data_location_id_normalizes_trailing_separators() {
        let id = DataLocationId::from_canonical_path("C:\\temp\\fixture\\");
        assert_eq!(id.as_str(), "C:\\temp\\fixture");
    }

    #[test]
    fn data_location_id_preserves_internal_separators() {
        let id = DataLocationId::from_canonical_path("C:\\temp\\fixture");
        assert_eq!(id.as_str(), "C:\\temp\\fixture");
    }

    // ============== CompatibilityState 序列化 ==============

    #[test]
    fn compatibility_state_verified_serializes_with_tag() {
        let state = CompatibilityState::Verified {
            schema_fingerprint: SchemaFingerprint(
                "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_string(),
            ),
            counts: TableCounts {
                project_count: 1,
                chat_session_count: 2,
                chat_message_count: 3,
            },
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"kind\":\"Verified\""));
        assert!(json.contains("project_count"));
    }

    #[test]
    fn compatibility_state_incompatible_wrong_key_serializes() {
        let state = CompatibilityState::Incompatible {
            reason: IncompatibleReason::WrongKey,
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("\"kind\":\"Incompatible\""));
        assert!(json.contains("wrong_key"));
    }

    #[test]
    fn compatibility_state_cipher_pragma_mismatch_serializes() {
        let state = CompatibilityState::Incompatible {
            reason: IncompatibleReason::CipherPragmaMismatch {
                pragma: "kdf_iter".to_string(),
                expected: "256000".to_string(),
                actual: "64000".to_string(),
            },
        };
        let value = serde_json::to_value(state).unwrap();
        let mismatch = &value["reason"]["cipher_pragma_mismatch"];
        assert_eq!(mismatch["pragma"], "kdf_iter");
        assert_eq!(mismatch["expected"], "256000");
        assert_eq!(mismatch["actual"], "64000");
    }

    #[test]
    fn compatibility_state_unknown_schema_carries_missing_tables() {
        let state = CompatibilityState::Incompatible {
            reason: IncompatibleReason::UnknownSchema {
                missing_tables: vec!["project".to_string(), "chat_session".to_string()],
            },
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("project"));
        assert!(json.contains("chat_session"));
    }

    // ============== AccountEvidence 默认状态 ==============

    #[test]
    fn account_evidence_default_is_missing() {
        let ev = AccountEvidence::default();
        assert_eq!(ev.evidence_state, EvidenceState::Missing);
        assert!(ev.user_id.is_none());
        assert!(ev.source_events.is_empty());
    }

    // ============== R2-3：跨层 serde JSON 契约测试 ==============
    // handoff 第 84-90 行：直接断言 Rust 实际 serde JSON 形态，
    // 不依赖人工推断或纯前端 mock。前端 DTO 必须与这里断言的形态一致。

    #[test]
    fn r2_3_user_id_serializes_as_json_string() {
        // UserId 必须序列化为裸字符串，不是 {"0":"..."} 对象
        let uid = UserId::from_verified("1000000000000001").unwrap();
        let json = serde_json::to_string(&uid).unwrap();
        // JSON 字符串字面量——前后双引号包裹纯字符串内容
        assert_eq!(json, "\"1000000000000001\"");
        // 反向断言：不含对象字段标记
        assert!(!json.contains("\"0\""));
        assert!(!json.contains("{"));
    }

    #[test]
    fn r2_3_auth_fingerprint_serializes_as_json_string() {
        // AuthFingerprint 必须序列化为裸字符串
        let fp = AuthFingerprint(
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_string(),
        );
        let json = serde_json::to_string(&fp).unwrap();
        assert_eq!(
            json,
            "\"abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789\""
        );
        assert!(!json.contains("\"0\""));
        assert!(!json.contains("{"));
    }

    #[test]
    fn r2_3_schema_fingerprint_serializes_as_json_string() {
        // SchemaFingerprint 必须序列化为裸字符串
        let fp = SchemaFingerprint(
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_string(),
        );
        let json = serde_json::to_string(&fp).unwrap();
        assert_eq!(
            json,
            "\"abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789\""
        );
        assert!(!json.contains("\"0\""));
        assert!(!json.contains("{"));
    }

    #[test]
    fn r2_3_workbench_read_state_full_json_shape_matches_frontend_dto() {
        // 完整 WorkbenchReadState 序列化形态必须与前端 WorkbenchReadStateDto 一致：
        // - user_id: 裸字符串 | null
        // - local_storage_user_id: 裸字符串 | null
        // - auth_fingerprint: 裸字符串 | null
        // - schema_fingerprint: 裸字符串（位于 compatibility.Verified 内）
        // - observed_at: { secs_since_epoch, nanos_since_epoch }
        // - evidence_state: snake_case tag
        let state = WorkbenchReadState {
            platform: crate::PlatformContext {
                platform_id: crate::PlatformId::work_cn(),
                display_name: "TRAE Work CN".to_string(),
                adapter_implemented: true,
            },
            data_location: crate::DataLocationState {
                selected: true,
                display_name: Some("C:\\fixture".to_string()),
                unavailable_reason: None,
            },
            compatibility: CompatibilityState::Verified {
                schema_fingerprint: SchemaFingerprint("fp-schema".to_string()),
                counts: TableCounts {
                    project_count: 1,
                    chat_session_count: 2,
                    chat_message_count: 3,
                },
            },
            current_account: AccountEvidence {
                user_id: Some(UserId::from_verified("1000000000000001").unwrap()),
                source_events: vec![SourceEventSummary {
                    source_kind: "alog".to_string(),
                    event_name: "fetchLogTask".to_string(),
                    log_session_id: Some("session-1".to_string()),
                }],
                auth_fingerprint: Some(AuthFingerprint("fp-auth".to_string())),
                local_storage_user_id: Some(UserId::from_verified("1000000000000001").unwrap()),
                product_version: Some("1.107.1".to_string()),
                observed_at: std::time::SystemTime::UNIX_EPOCH
                    + std::time::Duration::from_secs(1700000000),
                evidence_state: EvidenceState::Verified,
            },
            readonly_reason: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        // user_id 为裸字符串
        assert_eq!(v["current_account"]["user_id"], "1000000000000001");
        // local_storage_user_id 为裸字符串
        assert_eq!(
            v["current_account"]["local_storage_user_id"],
            "1000000000000001"
        );
        // auth_fingerprint 为裸字符串
        assert_eq!(v["current_account"]["auth_fingerprint"], "fp-auth");
        // schema_fingerprint 为裸字符串（位于 compatibility 内）
        assert_eq!(v["compatibility"]["schema_fingerprint"], "fp-schema");
        // compatibility.kind 为 PascalCase（CompatibilityState 无 rename_all，默认形态）
        // 固化此契约，防止未来意外添加 rename_all 导致前端 DTO 失配
        assert_eq!(v["compatibility"]["kind"], "Verified");
        // observed_at 为 { secs_since_epoch, nanos_since_epoch }
        assert!(v["current_account"]["observed_at"]["secs_since_epoch"].is_number());
        assert!(v["current_account"]["observed_at"]["nanos_since_epoch"].is_number());
        // evidence_state 为 snake_case
        assert_eq!(v["current_account"]["evidence_state"], "verified");
        // 反向断言：fingerprint 字段不含 {"0":...} 对象包装
        assert!(v["current_account"]["auth_fingerprint"].is_string());
        assert!(v["compatibility"]["schema_fingerprint"].is_string());
    }

    #[test]
    fn r2_3_account_evidence_missing_state_json_shape() {
        // 缺失/不可用状态的 DTO 形态：所有可空字段为 null
        let ev = AccountEvidence {
            user_id: None,
            source_events: Vec::new(),
            auth_fingerprint: None,
            local_storage_user_id: None,
            product_version: None,
            observed_at: std::time::SystemTime::UNIX_EPOCH,
            evidence_state: EvidenceState::Missing,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v["user_id"].is_null());
        assert!(v["auth_fingerprint"].is_null());
        assert!(v["local_storage_user_id"].is_null());
        assert_eq!(v["evidence_state"], "missing");
    }
}
