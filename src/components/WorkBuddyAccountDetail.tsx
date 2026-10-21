import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowLeft, CalendarClock, Coins, Cpu, KeyRound, Phone, Trash2, UserPen } from "lucide-react";
import type {
  WorkBuddyAccountEntryDto,
  WorkBuddyCheckinAccountStatusDto,
} from "../types/product";
import {
  fetchWorkBuddyCheckinOverview,
  listWorkBuddyCheckinAccounts,
  purgeWorkBuddyAccounts,
  setWorkBuddyAccountArchived,
  setWorkBuddyAccountAutoCheckin,
  setWorkBuddyAccountDisplayName,
} from "../platform/workbuddyAdapter";
import { WorkBuddyPoolSlotBadge, deriveWorkBuddyPoolLoginState } from "./StatusBadges";
import { ConfirmDialog } from "./shared/ConfirmDialog";
import { notify } from "../platform/notify";
import { safeUiErrorMessage } from "../utils/safeUiError";
import { workbuddyDisplayName } from "../utils/accountDisplay";

interface WorkBuddyAccountDetailProps {
  /** 详情目标账号（账号池清单条目，P19.2④ 含档案字段）。 */
  account: WorkBuddyAccountEntryDto;
  /** 是否为当前客户端登录账号（健康卡 uid 与池 account_id 同源）。 */
  isCurrentLogin: boolean;
  /** 数据变更（备注名/归档/参与/删除）后通知列表页重读。 */
  onDataChanged: () => Promise<void>;
  /** 返回账号列表。 */
  onBack: () => void;
}

type DetailAction = "alias" | "archive" | "purge" | "participation" | null;

/**
 * WorkBuddy 账号详情（P19.1 详情实装 + P19.2④ 档案管理转实装，对照
 * AdapterAccountDetail 轻量结构）：基础信息（含脱敏手机号）+ 凭据健康度 +
 * 签到与积分 + 自动签到参与开关 + 档案管理（备注名/归档/删除）+ 技术细节折叠。
 *
 * 数据源纪律：全部经既有命令面（be b962ae8 契约）；删除 = 工具侧池记录 +
 * 档案旁表，不触碰客户端 .info、不做客户端登出（弹窗文案写明边界）。
 */
export function WorkBuddyAccountDetail({
  account,
  isCurrentLogin,
  onDataChanged,
  onBack,
}: WorkBuddyAccountDetailProps) {
  const [overview, setOverview] = useState<WorkBuddyCheckinAccountStatusDto | null>(null);
  const [participating, setParticipating] = useState<boolean | null>(null);
  const [busy, setBusy] = useState<DetailAction>(null);
  // P19.2④ 备注名草稿：进入详情时以档案值为准；外部数据刷新时同步重置。
  const [aliasDraft, setAliasDraft] = useState(account.display_name);
  useEffect(() => {
    setAliasDraft(account.display_name);
  }, [account.display_name, account.account_id]);
  // 删除确认弹层（破坏性操作，单次确认纪律）。
  const [purgeDialogOpen, setPurgeDialogOpen] = useState(false);

  useEffect(() => {
    let cancelled = false;
    // 积分/签到状态（overview：余额/累计/连签）；参与集 = 有效参与。
    void fetchWorkBuddyCheckinOverview()
      .then((rows) => {
        if (!cancelled) {
          setOverview(rows.find((row) => row.profile_id === account.profile_id) ?? null);
        }
      })
      .catch(() => undefined); // fail-soft：积分区显示「未获取」
    void listWorkBuddyCheckinAccounts()
      .then((rows) => {
        if (!cancelled) {
          const row = rows.find((item) => item.profile_id === account.profile_id);
          setParticipating(row ? row.enabled : null);
        }
      })
      .catch(() => undefined); // fail-soft：参与行显示「不可读取」
    return () => { cancelled = true; };
  }, [account.profile_id]);

  // 备注名保存：trim 后为空 = 清除（回退捕获昵称）；≤100 字符由后端校验。
  const aliasDirty = aliasDraft.trim() !== account.display_name;
  const handleSaveAlias = useCallback(() => {
    if (busy !== null || !aliasDirty) return;
    setBusy("alias");
    void setWorkBuddyAccountDisplayName(account.account_id, aliasDraft.trim())
      .then(async (entry) => {
        notify.success(entry.display_name ? "备注名已保存。" : "备注名已清除，恢复捕获昵称。");
        await onDataChanged();
      })
      .catch((reason: unknown) => notify.error(
        `WorkBuddy：备注名保存失败——${safeUiErrorMessage(reason, "请稍后重试。")}`,
      ))
      .finally(() => setBusy(null));
  }, [account.account_id, aliasDraft, busy, onDataChanged]);

  // 归档开关：只影响列表标注，不自动退出签到参与集（提示文案写明）。
  const handleToggleArchived = useCallback(() => {
    if (busy !== null) return;
    setBusy("archive");
    void setWorkBuddyAccountArchived(account.account_id, !account.archived)
      .then(async () => {
        notify.success(!account.archived ? "账号已归档。" : "账号已取消归档。");
        await onDataChanged();
      })
      .catch((reason: unknown) => notify.error(
        `WorkBuddy：归档设置失败——${safeUiErrorMessage(reason, "请稍后重试。")}`,
      ))
      .finally(() => setBusy(null));
  }, [account.account_id, account.archived, busy, onDataChanged]);

  // 参与开关翻转：显式写后端 touched+enabled（opt-out 永久尊重，B5 契约）。
  const handleToggleParticipation = useCallback(() => {
    if (busy !== null || participating === null) return;
    setBusy("participation");
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
      .finally(() => setBusy(null));
  }, [account.profile_id, busy, onDataChanged, participating]);

  // 彻底删除：单次确认弹层 → purge(confirmed=true) → 回读刷新 → 回列表。
  const runPurge = useCallback(() => {
    if (busy !== null) return;
    setBusy("purge");
    void purgeWorkBuddyAccounts([account.account_id], true)
      .then(async (receipt) => {
        if (receipt.profile_cleanup_failed.length > 0) {
          notify.warning(
            "账号已从池中删除，但档案备注清理未完成（残留无害，可忽略）。",
          );
        } else {
          notify.success(
            `WorkBuddy：账号「${workbuddyDisplayName(account)}」已删除（剩余 ${receipt.remaining_accounts} 个账号）。`,
          );
        }
        setPurgeDialogOpen(false);
        await onDataChanged();
        onBack();
      })
      .catch((reason: unknown) => {
        setPurgeDialogOpen(false);
        notify.error(
          `WorkBuddy：删除未完成——${safeUiErrorMessage(reason, "账号数据未被改动，请稍后重试。")}`,
        );
      })
      .finally(() => setBusy(null));
  }, [account, busy, onDataChanged, onBack]);

  const poolState = deriveWorkBuddyPoolLoginState({
    access_expires_at_unix_seconds: account.access_token_expires_at_unix_seconds,
    refresh_expires_at_unix_seconds: account.refresh_token_expires_at_unix_seconds,
    now_unix_seconds: Math.floor(Date.now() / 1000),
  });
  const displayName = workbuddyDisplayName(account);

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

      {/* 基础信息：凭据状态、当前登录标记、脱敏手机号（有值才显示）、最近更新。 */}
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
          {account.masked_phone && (
            <div className="account-detail__fact">
              <dt>手机号</dt>
              <dd data-testid="workbuddy-detail-phone">
                <Phone size={13} aria-hidden="true" /> {account.masked_phone}
              </dd>
            </div>
          )}
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

      {/* 签到与积分：overview 数据面（credits_balance 余额主展示 + 累计/连签/今日）。 */}
      <section className="account-detail__section" aria-labelledby="workbuddy-detail-checkin">
        <h2 id="workbuddy-detail-checkin"><CalendarClock size={16} aria-hidden="true" />签到与积分</h2>
        <dl className="account-detail__facts">
          <div className="account-detail__fact">
            <dt>积分余额</dt>
            <dd data-testid="workbuddy-detail-credits">
              {overview?.error
                ? "查询失败（签到状态读取失败）"
                : overview?.credits_balance != null
                  ? overview.credits_balance
                  : overview == null
                    ? "未获取"
                    : overview.balance_error
                      ? `--（${safeUiErrorMessage(overview.balance_error, "余额暂不可读，请稍后刷新。")}）`
                      : "--"}
            </dd>
          </div>
          <div className="account-detail__fact">
            <dt>本期签到累计</dt>
            <dd data-testid="workbuddy-detail-accumulated">
              {overview?.error
                ? "查询失败"
                : overview?.total_credits != null
                  ? overview.total_credits
                  : "未获取"}
            </dd>
          </div>
          <div className="account-detail__fact">
            <dt>连签天数</dt>
            <dd data-testid="workbuddy-detail-streak">{overview?.error ? "查询失败" : overview?.streak_days ?? "未获取"}</dd>
          </div>
          <div className="account-detail__fact">
            <dt>今日签到</dt>
            <dd data-testid="workbuddy-detail-today">
              {overview?.error
                ? "查询失败"
                : overview?.today_checked_in === true
                  ? "已签到"
                  : overview?.today_checked_in === false
                    ? "未签到"
                    : "未获取"}
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
              disabled={busy !== null}
              onChange={handleToggleParticipation}
            />
            <span className="account-center__profile-copy">
              <strong>{participating ? "参与每日自动签到" : "不参与每日自动签到"}</strong>
              <span>关闭后每日批次将跳过此账号；手动签到不受影响。</span>
            </span>
          </label>
        )}
      </section>

      {/* P19.2④ 档案管理（be b962ae8 契约）：备注名编辑 + 归档开关；
          删除下沉到独立危险区（单次确认）。 */}
      <section className="account-detail__section" aria-labelledby="workbuddy-detail-profile">
        <h2 id="workbuddy-detail-profile"><UserPen size={16} aria-hidden="true" />档案管理</h2>
        <dl className="account-detail__facts">
          <div className="account-detail__fact">
            <dt>备注名</dt>
            <dd data-testid="workbuddy-detail-alias">
              <input
                type="text"
                value={aliasDraft}
                placeholder={account.screen_name}
                maxLength={100}
                disabled={busy !== null}
                data-testid="workbuddy-detail-alias-input"
                onChange={(event) => setAliasDraft(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && aliasDirty) {
                    event.preventDefault();
                    handleSaveAlias();
                  }
                }}
              />
              <button
                className="btn"
                type="button"
                onClick={handleSaveAlias}
                disabled={!aliasDirty || busy !== null}
                data-testid="workbuddy-detail-alias-save"
              >
                {busy === "alias" ? "保存中…" : "保存"}
              </button>
            </dd>
          </div>
          <div className="account-detail__fact">
            <dt>归档</dt>
            <dd>
              <label className="account-center__profile-select" data-testid="workbuddy-detail-archived-toggle">
                <input
                  type="checkbox"
                  checked={account.archived}
                  disabled={busy !== null}
                  onChange={handleToggleArchived}
                />
                <span className="account-center__profile-copy">
                  <strong>{account.archived ? "已归档" : "在册"}</strong>
                  <span>归档只影响账号列表标注；不会自动退出签到参与集（需停止自动签到请关闭上方参与开关）。</span>
                </span>
              </label>
            </dd>
          </div>
        </dl>
      </section>

      {/* 危险区：彻底删除（单次确认；边界文案写明——不碰客户端登录态）。 */}
      <section className="account-detail__section account-detail__section--danger" aria-labelledby="workbuddy-detail-danger">
        <h2 id="workbuddy-detail-danger">危险操作</h2>
        <div className="account-detail__danger-row">
          <p className="account-detail__danger-hint">
            删除范围 = 工具侧账号池记录与档案备注（备注名/归档）；
            不会改动 WorkBuddy 客户端的登录状态（客户端如需退出登录请在客户端操作）。
            删除后该账号不可再切换登录或直连签到；重新保存或登录可恢复。
          </p>
          <button
            className="btn btn--danger"
            type="button"
            onClick={() => setPurgeDialogOpen(true)}
            disabled={busy !== null}
            data-testid="workbuddy-detail-purge"
          >
            <Trash2 size={15} aria-hidden="true" />{busy === "purge" ? "删除中…" : "删除账号"}
          </button>
        </div>
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

      {/* 删除单次确认弹层（覆盖性操作；写明边界）。 */}
      {purgeDialogOpen && (
        <ConfirmDialog
          title="删除 WorkBuddy 账号"
          lines={[
            `确定从工具账号池删除「${displayName}」吗？`,
            "将移除：工具侧账号池记录、账号档案备注（备注名/归档）。",
            "不会改动 WorkBuddy 客户端的登录状态（不登出、不碰客户端数据）。",
            "删除后该账号不可再切换登录或直连签到；重新保存或登录可恢复。",
          ]}
          confirmLabel="确认删除"
          busyLabel="删除中…"
          danger
          busy={busy === "purge"}
          onCancel={() => {
            if (busy !== "purge") setPurgeDialogOpen(false);
          }}
          onConfirm={() => runPurge()}
          testId="workbuddy-detail-purge-dialog"
        />
      )}
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
