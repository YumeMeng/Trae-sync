// T02 Work CN 只读工作台状态 DTO：与 Rust 后端 `read_work_cn_state` 命令返回的
// `WorkbenchReadState` 结构保持一致。
//
// 安全约束（handoff 第 89-95 行）：
// - 前端只接收结构化 DTO，不接收 raw_key、认证正文或底层错误原文
// - userId 只用于判断是否检测到账号，不显示真实 ID 正文
// - 错误状态只展示结构化 kind，不展示 secret

/** 平台上下文（与 T01 WorkspaceStateDto.platform 一致） */
export interface WorkbenchPlatformDto {
  readonly platform_id: string;
  readonly display_name: string;
  readonly adapter_implemented: boolean;
}

/** 数据位置状态（与 T01 一致） */
export interface WorkbenchDataLocationDto {
  readonly selected: boolean;
  readonly display_name: string | null;
  readonly unavailable_reason: string | null;
}

/** schema 兼容状态：tag "kind" 区分 Verified / Incompatible */
export interface CompatibilityVerifiedDto {
  readonly kind: "Verified";
  // R2-2：Rust `SchemaFingerprint(pub String)` 默认序列化为裸字符串（newtype），
  // 不是 `{"0":"..."}` 对象。DTO 与 Rust serde 实际形态对齐。
  readonly schema_fingerprint: string;
  readonly counts: {
    readonly project_count: number;
    readonly chat_session_count: number;
    readonly chat_message_count: number;
  };
}

export interface CompatibilityIncompatibleDto {
  readonly kind: "Incompatible";
  readonly reason:
    | "wrong_key"
    | "truncated_file"
    | { readonly unknown_schema: { readonly missing_tables: string[] } }
    | { readonly missing_column: { readonly table: string; readonly column: string } }
    | { readonly missing_index: { readonly table: string; readonly index: string } }
    | { readonly missing_constraint: { readonly table: string; readonly constraint: string } }
    | { readonly cipher_version_mismatch: { readonly version: string } };
}

export type CompatibilityStateDto =
  | CompatibilityVerifiedDto
  | CompatibilityIncompatibleDto;

/** 账号证据：前端只读，不展示 user_id / auth_fingerprint 正文 */
export interface AccountEvidenceDto {
  readonly user_id: string | null;
  readonly source_events: ReadonlyArray<{
    readonly source_kind: string;
    readonly event_name: string;
    readonly log_session_id: string | null;
  }>;
  // R2-2：Rust `AuthFingerprint(pub String)` 默认序列化为裸字符串（newtype），
  // 不是 `{"0":"..."}` 对象。DTO 与 Rust serde 实际形态对齐。
  readonly auth_fingerprint: string | null;
  readonly local_storage_user_id: string | null;
  readonly product_version: string | null;
  readonly observed_at: { readonly secs_since_epoch: number; readonly nanos_since_epoch: number };
  // R2-3：Rust `EvidenceState` 标注 `#[serde(rename_all = "snake_case")]`，
  // `Verified` 序列化为 `"verified"`、`FingerprintChanged` → `"fingerprint_changed"` 等。
  // DTO 与 Rust serde 实际形态对齐，避免前端 switch 永不匹配。
  readonly evidence_state:
    | "verified"
    | "single_source"
    | "missing"
    | "conflict"
    | "expired"
    | "fingerprint_changed";
}

/** 结构化只读原因（snake_case） */
export type ReadonlyReasonDto =
  | "needs_product_closed"
  | "account_evidence_unavailable"
  | "data_location_unavailable"
  | "data_location_changed"
  | "schema_unsupported"
  | "wrong_key"
  | "truncated_file"
  | "unknown_schema"
  | "conflict"
  | "expired"
  | "fingerprint_changed"
  | "third_party_manager_diagnostic_only";

/** T02 工作台只读状态聚合 */
export interface WorkbenchReadStateDto {
  readonly platform: WorkbenchPlatformDto;
  readonly data_location: WorkbenchDataLocationDto;
  readonly compatibility: CompatibilityStateDto;
  readonly current_account: AccountEvidenceDto;
  readonly readonly_reason: ReadonlyReasonDto | null;
}
