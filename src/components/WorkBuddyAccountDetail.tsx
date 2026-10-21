import { useCallback, useEffect, useState } from "react";
import { ArrowLeft, CalendarClock, Coins, Cpu, KeyRound } from "lucide-react";
import type {
  WorkBuddyCheckinAccountStatusDto,
  WorkBuddyLoginSummaryDto,
} from "../types/product";
import {
  fetchWorkBuddyCheckinOverview,
  listWorkBuddyCheckinAccounts,
  setWorkBuddyAccountAutoCheckin,
} from "../platform/workbuddyAdapter";
import { WorkBuddyPoolSlotBadge, deriveWorkBuddyPoolLoginState } from "./StatusBadges";
import { notify } from "../platform/notify";
import { safeUiErrorMessage } from "../utils/safeUiError";

interface WorkBuddyAccountDetailProps {
  /** 详情目标账号（账号池清单条目）。 */
  account: WorkBuddyLoginSummaryDto;
  /** 是否为当前客户端登录账号（健康卡 uid 与池 account_id 同源）。 */
  isCurrentLogin: boolean;
  /** 数据变更（参与开关翻转）后通知列表页重读。 */
  onDataChanged: () => Promise<void>;
  /** 返回账号列表。 */
  onBack: () => void;
}

/**
 * WorkBuddy 账号详情（P19.1 详情实装，对照 AdapterAccountDetail 轻量结构）：
 * 基础信息 + 凭据健康度 + 签到与积分 + 自动签到参与开关 + 技术细节折叠。
 *
 * 数据源纪律：只用既有命令面（池清单 / checkin overview / 参与集命令）；
 * 备注名编辑、归档/删除与手机号在 WB 命令面与 DTO 中均无对应实现
 * （IA 草案 §1.2 判定「移植」待后端），区块诚实标注缺席三态，不发明数据。
 */
export function WorkBuddyAccountDetail({
  account,
  isCurrentLogin,
  onDataChanged,
  onBack,
}: WorkBuddyAccountDetailProps) {
  const [overview, setOverview] = useState<WorkBuddyCheckinAccountStatusDto | null>(null);
  const [participating, setParticipating] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    // 积分/签到状态（overview 已有 total_credits/streak）；参与集 = 有效参与。
    void fetchWorkBuddyCheckinOverview()
      .then((rows) => {
        if (!cancelled) {
          setOverview(rows.find((row) => row.profile_id === account.profile_id) ?? null);
        }
      })
      .catch(() => undefined); // fail-soft：积分区显示「未读取」
    void listWorkBuddyCheckinAccounts()
      .then((rows) => {
        if (!cancelled) {
          const row = rows.find((item) => item.profile_id === account.profile_id);
          setParticipating(row ? row.enabled : null);
        }
      })
      .catch(() => undefined); // fail-soft：参与行显示「未读取」
    return () => { cancelled = true; };
  }, [account.profile_id]);

  // 参与开关翻转：显式写后端 touched+enabled（opt-out 永久尊重，B5 契约）。
  const handleToggleParticipation = useCallback(() => {
    if (busy || participating === null) return;
    setBusy(true);
    void setWorkBuddyAccountAutoCheckin(account.profile_id, !participating)
      .then(async () => {
        const rows = await listWorkBuddyCheckinAccounts();
        const row = rows.find((item) => item.profile_id === account.profile_id);
        setParticipating(row ? row.enabled : null);
        notify.success(!participating
          ? "WorkBuddy：已加入每日自动签到。"
          : "WorkBuddy：已退出每日自动签到（仍可手动签到）。");
        await onDataChanged();
      })
      .catch((reason: unknown) => notify.error(
        `WorkBuddy：参与状态保存失败——${safeUiErrorMessage(reason, "请稍后重试。")}`,
      ))
      .finally(() => setBusy(false));
  }, [account.profile_id, busy, onDataChanged, participating]);

  const poolState = deriveWorkBuddyPoolLoginState({
    access_expires_at_unix_seconds: account.access_token_expires_at_unix_seconds,
    refresh_expires_at_unix_seconds: account.refresh_token_expires_at_unix_seconds,
    now_unix_seconds: Math.floor(Date.now() / 1000),
  });
  const displayName = account.screen_name || "WorkBuddy 账号";

  return (
    <section className="account-detail" role="region" aria-label="账号详情" data-testid="workbuddy-account-detail">
      <header className="page-header">
        <div className="page-header__copy">
          <button className="btn btn--quiet" type="button" onClick={onBack} data-testid="workbuddy-account-detail-back">
            <ArrowLeft size={15} aria-hidden="true" />返回账号列表
          </button>
          <h1 data-page-title="accounts" tabIndex={-1}>{displayName}</h1>
        </div>
      </header>

      {/* 基础信息：凭据状态、当前登录标记、账号域、最近更新（全部来自池清单 DTO）。 */}
      <section className="account-detail__section" aria-labelledby="workbuddy-detail-basic">
        <h2 id="workbuddy-detail-basic"><Coins size={16} aria-hidden="true" />基础信息</h2>
        <dl className="account-detail__facts">
          <div className="account-detail__fact">
            <dt>登录状态</dt>
            <dd data-testid="workbuddy-detail-status">
              <WorkBuddyPoolSlotBadge state={poolState} />
            </dd>
          </div>
          <div className="account-detail__fact">
            <dt>当前账号</dt>
            <dd data-testid="workbuddy-detail-current">
              {isCurrentLogin ? (
                <span className="account-item__current-chip" title="WorkBuddy 客户端当前登录的账号">
                  使用中
                </span>
              ) : (
                "不是当前账号"
              )}
            </dd>
          </div>
          <div className="account-detail__fact">
            <dt>最近更新</dt>
            <dd data-testid="workbuddy-detail-updated">{formatUnixTime(account.last_updated_at_unix_seconds)}</dd>
          </div>
        </dl>
      </section>

      {/* 凭据健康度：access/refresh 到期时间轴（池 DTO 有数据源，展示剩余天数与时刻；
          时间戳为 0 = 未采集，诚实标注未知）。 */}
      <section className="account-detail__section" aria-labelledby="workbuddy-detail-credential">
        <h2 id="workbuddy-detail-credential"><KeyRound size={16} aria-hidden="true" />凭据健康度</h2>
        <dl className="account-detail__facts">
          <div className="account-detail__fact">
            <dt>访问凭据（access）</dt>
            <dd>{tokenExpiry(account.access_token_expires_at_unix_seconds)}</dd>
          </div>
          <div className="account-detail__fact">
            <dt>续期凭据（refresh）</dt>
            <dd>{tokenExpiry(account.refresh_token_expires_at_unix_seconds)}</dd>
          </div>
        </dl>
        <p className="account-detail__hint">应用会在切号与凭据充裕判断下自动续期；也可在账号页用「刷新登录凭据」手动续期。</p>
      </section>

      {/* 签到与积分：overview 数据面（total_credits/streak/今日状态）。 */}
      <section className="account-detail__section" aria-labelledby="workbuddy-detail-checkin">
        <h2 id="workbuddy-detail-checkin"><CalendarClock size={16} aria-hidden="true" />签到与积分</h2>
        <dl className="account-detail__facts">
          <div className="account-detail__fact">
            <dt>积分余额</dt>
            <dd data-testid="workbuddy-detail-credits">
              {overview?.error
                ? "状态未知（签到状态读取失败）"
                : overview?.total_credits != null
                  ? overview.total_credits
                  : "未读取"}
            </dd>
          </div>
          <div className="account-detail__fact">
            <dt>连签天数</dt>
            <dd data-testid="workbuddy-detail-streak">{overview?.streak_days ?? "未读取"}</dd>
          </div>
          <div className="account-detail__fact">
            <dt>今日签到</dt>
            <dd data-testid="workbuddy-detail-today">
              {overview?.error
                ? "状态未知"
                : overview?.today_checked_in === true
                  ? "已签到"
                  : overview?.today_checked_in === false
                    ? "未签到"
                    : "未读取"}
            </dd>
          </div>
        </dl>
        <p className="account-detail__hint">签到动作在「签到」页；每日自动签到的总开关与触发时间在「设置」页。</p>
      </section>

      {/* 自动签到参与开关：命令面已有（set_workbuddy_account_auto_checkin），实装。 */}
      <section className="account-detail__section" aria-labelledby="workbuddy-detail-participation">
        <h2 id="workbuddy-detail-participation">自动签到参与</h2>
        {participating === null ? (
          <p className="account-detail__hint" data-testid="workbuddy-detail-participation-unknown">
            参与状态暂时不可读取，请返回后重试。
          </p>
        ) : (
          <label className="account-center__profile-select" data-testid="workbuddy-detail-participation-toggle">
            <input
              type="checkbox"
              checked={participating}
              disabled={busy}
              onChange={handleToggleParticipation}
            />
            <span className="account-center__profile-copy">
              <strong>{participating ? "参与每日自动签到" : "不参与每日自动签到"}</strong>
              <span>关闭后每日批次将跳过此账号；手动签到不受影响。</span>
            </span>
          </label>
        )}
      </section>

      {/* 档案管理能力缺口（诚实缺席）：WB 命令面暂无备注名/归档/删除命令，
          DTO 无手机号字段——待后端移植后实装（IA 草案 §1.2 判定「移植」）。 */}
      <section className="account-detail__section" aria-labelledby="workbuddy-detail-absent">
        <h2 id="workbuddy-detail-absent">档案管理</h2>
        <p className="account-detail__hint" data-testid="workbuddy-detail-absent-note">
          备注名编辑、归档与删除需要账号档案命令面（WorkBuddy 后端暂未提供）；
          WorkBuddy 账号档案不包含手机号数据。以上区块将随后端命令面补齐而实装。
        </p>
      </section>

      {/* 技术细节：排障用标识，默认折叠。 */}
      <details className="account-detail__tech">
        <summary><Cpu size={14} aria-hidden="true" />技术细节</summary>
        <dl className="account-detail__facts">
          <div className="account-detail__fact">
            <dt>账号档案标识</dt>
            <dd><code>{account.profile_id}</code></dd>
          </div>
          <div className="account-detail__fact">
            <dt>服务端账号标识</dt>
            <dd><code>{account.account_id}</code></dd>
          </div>
          <div className="account-detail__fact">
            <dt>登录域</dt>
            <dd><code>{account.domain}</code></dd>
          </div>
        </dl>
      </details>
    </section>
  );
}

/** Unix 秒 → 本地「日期 时:分」；0/缺省 = 未知（与账号卡行同口径）。 */
function formatUnixTime(unixSeconds: number): string {
  if (!unixSeconds) return "未知";
  return new Date(unixSeconds * 1000).toLocaleString(undefined, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** 凭据到期展示：剩余天数 + 具体到期时刻；0 = 未采集（诚实标注，不臆测）。 */
function tokenExpiry(expiresAtUnixSeconds: number): string {
  if (!expiresAtUnixSeconds) return "有效期未知（档案未采集）";
  const remainingMs = expiresAtUnixSeconds * 1000 - Date.now();
  if (remainingMs <= 0) return "已过期";
  const days = Math.ceil(remainingMs / 86400000);
  const deadline = new Intl.DateTimeFormat("zh-CN", {
    year: "numeric", month: "2-digit", day: "2-digit",
    hour: "2-digit", minute: "2-digit", hour12: false,
  }).format(new Date(expiresAtUnixSeconds * 1000));
  return `剩 ${days} 天（至 ${deadline}）`;
}
