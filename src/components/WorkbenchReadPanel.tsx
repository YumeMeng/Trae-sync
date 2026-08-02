import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { AccountEvidenceDto, WorkbenchReadStateDto } from "../types/workbench_read";

// T02 Work CN 只读入口面板：
// - 初始不自动扫描（切片 C AC1）
// - 用户输入 fixture_root 与 db_relative_path 后点击"读取"才调用后端
// - 显示平台、数据位置、兼容状态、当前账号/只读原因（切片 C AC2）
// - 错误状态只显示结构化 kind，不展示 raw_key/认证正文（切片 C AC3）
// - 所有真实写能力仍禁用，手工账号选择不能解除只读（切片 C AC4）
export function WorkbenchReadPanel() {
  const [fixtureRoot, setFixtureRoot] = useState("");
  const [dbRelativePath, setDbRelativePath] = useState("database.db");
  const [state, setState] = useState<WorkbenchReadStateDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  // 用户点击"读取"才调用后端——不自动扫描
  async function handleRead() {
    setLoading(true);
    setError(null);
    try {
      const result = await invoke<WorkbenchReadStateDto>("read_work_cn_state", {
        fixtureRoot,
        dbRelativePath,
      });
      setState(result);
    } catch (e) {
      setError(String(e));
      setState(null);
    } finally {
      setLoading(false);
    }
  }

  return (
    <section className="workbench-read" role="region" aria-label="Work CN 只读入口">
      <h2>Work CN 只读入口（fixture 模式）</h2>
      <p className="workbench-read__hint" data-testid="workbench-read-hint">
        T02 阶段仅支持 fixture 路径，真实 TRAE 数据位置由 T03+ 启用。
      </p>
      <div className="workbench-read__form">
        <label className="workbench-read__field">
          fixture 路径
          <input
            type="text"
            value={fixtureRoot}
            onChange={(e) => setFixtureRoot(e.target.value)}
            placeholder="例如：%LOCALAPPDATA%\Trae Sync\tests\fixture-xxx"
            data-testid="fixture-root-input"
          />
        </label>
        <label className="workbench-read__field">
          数据库相对路径
          <input
            type="text"
            value={dbRelativePath}
            onChange={(e) => setDbRelativePath(e.target.value)}
            placeholder="database.db"
            data-testid="db-relative-path-input"
          />
        </label>
        <button
          type="button"
          onClick={handleRead}
          disabled={loading || fixtureRoot.length === 0}
          data-testid="read-workbench-button"
        >
          {loading ? "读取中…" : "读取 Work CN 状态"}
        </button>
      </div>

      {error && (
        <div className="workbench-read__error" role="alert" data-testid="workbench-read-error">
          读取失败：{error}
        </div>
      )}

      {state && <WorkbenchReadStateView state={state} />}

      <p className="workbench-read__readonly-notice" data-testid="readonly-notice">
        所有同步、写入、备份恢复能力仍保持禁用；手工账号选择不能解除只读。
      </p>
    </section>
  );
}

// 只读状态视图：渲染关键字段，不展示 raw_key / 认证正文
function WorkbenchReadStateView({ state }: { state: WorkbenchReadStateDto }) {
  const compatibilityKind = state.compatibility.kind;
  // R6：显示明确账号标识——user_id 摘要（首尾 4 位）或结构化不可用原因
  const accountLabel = renderAccountLabel(state.current_account);
  // 不展示 user_id / auth_fingerprint 正文，只显示 evidence_state
  const evidenceState = state.current_account.evidence_state;

  return (
    <div className="workbench-read__state" data-testid="workbench-read-state">
      <div data-testid="wr-platform">
        平台：{state.platform.display_name}
        {state.platform.adapter_implemented
          ? "（Adapter 已实现只读探测）"
          : "（边界保留）"}
      </div>
      <div data-testid="wr-data-location">
        数据位置：{state.data_location.display_name ?? "未选择"}
      </div>
      <div data-testid="wr-compatibility">兼容状态：{compatibilityKind}</div>
      {compatibilityKind === "Incompatible" && (
        <div data-testid="wr-incompatible-reason">
          不兼容原因：
          {renderIncompatibleReason(state.compatibility.reason)}
        </div>
      )}
      <div data-testid="wr-account">
        当前账号：{accountLabel}
        （证据状态：{evidenceState}）
      </div>
      <div data-testid="wr-readonly-reason">
        只读原因：{state.readonly_reason ?? "无（账号已 verified，但 T02 写能力仍禁用）"}
      </div>
    </div>
  );
}

/// R6：渲染账号明确标识或结构化不可用原因。
///
/// - user_id 存在：显示首尾 4 位摘要（如 `1000…0001`），不暴露完整 ID
/// - user_id 为空：显示结构化不可用原因（基于 evidence_state），而非模糊的“已检测/未检测”
///
/// 这种非歧义标识使用户能区分不同账号与不同证据问题，满足 T02 AC。
function renderAccountLabel(account: AccountEvidenceDto): string {
  if (account.user_id !== null && account.user_id.length >= 8) {
    // 显示首尾 4 位，中间用省略号——足够识别同一账号多次出现，不暴露完整 ID
    const head = account.user_id.slice(0, 4);
    const tail = account.user_id.slice(-4);
    return `账号 ${head}…${tail}`;
  }
  // user_id 缺失：按 evidence_state 给出结构化不可用原因
  // R2-3：evidence_state 为 snake_case（与 Rust serde rename_all 对齐）
  switch (account.evidence_state) {
    case "missing":
      return "账号证据缺失（无白名单来源）";
    case "conflict":
      return "账号证据冲突（来源不一致）";
    case "single_source":
      return "账号证据单来源（未达到 verified）";
    case "expired":
      return "账号证据过期（会话过老）";
    case "fingerprint_changed":
      return "账号指纹漂移（认证字段变化）";
    case "verified":
      // evidence_state = verified 但 user_id 为 null——不应发生，保守显示
      return "账号已验证但 user_id 缺失";
    default:
      return "账号证据未知状态";
  }
}

// 渲染不兼容原因：只显示结构化 kind，不显示底层错误原文
function renderIncompatibleReason(
  reason:
    | "wrong_key"
    | "truncated_file"
    | { readonly unknown_schema: { readonly missing_tables: string[] } }
    | { readonly missing_column: { readonly table: string; readonly column: string } }
    | { readonly missing_index: { readonly table: string; readonly index: string } }
    | { readonly missing_constraint: { readonly table: string; readonly constraint: string } }
    | { readonly cipher_version_mismatch: { readonly version: string } }
): string {
  if (typeof reason === "string") {
    return reason;
  }
  if ("unknown_schema" in reason) {
    return `unknown_schema（缺少表：${reason.unknown_schema.missing_tables.join(", ")}）`;
  }
  if ("missing_column" in reason) {
    return `missing_column（${reason.missing_column.table}.${reason.missing_column.column}）`;
  }
  if ("missing_index" in reason) {
    return `missing_index（${reason.missing_index.table}.${reason.missing_index.index}）`;
  }
  if ("missing_constraint" in reason) {
    return `missing_constraint（${reason.missing_constraint.table}.${reason.missing_constraint.constraint}）`;
  }
  if ("cipher_version_mismatch" in reason) {
    return `cipher_version_mismatch（版本：${reason.cipher_version_mismatch.version}）`;
  }
  return "未知";
}
