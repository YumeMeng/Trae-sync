/**
 * 右栏会话查看器（P4e-7b，纯展示）：标题区（标题 / 项目 / 消息数 / 最后活动 / 关闭）+ 消息·接力两 tab
 * + 消息流四态（loading / error / empty / ready）+ 顶部「加载更早」分页提示。
 * 无会话（viewer === null）时给空态引导（library-viewer-empty）。
 * testid 逐字沿用旧锚（L=LibrarySessionsPanel）：library-viewer（L2029）、library-viewer-title（L2032）、
 * library-viewer-close（L2052）、library-tab-messages/relay（L2064/L2074）、library-viewer-messages（L2142）、
 * library-chat-loading-older / library-chat-notice（L2145/L2151）、library-viewer-empty（L1724）。
 * 滚动锚定（滚到顶请求更早一页、拼接后保持滚动位置）归装配层（useSessionViewer）：本层只暴露 flowRef 与 onLoadOlder。
 * 不取数、不判断产品、不读时钟、不发命令；文案取 config 文案表。
 */
import type { ReactNode, Ref, UIEvent } from "react";
import { Clock, MessageSquare, X } from "lucide-react";
import { LIBRARY_TEXTS, fillLibraryText, type ViewerPaneView } from "../config";
import { CHAT_WINDOW_LIMIT } from "../relayLegs";
import { MessageFlow, RelayTimeline, type MessageFlowItem } from "./MessageFlow";

/** 标题区数据（会话标题与时刻文案由 builder / 装配层给，本层不取数、不读时钟）。 */
export interface SessionViewerHeading {
  readonly title: string; // 已回退「未命名会话」的会话标题
  readonly projectName: string; // 所属项目名
  readonly messageCount: number; // 会话消息总数（不是当前窗口条数）
  readonly lastActiveLabel: string; // 已格式化的「最后活动」时刻
}

/** 查看器视图（K-08：消息可携带 thoughts，TRAE 侧由 TraeMessageView 补齐；WB 侧不携带即不展开）。 */
export interface SessionViewerView extends ViewerPaneView { readonly messages: readonly MessageFlowItem[] }

export interface SessionViewerProps {
  /** 查看器视图（null = 未选会话 → 空态引导）。 */
  readonly view: SessionViewerView | null;
  /** 标题区（view 非 null 时必给）。 */
  readonly heading: SessionViewerHeading;
  /** 消息流容器 ref（装配层用于默认滚到底与拼接后保持滚动位置）。 */
  readonly flowRef?: Ref<HTMLDivElement>;
  /** tab 切换出口（messages / relay）。 */
  readonly onTabChange: (tab: "messages" | "relay") => void;
  /** 关闭查看器出口。 */
  readonly onClose: () => void;
  /** 滚到顶「加载更早」出口（是否可加载由 view.hasMoreBefore / loadingOlder 判定）。 */
  readonly onLoadOlder: () => void;
}

/** 标题区（L2030-2056）：标题 + 项目 + 消息数 + 最后活动 + 关闭。 */
function ViewerHead({ heading, onClose }: { readonly heading: SessionViewerHeading; readonly onClose: () => void }) {
  return (
    <div className="viewer-head">
      <div className="viewer-head__main">
        <div className="viewer-head__title" data-testid="library-viewer-title">{heading.title}</div>
        <div className="viewer-head__meta">
          <span>{heading.projectName}</span>
          <span>
            <MessageSquare size={11} aria-hidden="true" />
            {fillLibraryText(LIBRARY_TEXTS.messageCount, { count: heading.messageCount })}
          </span>
          <span>
            <Clock size={11} aria-hidden="true" />
            {fillLibraryText(LIBRARY_TEXTS.lastActive, { time: heading.lastActiveLabel })}
          </span>
        </div>
      </div>
      <button className="btn viewer-close" type="button" onClick={onClose}
        aria-label={LIBRARY_TEXTS.closeViewer} data-testid="library-viewer-close">
        <X size={15} aria-hidden="true" />
      </button>
    </div>
  );
}

/** 消息流四态（L2123-2131）：loading / error / empty 各一句，ready 才渲染平铺消息与分页提示条。 */
function MessagesBody({ view, flowRef, onLoadOlder }: {
  readonly view: SessionViewerView; readonly flowRef?: Ref<HTMLDivElement>; readonly onLoadOlder: () => void;
}) {
  if (view.status === "loading") return <div className="viewer-state">{LIBRARY_TEXTS.messagesLoading}</div>;
  if (view.status === "error") return <div className="viewer-state">{LIBRARY_TEXTS.messagesError}</div>;
  if (view.status === "empty") return <div className="viewer-state">{view.emptyText}</div>;
  // 滚到顶（scrollTop <= 24）且仍有更早消息、且不在加载中 → 请求上一页；位置保持由装配层做。
  const onScroll = (event: UIEvent<HTMLDivElement>) => {
    if (event.currentTarget.scrollTop > 24 || !view.hasMoreBefore || view.loadingOlder) return;
    onLoadOlder();
  };
  return (
    <div className="chat-flow" ref={flowRef} onScroll={onScroll} data-testid="library-viewer-messages">
      {view.loadingOlder && (
        <div className="chat-notice" data-testid="library-chat-loading-older">{LIBRARY_TEXTS.loadingOlder}</div>
      )}
      {view.hasMoreBefore && view.messageOffset >= CHAT_WINDOW_LIMIT && (
        <div className="chat-notice" data-testid="library-chat-notice">
          {fillLibraryText(LIBRARY_TEXTS.loadedWindow, { offset: view.messageOffset })}
        </div>
      )}
      <MessageFlow messages={view.messages} />
    </div>
  );
}

/** 单枚查看器 tab（L2058-2077）：消息 / 接力；aria-selected 与激活类同步。 */
function ViewerTab({ id, label, active, testId, onClick }: {
  readonly id: "messages" | "relay"; readonly label: string; readonly active: boolean;
  readonly testId: string; readonly onClick: (id: "messages" | "relay") => void;
}) {
  return (
    <button type="button" role="tab" aria-selected={active} className={`viewer-tab${active ? " viewer-tab--active" : ""}`}
      onClick={() => onClick(id)} data-testid={testId}>{label}</button>
  );
}

/** 查看器：无会话给空态引导；有会话给标题区 + tab 条 + tab 面板（消息流 / 接力时间线）。 */
export function SessionViewer({ view, heading, flowRef, onTabChange, onClose, onLoadOlder }: SessionViewerProps) {
  if (view === null) {
    return (
      <section className="viewer-panel" aria-label={LIBRARY_TEXTS.regionViewer}>
        <div className="viewer-empty" data-testid="library-viewer-empty">
          <MessageSquare size={30} strokeWidth={1.4} aria-hidden="true" />
          <p>{LIBRARY_TEXTS.viewerEmpty}</p>
        </div>
      </section>
    );
  }
  const tab: ReactNode = view.activeTab === "messages"
    ? <MessagesBody view={view} flowRef={flowRef} onLoadOlder={onLoadOlder} />
    : <RelayTimeline legs={view.legs} />;
  return (
    <section className="viewer-panel" aria-label={view.ariaLabel}>
      <div className="viewer" data-testid="library-viewer">
        <ViewerHead heading={heading} onClose={onClose} />
        <div className="viewer-tabs" role="tablist" aria-label={view.tabsAriaLabel}>
          <ViewerTab id="messages" label={LIBRARY_TEXTS.tabMessages} active={view.activeTab === "messages"}
            testId="library-tab-messages" onClick={onTabChange} />
          <ViewerTab id="relay" label={LIBRARY_TEXTS.tabRelay} active={view.activeTab === "relay"}
            testId="library-tab-relay" onClick={onTabChange} />
        </div>
        <div className="viewer-body">{tab}</div>
      </div>
    </section>
  );
}
