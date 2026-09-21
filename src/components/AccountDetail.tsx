import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  ArrowLeft,
  CalendarClock,
  Coins,
  Cpu,
  KeyRound,
  LogIn,
  RefreshCw,
  RotateCcw,
  Trash2,
} from "lucide-react";
import type {
  CheckinBatchSummaryDto,
  CredentialRefreshEntryDto,
  CheckinOverviewEntryDto,
  CreditsRefreshEntryDto,
} from "../types/account_switch";
import { safeUiErrorMessage } from "../utils/safeUiError";
import { effectiveDisplayName } from "../utils/accountDisplay";
import { ConfirmDialog } from "./ConfirmDialog";

interface AccountDetailProps {
  entry: CheckinOverviewEntryDto;
  /** OAuth 重新登录（同一账号重复登录会覆盖更新档案与凭据）。 */
  onRelogin: () => Promise<void>;
  loginBusy: boolean;
  /** 数据变更（签到/刷新/删除）后通知列表页重读总览。 */
  onDataChanged: () => Promise<void>;
  /**
   * G11 保存手机号补录（父组件持有总览数据，负责第三层查重提示）：
   * 空串 = 清除补录（回退脱敏号展示）；返回 false = 本次未保存
   * （用户取消，或查重命中转由确认弹层处理）。
   */
  onSaveMobile: (mobile: string) => Promise<boolean>;
  /** 账号删除成功或点击返回时回列表。 */
  onBack: () => void;
}

type DetailAction = "credentials" | "credits" | "checkin" | "remove" | "relogin" | "auto-checkin" | "reset-device" | "alias" | "mobile" | null;

/** 需要二次确认的详情页操作（弹层确认后执行）。 */
type ConfirmableAction = "credentials" | "reset-device" | "remove";

/**
 * 单账号详情独立视图：基础信息 -> 登录健康度 -> 操作区 -> 折叠技术细节。
 * 操作区包含：刷新登录凭据（同设备换发，不打开用户 OAuth）/ 刷新额度（只读查询）/
 * 立即签到（单账号批次）/ 重新登录（OAuth 覆盖更新）/ 删除账号（二次确认）。
 */
export function AccountDetail({ entry, onRelogin, loginBusy, onDataChanged, onSaveMobile, onBack }: AccountDetailProps) {
  const [busy, setBusy] = useState<DetailAction>(null);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  // 待确认操作：非空即弹出统一确认弹层（ConfirmDialog）。
  const [pendingAction, setPendingAction] = useState<ConfirmableAction | null>(null);
  // U-1 备注名草稿：进入详情时以档案值为准；外部数据刷新时同步重置。
  const [aliasDraft, setAliasDraft] = useState(entry.display_name ?? "");
  useEffect(() => {
    setAliasDraft(entry.display_name ?? "");
  }, [entry.display_name, entry.profile_id]);
  // G11 手机号补录草稿：与备注名同一模式（输入值 = 补录全号；占位 = 脱敏号）。
  const [mobileDraft, setMobileDraft] = useState(entry.mobile_full ?? "");
  useEffect(() => {
    setMobileDraft(entry.mobile_full ?? "");
  }, [entry.mobile_full, entry.profile_id]);

  const runAction = useCallback(async (action: Exclude<DetailAction, null>, task: () => Promise<void>) => {
    setBusy(action);
    setError(null);
    setMessage(null);
    try {
      await task();
    } catch (reason: unknown) {
      setError(safeUiErrorMessage(reason, "操作未完成，请稍后重试。"));
    } finally {
      setBusy(null);
    }
  }, []);

  // U-1 保存备注名：trim 后为空 = 清除（回退服务端名）。
  const aliasDirty = aliasDraft.trim() !== (entry.display_name ?? "").trim();
  const handleSaveAlias = useCallback(() => void runAction("alias", async () => {
    await invoke("set_account_display_name", {
      profileId: entry.profile_id,
      displayName: aliasDraft.trim() || null,
    });
    await onDataChanged();
    setMessage(aliasDraft.trim() ? "备注名已保存。" : "备注名已清除，恢复服务端名称。");
  }), [aliasDraft, entry.profile_id, onDataChanged, runAction]);

  // G11 保存手机号：非空 = 补录全号（父组件做查重提示）；空 = 清除（回退脱敏号）。
  const mobileDirty = mobileDraft.trim() !== (entry.mobile_full ?? "").trim();
  const handleSaveMobile = useCallback(() => void runAction("mobile", async () => {
    const saved = await onSaveMobile(mobileDraft.trim());
    if (saved) {
      setMessage(mobileDraft.trim() ? "手机号已保存。" : "已清除补录的手机号，恢复显示脱敏号。");
    }
  }), [mobileDraft, onSaveMobile, runAction]);

  // 刷新额度：只读查询（不消耗签到资格），写回缓存后重读总览。
  const handleRefreshCredits = useCallback(() => void runAction("credits", async () => {
    const results = await invoke<CreditsRefreshEntryDto[]>("refresh_checkin_credits", {
      profileIds: [entry.profile_id],
    });
    const result = results[0];
    if (result?.error_code) {
      throw new Error(result.error_code);
    }
    await onDataChanged();
    const usage = result?.usage_remaining_credits !== null && result?.usage_remaining_credits !== undefined
      ? formatCreditsValue(result.usage_remaining_credits)
      : "未知";
    setMessage(`额度已更新：模型积分 ${usage}（今日${result?.checked_in ? "已签" : "未签"}）。`);
  }), [entry.profile_id, onDataChanged, runAction]);

  // 手动刷新登录凭据：只执行同设备换发，成功后写回本机加密凭据与 TRAE 登录 blob。
  // 入口只负责打开二次确认弹层，执行体在 executePendingAction。
  const handleRefreshCredentials = useCallback(() => {
    setPendingAction("credentials");
  }, []);

  // 重铸签到设备（ADR-0019 v5）：走网络完整流程（GetPCAuthCode →
  // ExchangeToken → 凭据写回）；连续多日 9074/9095 时的人工兜底，
  // 确认后执行（更换设备绑定与登录令牌）。入口只负责打开二次确认弹层。
  const handleResetDevice = useCallback(() => {
    setPendingAction("reset-device");
  }, []);

  // 删除账号：破坏性操作，二次确认（铁律：单次确认后执行）。入口只负责打开弹层。
  const handleRemove = useCallback(() => {
    setPendingAction("remove");
  }, []);

  // 确认弹层内容：标题 + 影响范围行 + 按钮形态（破坏性用危险色）。
  const pendingActionMeta = pendingAction === null ? null : {
    credentials: {
      title: "刷新登录凭据",
      lines: [
        `将为账号“${entry.screen_name}”向服务端换发新的登录令牌，并更新本机保存的登录信息。`,
        "期间不会打开新的登录页面。",
      ],
      confirmLabel: "刷新凭据",
      busyLabel: "刷新中…",
      danger: false,
    },
    "reset-device": {
      title: "重置签到设备",
      lines: [
        `确定为账号“${entry.screen_name}”重置签到设备吗？`,
        "将通过服务端生成全新设备并更新登录信息；不影响账号与其他数据。",
      ],
      confirmLabel: "重置设备",
      busyLabel: "重置中…",
      danger: false,
    },
    remove: {
      title: "删除账号",
      lines: [
        `确定删除账号“${entry.screen_name}”吗？`,
        "将移除：账号档案、本机加密登录凭据、积分缓存。",
        "不影响其他账号与历史数据，删除后可通过重新登录找回。",
      ],
      confirmLabel: "删除",
      busyLabel: "删除中…",
      danger: true,
    },
  }[pendingAction];

  // 确认后关闭弹层并执行对应操作（页面按钮同步进入 busy 态，失败信息走页面错误条）。
  const executePendingAction = useCallback(() => {
    const action = pendingAction;
    if (!action) return;
    setPendingAction(null);
    if (action === "credentials") {
      void runAction("credentials", async () => {
        const results = await invoke<CredentialRefreshEntryDto[]>("refresh_checkin_credentials", {
          profileIds: [entry.profile_id],
        });
        const result = results[0];
        if (!result?.refreshed) {
          throw new Error(result?.error_code ?? "credential_refresh_failed");
        }
        await onDataChanged();
        setMessage("登录凭据已更新，无需重新登录。");
      });
    } else if (action === "reset-device") {
      void runAction("reset-device", async () => {
        await invoke("reset_checkin_device", { profileId: entry.profile_id });
        await onDataChanged();
        setMessage("签到设备已重置，下次签到将使用新设备。");
      });
    } else {
      void runAction("remove", async () => {
        await invoke("remove_checkin_account", { profileId: entry.profile_id });
        await onDataChanged();
        onBack();
      });
    }
  }, [pendingAction, entry.profile_id, onDataChanged, onBack, runAction]);

  // 立即签到：单账号批次，当日已签（缓存确认）时按钮禁用并显示已签状态。
  const checkedIn = entry.checked_in === true;
  const handleCheckin = useCallback(() => void runAction("checkin", async () => {
    const summary = await invoke<CheckinBatchSummaryDto>("run_checkin", { profileIds: [entry.profile_id] });
    await onDataChanged();
    const first = summary.results[0];
    if (first?.outcome === "claimed") {
      // status.credits 为静态池值，不反映签到奖励（实证恒 200）；
      // 成功的权威信号是 claimed + checked_in 翻转，不展示增量。
      setMessage("签到成功，奖励已发放。");
    } else if (first?.outcome === "already_checked_in") {
      setMessage("今日已签到，无需重复领取。");
    } else if (first?.detail_code) {
      // 原始业务码抛出，由 safeUiErrorMessage 映射文案
      // （ADR-0019：业务码不得压平）。
      throw new Error(first.detail_code);
    } else {
      throw new Error("checkin_failed");
    }
  }), [entry.profile_id, onDataChanged, runAction]);

  const handleRelogin = useCallback(() => {
    void runAction("relogin", async () => {
      await onRelogin();
      setMessage("重新登录完成，登录凭据已更新。");
    });
  }, [onRelogin, runAction]);

  return (
    <section className="account-detail" role="region" aria-label="账号详情">
      <header className="page-header">
        <div className="page-header__copy">
          <button className="btn btn--quiet" type="button" onClick={onBack} data-testid="account-detail-back">
            <ArrowLeft size={15} aria-hidden="true" />返回账号列表
          </button>
          <h1 data-page-title="accounts" tabIndex={-1}>{effectiveDisplayName(entry)}</h1>
        </div>
      </header>

      {error && <p className="workbench__error" role="alert">{error}</p>}
      {message && <p className="account-detail__message" role="status">{message}</p>}

      {/* 基础信息：备注名（行内编辑）、手机号、额度、今日状态、时间与设备 */}
      <section className="account-detail__section" aria-labelledby="account-detail-basic">
        <h2 id="account-detail-basic"><Coins size={16} aria-hidden="true" />基础信息</h2>
        <dl className="account-detail__facts">
          <div className="account-detail__fact">
            <dt>备注名</dt>
            <dd data-testid="account-detail-alias">
              {/* U-1：本地别名行内编辑；空 = 显示服务端名。 */}
              <input
                type="text"
                value={aliasDraft}
                placeholder={entry.screen_name}
                maxLength={64}
                disabled={busy !== null || loginBusy}
                data-testid="account-detail-alias-input"
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
                disabled={!aliasDirty || busy !== null || loginBusy}
                data-testid="account-detail-alias-save"
              >
                {busy === "alias" ? "保存中…" : "保存"}
              </button>
            </dd>
          </div>
          <div className="account-detail__fact">
            <dt>手机号</dt>
            <dd data-testid="account-detail-mobile">
              {/* G11：行内补录完整手机号（与备注名同款交互）；未补录时占位
                  显示脱敏号，保存后显示全号；清空保存 = 回退脱敏号。 */}
              <div className="account-detail__mobile-row">
                <input
                  type="tel"
                  inputMode="numeric"
                  value={mobileDraft}
                  placeholder={entry.masked_mobile || "未采集"}
                  maxLength={11}
                  disabled={busy !== null || loginBusy}
                  data-testid="account-detail-mobile-input"
                  aria-label="完整手机号"
                  onChange={(event) => setMobileDraft(event.target.value.replace(/[^\d]/g, ""))}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" && mobileDirty) {
                      event.preventDefault();
                      handleSaveMobile();
                    }
                  }}
                />
                <button
                  className="btn"
                  type="button"
                  onClick={handleSaveMobile}
                  disabled={!mobileDirty || busy !== null || loginBusy}
                  data-testid="account-detail-mobile-save"
                >
                  {busy === "mobile" ? "保存中…" : "保存"}
                </button>
              </div>
              {entry.masked_mobile && (
                <p className="account-detail__mobile-hint">服务端脱敏号：{entry.masked_mobile}（输入需与其首尾号段一致）</p>
              )}
            </dd>
          </div>
          <div className="account-detail__fact">
            <dt>模型积分（真实额度）</dt>
            <dd data-testid="account-detail-usage">
              {entry.usage_remaining_credits != null
                ? `${formatCreditsValue(entry.usage_remaining_credits)}${entry.usage_cached_at ? `（${formatDateTime(entry.usage_cached_at)} 更新）` : ""}`
                : "未查询（点击下方“刷新额度”）"}
            </dd>
          </div>
          <div className="account-detail__fact">
            <dt>最近额度刷新</dt>
            <dd data-testid="account-detail-refresh-status">
              {entry.refresh_error_code != null ? (
                <span className="account-detail__fact-warn" title="下次刷新成功后此提示自动消失">
                  失败：{safeUiErrorMessage(entry.refresh_error_code, "原因未知，请稍后重试。")}
                </span>
              ) : entry.usage_cached_at ? (
                `正常（${formatDateTime(entry.usage_cached_at)}）`
              ) : (
                "尚未刷新"
              )}
            </dd>
          </div>
          <div className="account-detail__fact">
            <dt>今日签到</dt>
            <dd data-testid="account-detail-checked-in">
              {entry.checked_in === true ? "已签到" : entry.checked_in === false ? "未签到" : "未知（刷新后显示）"}
            </dd>
          </div>
          <div className="account-detail__fact">
            <dt>添加时间</dt>
            <dd>{formatDateTime(entry.created_at)}</dd>
          </div>
          <div className="account-detail__fact">
            <dt>最近验证</dt>
            <dd>{formatDateTime(entry.last_verified_at)}</dd>
          </div>
          <div className="account-detail__fact">
            <dt>签到设备</dt>
            <dd>{entry.device_tail ? `…${entry.device_tail}` : "未知"}</dd>
          </div>
        </dl>
      </section>

      {/* 登录健康度：token 到期时间轴 */}
      <section className="account-detail__section" aria-labelledby="account-detail-health">
        <h2 id="account-detail-health"><KeyRound size={16} aria-hidden="true" />登录健康度</h2>
        <dl className="account-detail__facts">
          <div className="account-detail__fact">
            <dt>访问令牌（access）</dt>
            <dd>{tokenExpiry(entry.access_token_expires_at_unix_seconds, false)}</dd>
          </div>
          <div className="account-detail__fact">
            <dt>刷新令牌（refresh）</dt>
            <dd>{tokenExpiry(entry.refresh_token_expires_at_unix_seconds, true)}</dd>
          </div>
        </dl>
        {entry.credential_legacy ? (
          <p className="account-detail__hint account-detail__hint--warn" data-testid="account-detail-legacy-hint">
            该账号仍在使用旧版登录通道；可先尝试“刷新登录凭据”，若当前凭据已无法换发，再重新登录。
          </p>
        ) : (
          <p className="account-detail__hint">应用会按需维护登录凭据，也可手动刷新；不会重复打开用户 OAuth 登录。</p>
        )}
      </section>

      {/* 自动签到参与开关：只影响自动批次，手动签到不受限 */}
      <section className="account-detail__section" aria-labelledby="account-detail-auto-checkin">
        <h2 id="account-detail-auto-checkin"><CalendarClock size={16} aria-hidden="true" />自动签到</h2>
        <label className="account-center__profile-select" data-testid="auto-checkin-account-toggle">
          <input
            type="checkbox"
            checked={entry.auto_checkin_enabled}
            disabled={busy !== null || loginBusy}
            onChange={() => void runAction("auto-checkin", async () => {
              await invoke("set_account_auto_checkin", {
                profileId: entry.profile_id,
                enabled: !entry.auto_checkin_enabled,
              });
              await onDataChanged();
              setMessage(!entry.auto_checkin_enabled ? "已加入每日自动签到。" : "已退出每日自动签到。");
            })}
          />
          <span className="account-center__profile-copy">
            <strong>{entry.auto_checkin_enabled ? "参与每日自动签到" : "不参与每日自动签到"}</strong>
            <span>到点后与其他账号随机错峰执行；关闭后仍可手动签到。</span>
          </span>
        </label>
      </section>

      {/* 操作区（U-2 危险分区）：常规操作一组；删除下沉到独立危险区，避免误触。 */}
      <section className="account-detail__section" aria-labelledby="account-detail-actions">
        <h2 id="account-detail-actions">操作</h2>
        <div className="account-detail__actions">
          <button className="btn btn--primary" type="button" onClick={handleRefreshCredentials} disabled={busy !== null || loginBusy} data-testid="account-detail-refresh-credentials">
            <RefreshCw size={15} aria-hidden="true" />{busy === "credentials" ? "刷新中…" : "刷新登录凭据"}
          </button>
          <button className="btn" type="button" onClick={handleRefreshCredits} disabled={busy !== null || loginBusy}>
            <RefreshCw size={15} aria-hidden="true" />{busy === "credits" ? "查询中…" : "刷新额度"}
          </button>
          <button
            className="btn btn--primary"
            type="button"
            onClick={handleCheckin}
            disabled={busy !== null || loginBusy || checkedIn}
            title={checkedIn ? "今日已签到（缓存确认），无需重复领取" : undefined}
            data-testid="account-detail-checkin"
          >
            <CalendarClock size={15} aria-hidden="true" />
            {busy === "checkin" ? "签到中…" : checkedIn ? "今日已签到" : "立即签到"}
          </button>
          <button className="btn" type="button" onClick={handleRelogin} disabled={busy !== null || loginBusy}>
            <LogIn size={15} aria-hidden="true" />{loginBusy ? "等待浏览器登录…" : "重新登录"}
          </button>
          <button
            className="btn"
            type="button"
            onClick={handleResetDevice}
            disabled={busy !== null || loginBusy}
            title="连续多日签到被拒（9074/9095）时使用；通过服务端生成全新设备"
            data-testid="account-detail-reset-device"
          >
            <RotateCcw size={15} aria-hidden="true" />{busy === "reset-device" ? "重置中…" : "重置签到设备"}
          </button>
        </div>
      </section>

      {/* 危险区：破坏性操作独立分隔线下沉页底（U-2 分区决策）。 */}
      <section className="account-detail__section account-detail__section--danger" aria-labelledby="account-detail-danger">
        <h2 id="account-detail-danger">危险操作</h2>
        <div className="account-detail__danger-row">
          <p className="account-detail__danger-hint">
            删除将移除账号档案、本机加密登录凭据与积分缓存；不影响其他账号与历史数据，删除后可通过重新登录找回。
          </p>
          <button className="btn btn--danger" type="button" onClick={handleRemove} disabled={busy !== null || loginBusy}>
            <Trash2 size={15} aria-hidden="true" />{busy === "remove" ? "删除中…" : "删除账号"}
          </button>
        </div>
      </section>

      {/* 技术细节：排障用标识，默认折叠 */}
      <details className="account-detail__tech">
        <summary><Cpu size={14} aria-hidden="true" />技术细节</summary>
        <dl className="account-detail__facts">
          <div className="account-detail__fact">
            <dt>账号档案标识</dt>
            <dd><code>{entry.profile_id}</code></dd>
          </div>
          <div className="account-detail__fact">
            <dt>服务端账号标识</dt>
            <dd><code>{entry.account_id}</code></dd>
          </div>
          <div className="account-detail__fact">
            <dt>设备 ID</dt>
            <dd><code>{entry.device_id ?? "未知"}</code></dd>
          </div>
        </dl>
      </details>

      {/* 二次确认弹层：刷新凭据 / 重置设备 / 删除账号共用（与全工具确认形态统一）。 */}
      {pendingActionMeta && (
        <ConfirmDialog
          title={pendingActionMeta.title}
          lines={pendingActionMeta.lines}
          confirmLabel={pendingActionMeta.confirmLabel}
          busyLabel={pendingActionMeta.busyLabel}
          danger={pendingActionMeta.danger}
          onCancel={() => setPendingAction(null)}
          onConfirm={executePendingAction}
          testId={`account-detail-${pendingAction}-confirm`}
        />
      )}
    </section>
  );
}

/** token 到期展示：剩余天数 + 具体到期时刻。 */
function tokenExpiry(expiresAt: number | null, isRefresh: boolean): string {
  if (expiresAt === null) return "未能读取";
  const remainingMs = expiresAt * 1000 - Date.now();
  if (remainingMs <= 0) return "已过期";
  const days = Math.ceil(remainingMs / 86400000);
  const deadline = new Intl.DateTimeFormat("zh-CN", {
    year: "numeric", month: "2-digit", day: "2-digit",
    hour: "2-digit", minute: "2-digit", hour12: false,
  }).format(new Date(expiresAt * 1000));
  const kind = isRefresh ? "刷新" : "访问";
  return `${kind}令牌剩 ${days} 天（至 ${deadline}）`;
}

function formatDateTime(value: string | null): string {
  if (!value) return "未知";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "未知";
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric", month: "2-digit", day: "2-digit",
    hour: "2-digit", minute: "2-digit", hour12: false,
  }).format(date);
}

/** 额度数值格式化：保留 1 位小数（整数不带小数点），与账号卡片同口径。 */
function formatCreditsValue(value: number): string {
  const rounded = Math.round(value * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
}
