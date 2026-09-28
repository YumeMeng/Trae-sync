import { ArrowLeft, ArrowLeftRight, Coins, Cpu } from "lucide-react";
import type { AccountView } from "../platform/accountAdapter";
import { RemoteDevicesPanel } from "./RemoteDevicesPanel";

interface AdapterAccountDetailProps {
  /** 详情目标账号（适配器快照中的条目）。 */
  account: AccountView;
  /** 面板归属产品（workbench 当前 adapter 的产品），由父层传入。 */
  productId: string;
  productName: string;
  /** 适配器是否声明 switch 能力；不含能力时操作区不渲染。 */
  canSwitch: boolean;
  /** 切换事务进行中（同一时刻只允许一个切换）。 */
  switchBusy: boolean;
  /** 切换到此账号（父层接 handleAdapterSwitch）。 */
  onSwitch: () => void;
  /** 退出登录设备成功后联动：父层注入单账号健康检测。 */
  onSignedOut: () => void;
  /** 返回账号列表。 */
  onBack: () => void;
}

/**
 * 适配器产品的轻量账号详情（P10-5）：基础信息 + 可选切换 + 登录设备。
 * 设备管理绑定在每个账号上，不在账号列表页另设独立模块。
 * 信息字段只用适配器快照已有数据（AccountView），不发明新数据；
 * 内部标识收进默认折叠的技术细节区（界面表达纪律）。
 */
export function AdapterAccountDetail({
  account,
  productId,
  productName,
  canSwitch,
  switchBusy,
  onSwitch,
  onSignedOut,
  onBack,
}: AdapterAccountDetailProps) {
  const displayName = account.displayName || "未命名账号";
  // 凭据状态徽章：与账号卡片（AdapterAccountCard）同口径的三态映射。
  const statusClass =
    account.status === "active"
      ? "slot-badge--ok"
      : account.status === "expired"
        ? "slot-badge--danger"
        : "slot-badge--idle";
  const statusLabel =
    account.status === "active"
      ? "登录有效"
      : account.status === "expired"
        ? "登录已过期"
        : "状态待验证";

  return (
    <section className="account-detail" role="region" aria-label="账号详情" data-testid="adapter-account-detail">
      <header className="page-header">
        <div className="page-header__copy">
          <button className="btn btn--quiet" type="button" onClick={onBack} data-testid="adapter-account-detail-back">
            <ArrowLeft size={15} aria-hidden="true" />返回账号列表
          </button>
          <h1 data-page-title="accounts" tabIndex={-1}>{displayName}</h1>
        </div>
      </header>

      {/* 基础信息：状态徽章、当前使用标记、最近验证（全部来自适配器快照）。 */}
      <section className="account-detail__section" aria-labelledby="adapter-account-detail-basic">
        <h2 id="adapter-account-detail-basic"><Coins size={16} aria-hidden="true" />基础信息</h2>
        <dl className="account-detail__facts">
          <div className="account-detail__fact">
            <dt>登录状态</dt>
            <dd data-testid="adapter-account-detail-status">
              <span className={`slot-badge ${statusClass}`} title={`${productName} 当前保存凭据的状态`}>
                {statusLabel}
              </span>
            </dd>
          </div>
          <div className="account-detail__fact">
            <dt>当前使用</dt>
            <dd data-testid="adapter-account-detail-current">
              {account.isCurrent ? (
                <span className="account-item__current-chip" title="当前产品正在使用的账号">
                  使用中
                </span>
              ) : (
                "否"
              )}
            </dd>
          </div>
          <div className="account-detail__fact">
            <dt>最近验证</dt>
            <dd data-testid="adapter-account-detail-verified">{formatDateTime(account.lastVerifiedAt)}</dd>
          </div>
        </dl>
      </section>

      {/* 可选操作区：仅当适配器支持切换且目标不是当前账号时出现。 */}
      {canSwitch && !account.isCurrent && (
        <section className="account-detail__section" aria-labelledby="adapter-account-detail-actions">
          <h2 id="adapter-account-detail-actions">操作</h2>
          <div className="account-detail__actions">
            <button
              className="btn btn--primary"
              type="button"
              disabled={switchBusy}
              onClick={onSwitch}
              data-testid="adapter-account-detail-switch"
              title={`切换当前产品到 ${displayName}`}
            >
              <ArrowLeftRight size={15} aria-hidden="true" />
              {switchBusy ? "切换中…" : "切换到此账号"}
            </button>
          </div>
        </section>
      )}

      {/* 登录设备模块：固定绑定本账号，默认折叠，展开才触网。 */}
      <RemoteDevicesPanel
        productId={productId}
        productName={productName}
        profileId={account.id}
        accountLabel={displayName}
        onSignedOut={onSignedOut}
      />

      {/* 技术细节：账号标识只做排障参考，默认折叠。 */}
      <details className="account-detail__tech">
        <summary><Cpu size={14} aria-hidden="true" />技术细节</summary>
        <dl className="account-detail__facts">
          <div className="account-detail__fact">
            <dt>账号标识</dt>
            <dd><code>{account.id}</code></dd>
          </div>
        </dl>
      </details>
    </section>
  );
}

/** 时间展示与账号卡片同口径：未验证 / 不可读都有中性回退文案。 */
function formatDateTime(value: string | null): string {
  if (!value) return "尚未验证";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "时间不可用";
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric", month: "2-digit", day: "2-digit",
    hour: "2-digit", minute: "2-digit", hour12: false,
  }).format(date);
}
