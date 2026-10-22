import type { OverviewSnapshot } from "../../platform/overview";
import type { WorkspaceStateDto } from "../../types/workspace";
import {
  ACCOUNTS_ACTION,
  EMPTY_VIEW,
  formatLastActive,
  type OverviewAction,
  type OverviewEvidence,
  type OverviewStatGroup,
  type OverviewView,
} from "./stats";

/**
 * TRAE 总览的数据面配置：主库统计优先，主库不可读时回落旧历史统计
 * （G1 三分支：有数据 / 无数据 + 目录就绪 / 目录缺失）。
 * 本文件只做"快照 → 视图模型"的纯映射，不读数据、不碰 DOM。
 */
export function traeView(
  snapshot: OverviewSnapshot,
  state: WorkspaceStateDto | undefined,
): OverviewView {
  // 工作台状态是 TRAE 数据面的前提（WB 槽位不传）；缺状态按无数据处理。
  if (snapshot.kind !== "trae" || state === undefined) return EMPTY_VIEW;
  const { checkinRows, environment, masterStats } = snapshot.trae;
  const masterReady = masterStats !== null && masterStats.status === "ready";
  const masterHasData =
    masterReady &&
    masterStats !== null &&
    (masterStats.session_count > 0 ||
      masterStats.project_count > 0 ||
      masterStats.participating_account_count > 0);
  const historyHasData =
    state.history.account_count + state.history.project_count + state.history.session_count > 0;
  const hasData = masterReady ? masterHasData : historyHasData;

  const lastActive = masterStats?.last_active_unix_seconds;
  const evidence: OverviewEvidence = {
    accountName: environment?.current_account_name ?? "未登录",
    lastActive:
      lastActive === null || lastActive === undefined ? null : formatLastActive(lastActive),
  };

  // 待处理提示：真实签到能力开启且确有未签账号时才出（一条）。
  const unchecked =
    checkinRows === null ? 0 : checkinRows.filter((row) => row.checked_in !== true).length;
  const pending: OverviewAction | null =
    unchecked > 0
      ? { label: `去签到（余 ${unchecked}）`, page: "checkin", testId: "overview-checkin-cta" }
      : null;

  // 目录缺失 = 真实异常态：只留一句结论，不出统计与主行动（G1）。
  if (!state.data_location.selected) {
    return {
      ...EMPTY_VIEW,
      evidence,
      empty: {
        title: "未找到 TRAE 数据目录，请确认 TRAE 已安装。",
        icon: "database",
        testId: "overview-data-missing",
      },
    };
  }

  if (!hasData) {
    return {
      ...EMPTY_VIEW,
      evidence,
      pending,
      empty: { title: "主库就绪，暂无对话。", icon: "book", testId: "overview-empty-hint" },
      primary: { label: "去环境页查看主库", page: "environment", testId: "overview-master-cta" },
      secondary: ACCOUNTS_ACTION,
    };
  }

  const groups: readonly OverviewStatGroup[] =
    masterReady && masterStats !== null
      ? [
          {
            testId: "overview-stats-master",
            stripClass: "overview-page__stats",
            stats: [
              { key: "sessions", label: "主库对话", value: String(masterStats.session_count) },
              { key: "projects", label: "主库项目", value: String(masterStats.project_count) },
              {
                key: "accounts",
                label: "参与账号",
                value: String(masterStats.participating_account_count),
                hint: "主库内出现过的账号数（含历史归属）",
              },
            ],
          },
        ]
      : [
          {
            testId: "overview-stats",
            stripClass: "overview-page__stats",
            stats: [
              { key: "accounts", label: "账号", value: String(state.history.account_count) },
              { key: "projects", label: "项目", value: String(state.history.project_count) },
              { key: "sessions", label: "对话", value: String(state.history.session_count) },
            ],
          },
        ];

  return {
    ...EMPTY_VIEW,
    groups,
    evidence,
    pending,
    primary: { label: "查看主库记录", page: "master-library", testId: "overview-scan-cta" },
    secondary: ACCOUNTS_ACTION,
  };
}