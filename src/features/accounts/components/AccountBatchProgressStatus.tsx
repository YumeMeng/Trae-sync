import type { AccountBatchProgress, AccountBatchProgressMap } from "./useAccountBatchProgress";

/**
 * 批量进度逐账号文案（P4c-9c 补回 K-05）：旧 AccountCenterTrae 在页头下方状态区按链
 * 逐账号上屏（旧 L1140-1161），本组件是同一位置、同一文案的纯展示实现。
 *
 * 渲染条件与旧实现同口径：该链**忙碌**（批次在跑）且**已收到该链进度事件**。
 * 旧实现里进度只是把忙碌文案「升级」为逐账号文案，故：
 * - 无进度 → 不渲染（旧实现在此显示回退文案，按本票边界不补，差异见 K-05）；
 * - 批次收尾后忙碌位归零，残进度不会继续上屏（三链都在批次前后清进度，与旧实现同法）。
 * 三条链按钮互斥（页头按钮互斥），同屏最多两条文案（健康检测见下方说明）。
 */

/** 旧实现文案逐字：`正在处理 ${completed}/${total}：${name}`（旧 L1141 / L1150 / L1160）。 */
const progressCopy = (progress: AccountBatchProgress): string =>
  `正在处理 ${progress.completed}/${progress.total}：${progress.name}`;

/** 三链忙碌位（与进度链 kind 一一对应；缺省 false 即该链未在跑）。 */
export interface AccountBatchBusy {
  readonly credits: boolean;
  readonly credentials: boolean;
  readonly health: boolean;
}

export interface AccountBatchProgressStatusProps {
  /** 三链进度（按 kind 判别，缺键即该链无进度）。 */
  readonly progress: AccountBatchProgressMap;
  /** 三链忙碌位：批次收尾后即归零，避免残进度继续上屏。 */
  readonly busy: AccountBatchBusy;
}

export function AccountBatchProgressStatus({ progress, busy }: AccountBatchProgressStatusProps) {
  // 旧 L939 healthProgressShown：健康检测的网络步骤复用额度链事件（refresh_checkin_credits
  // 同时服务「刷新额度」与「健康检测网络步骤」），故先看健康链、再看额度链。
  const healthShown = progress.health_check ?? progress.credits_refresh;
  // 渲染顺序与旧实现一致：额度 → 凭据 → 健康检测。
  const lines: Array<{ readonly key: string; readonly text: string }> = [];
  if (busy.credits && progress.credits_refresh !== undefined) {
    lines.push({ key: "credits", text: progressCopy(progress.credits_refresh) });
  }
  if (busy.credentials && progress.credential_refresh !== undefined) {
    lines.push({ key: "credentials", text: progressCopy(progress.credential_refresh) });
  }
  if (busy.health && healthShown !== undefined) {
    lines.push({ key: "health", text: progressCopy(healthShown) });
  }
  if (lines.length === 0) return null;
  return (
    <>
      {lines.map((line) => (
        <p key={line.key} className="account-center__meta" role="status">{line.text}</p>
      ))}
    </>
  );
}
