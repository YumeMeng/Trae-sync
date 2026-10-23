/**
 * 消息流与接力时间线（P4e-7b，纯展示）：消息行三档（user 按归属腿着色 / assistant 中性 / trace 任务轨迹摘要）
 * 与接力时间线（账号腿 + 时间段 + 新增消息数 + 当前腿徽章）。
 * K-08 闭环：轨迹行的 thoughts 展开列表在此渲染；thoughts 为空（或未携带）时不渲染该块、不占位。
 * testid 逐字沿用旧锚（L=LibrarySessionsPanel）：library-message-<id>（L2180/L2190）、library-relay-tab（L2227）。
 * 本文件不取数、不判断产品、不读时钟、不发命令；文案取 config 文案表与 relayLegs 的 legMetaText。
 */
import { Bot } from "lucide-react";
import { LIBRARY_TEXTS, type MessageView, type RelayLegView } from "../config";
import { legMetaText } from "../relayLegs";

/** 消息行可携带轨迹展开列表（K-08）：config 的 MessageView 未建模该字段，TRAE 侧以 TraeMessageView 补齐。 */
export interface MessageFlowItem extends MessageView { readonly thoughts?: readonly string[] }

/** 账号色块头像（L2462-2469）：圆形 + 账号色底 + 首字母；消息流与接力时间线共用。 */
function AccountAvatar({ name, colorIndex }: { readonly name: string | null; readonly colorIndex: number }) {
  const letter = (name ?? "").trim().charAt(0).toUpperCase() || "?";
  return <span className={`account-avatar account-avatar--c${colorIndex}`} aria-hidden="true">{letter}</span>;
}

/** 消息正文（L2200-2216）：正文 + 轨迹展开列表；空数组不渲染 <ul>，不留占位。 */
function MessageBody({ message }: { readonly message: MessageFlowItem }) {
  const thoughts = message.thoughts ?? [];
  return (
    <div className={`chat-msg__text${message.role === "trace" ? " chat-msg__text--trace" : ""}`}>
      {message.text}
      {thoughts.length > 0 && (
        <ul className="chat-msg__thoughts">
          {thoughts.map((thought, index) => <li key={index}>{thought}</li>)}
        </ul>
      )}
    </div>
  );
}

/** 单条消息行（L2166-2196）：user 右对齐带账号色头像与账号名；assistant / trace 左对齐中性色机器人图标。 */
export function MessageRow({ message }: { readonly message: MessageFlowItem }) {
  // 旧锚 library-message-<id> 由 builder 写进 testId；空串（纯派生层）按同一规则回退，锚点不漂移。
  const testId = message.testId !== "" ? message.testId : `library-message-${message.id}`;
  if (message.role === "user") {
    const name = message.accountName ?? LIBRARY_TEXTS.removedAccount;
    return (
      <div className="chat-msg chat-msg--user" data-testid={testId}>
        <div className="chat-msg__side">
          <AccountAvatar name={name} colorIndex={message.colorIndex} />
          <span className="chat-msg__name">{name}</span>
        </div>
        <div className="chat-msg__bubble"><MessageBody message={message} /></div>
      </div>
    );
  }
  return (
    <div className="chat-msg chat-msg--assistant" data-testid={testId}>
      <span className="chat-msg__bot" aria-hidden="true"><Bot size={14} strokeWidth={1.8} /></span>
      <div className="chat-msg__bubble"><MessageBody message={message} /></div>
    </div>
  );
}

/** 消息流：按视图次序平铺（不排序、不分组）；分页提示条归查看器壳（SessionViewer）。 */
export function MessageFlow({ messages }: { readonly messages: readonly MessageFlowItem[] }) {
  return <>{messages.map((message) => <MessageRow key={message.id} message={message} />)}</>;
}

/** 接力时间线（L2219-2259）：提示句 + 逐腿（头像 + 时间段·新增消息数 + 当前腿徽章）。 */
export function RelayTimeline({ legs }: { readonly legs: readonly RelayLegView[] }) {
  return (
    <div className="viewer-relay" data-testid="library-relay-tab">
      <p className="viewer-relay__hint">{LIBRARY_TEXTS.relayHint}</p>
      <div className="leg-timeline">
        {legs.map((leg, index) => (
          <div className="leg-step" key={`${leg.userId}-${index}`}>
            <div className="leg-step__rail">
              <span className="leg-step__dot"><AccountAvatar name={leg.accountName} colorIndex={leg.colorIndex} /></span>
              <span className="leg-step__line" />
            </div>
            <div className="leg-step__body">
              <div className="leg-step__name">
                {leg.accountName}
                {leg.current && <span className="leg-step__badge">{LIBRARY_TEXTS.currentLeg}</span>}
              </div>
              <div className="leg-step__meta">{legMetaText(leg)}</div>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
