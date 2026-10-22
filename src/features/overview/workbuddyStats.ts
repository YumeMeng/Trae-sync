import type { OverviewSnapshot } from "../../platform/overview";
import {
  EMPTY_VIEW,
  formatCredits,
  type OverviewAction,
  type OverviewStat,
  type OverviewStatGroup,
  type OverviewView,
} from "./stats";

/**
 * WorkBuddy 总览的数据面配置：签到与账号（4 指标）+ 对话库（3 指标）。
 *
 * 积分沿用 P19.2③ 双行语义：credits_balance 是余额（随消耗减少），
 * total_credits 是本期签到累计（只增不减）；两行各自独立占位化——
 * 全部未获取 → 占位 + 刷新入口，部分获取 → 合计 known 账号并注明 N/M
 * （假 0 纪律：读不到就写读不到，禁止静默 0）。
 */
export function workbuddyView(snapshot: OverviewSnapshot): OverviewView {
  if (snapshot.kind !== "workbuddy") return EMPTY_VIEW;
  const { participation, checkinRows, libraryStats } = snapshot.workbuddy;
  const accountCount = participation?.length ?? 0;
  const knownRows = checkinRows?.filter((row) => row.error === null) ?? [];
  const checkedCount = knownRows.filter((row) => row.today_checked_in === true).length;

  const checkinGroup: OverviewStatGroup | null =
    participation === null || checkinRows === null
      ? null
      : {
          testId: "wb-overview-checkin",
          title: "签到与账号",
          stripClass: "overview-checkin__stats",
          stats: [
            { key: "accounts", label: "账号", value: String(accountCount) },
            { key: "checked", label: "今日签到", value: `${checkedCount}/${accountCount}` },
            balanceStat(knownRows, accountCount),
            accumStat(knownRows, accountCount),
          ],
        };

  const libraryGroup: OverviewStatGroup | null =
    libraryStats === null
      ? null
      : {
          testId: "wb-overview-stats-library",
          title: "对话库",
          stripClass: "overview-page__stats",
          stats: [
            { key: "sessions", label: "库内会话", value: String(libraryStats.session_count) },
            { key: "projects", label: "项目", value: String(libraryStats.project_count) },
            { key: "accounts", label: "库内账号", value: String(libraryStats.account_count) },
          ],
        };

  const pending: OverviewAction | null =
    checkedCount < accountCount
      ? {
          label: `去签到（余 ${accountCount - checkedCount}）`,
          page: "checkin",
          testId: "wb-overview-checkin-cta",
        }
      : null;

  return {
    ...EMPTY_VIEW,
    groups: [checkinGroup, libraryGroup].filter(
      (group): group is OverviewStatGroup => group !== null,
    ),
    pending,
    empty:
      libraryStats === null
        ? { title: "对话库暂不可读取。", icon: "database", testId: "wb-overview-library-hint" }
        : null,
    primary: { label: "查看对话库", page: "master-library", testId: "wb-overview-library-cta" },
    secondary: { label: "管理账号", page: "accounts", testId: "wb-overview-accounts-cta" },
    refresh: balanceStat(knownRows, accountCount).tone === "warn"
      ? { label: "点击刷新", testId: "wb-overview-credits-refresh" }
      : null,
  };
}

/** 余额指标：全部未获取 → 占位（warn）；部分获取 → 合计 + N/M 注。 */
function balanceStat(
  rows: readonly { readonly credits_balance: number | null }[],
  accountCount: number,
): OverviewStat {
  const known = rows.filter((row) => row.credits_balance !== null);
  const total = known.reduce((sum, row) => sum + (row.credits_balance ?? 0), 0);
  if (accountCount > 0 && known.length === 0) {
    return {
      key: "balance",
      label: "积分余额",
      value: "余额查询失败",
      note: `${accountCount} 个账号均未获取到余额`,
      tone: "warn",
      testId: "wb-overview-credits-unknown",
    };
  }
  return {
    key: "balance",
    label: "积分余额",
    value: accountCount > 0 ? formatCredits(total) : "—",
    note: known.length > 0 && known.length < accountCount
      ? `${known.length}/${accountCount} 账号已获取`
      : undefined,
    hint: "各账号积分余额合计（随对话消耗减少）",
  };
}

/** 本期签到累计指标：全部未获取 → 占位（warn）；部分获取 → 合计 + N/M 注。 */
function accumStat(
  rows: readonly { readonly total_credits: number | null }[],
  accountCount: number,
): OverviewStat {
  const known = rows.filter((row) => row.total_credits !== null);
  const total = known.reduce((sum, row) => sum + (row.total_credits ?? 0), 0);
  if (accountCount > 0 && known.length === 0) {
    return {
      key: "accum",
      label: "本期签到累计",
      value: "未获取",
      tone: "warn",
      testId: "wb-overview-accum-unknown",
    };
  }
  return {
    key: "accum",
    label: "本期签到累计",
    value: accountCount > 0 ? formatCredits(total) : "—",
    note: known.length > 0 && known.length < accountCount
      ? `${known.length}/${accountCount} 账号已获取`
      : undefined,
    hint: "本期签到活动累计获得（只增不减，非余额）",
  };
}