import { ArrowLeft, Cpu, RefreshCw } from "lucide-react";
import { Badge } from "../../../ui/Badge";
import { SectionHeader } from "../../../ui/SectionHeader";
import type { AccountActionKind, AccountDetailFieldView, DetailActionView, DetailFieldView, DetailSectionView, DetailViewModel } from "../config";
import { DetailDangerAction } from "./DetailDangerAction";

/**
 * 账号详情页（P4c-7b，纯新增、未接线）：**一个**通用渲染器覆盖两产品的全部详情。
 * 分区顺序、标题、字段、动作、testid 全部来自 buildDetailView 产出的 DetailViewModel：
 * 本文件不拼 testid、不自创文案、不判断 productId、不读数据、不发命令。
 * 三种特例（tech 折叠 / danger 二次确认 / checkin 开关）只在渲染层分支，数据面零差异。
 */
export interface AccountDetailPageProps {
  view: DetailViewModel;
  /** 进行中的动作 id（builder 已算好 disabledReason；此处只补忙碌态与转圈图标）。 */
  busy?: readonly string[];
  /** 返回账号列表（页头 account-detail-back）。 */
  onBack(): void;
  /** 分区内动作统一出口：动作 id 原样透出，语义归页面。 */
  onAction(id: string): void;
}

/** 动作色档：与页头同一套按钮族（primary / 次要描边 / 危险）。 */
const KIND_CLASS: Record<AccountActionKind, string> = {
  primary: "btn btn--primary",
  ghost: "btn",
  danger: "btn btn--danger",
};
/** 字段 tone → 文字色：neutral 走默认，warn=琥珀、danger=红（均为既有样式类，不新增 CSS）。 */
const TONE_CLASS: Record<"neutral" | "warn" | "danger", string | undefined> = {
  neutral: undefined,
  warn: "account-detail__fact-warn",
  danger: "workbench__error",
};
/** 开关态从声明文案读（builder 两产品同词表）：开态以「参与每日」起头，其余按关态。 */
const SWITCH_ON_PREFIX = "参与每日";
const switchOn = (value: string): boolean => value.startsWith(SWITCH_ON_PREFIX);
/** 字段值收窄：value=null = 本产品不渲染该行（config 契约）；tone/hint 是可选附加信息。 */
type RenderableField = AccountDetailFieldView & Partial<DetailFieldView> & { readonly value: string };
const isRenderable = (field: AccountDetailFieldView): field is RenderableField => field.value !== null;
/** 特例 3 的判定：checkin 区内 id 以 -toggle 结尾且无警示 tone 的字段 = 开关型动作。 */
const isSwitchField = (sectionId: DetailSectionView["id"], field: RenderableField): boolean =>
  sectionId === "checkin" && field.id.endsWith("-toggle") && field.tone === undefined;

interface FieldRowProps {
  readonly field: RenderableField;
  readonly sectionId: DetailSectionView["id"];
  readonly busy: readonly string[];
  readonly onAction: (id: string) => void;
}

/** 字段行：label + 字符串 value；tone 只改文字色，hint 进悬浮说明，开关型字段渲染成开关。 */
function FieldRow({ field, sectionId, busy, onAction }: FieldRowProps) {
  if (isSwitchField(sectionId, field)) {
    return (
      <div className="account-detail__fact">
        <dt>{field.label}</dt>
        <dd>
          {/* testid 落在 label 上（与旧实现同位置：点文字也可切换）。 */}
          <label className="account-center__profile-select" data-testid={field.id}>
            <input
              type="checkbox"
              checked={switchOn(field.value)}
              disabled={busy.includes(field.id)}
              onChange={() => onAction(field.id)}
            />
            <span className="account-center__profile-copy">
              <strong>{field.value}</strong>
            </span>
          </label>
        </dd>
      </div>
    );
  }
  return (
    <div className="account-detail__fact">
      <dt>{field.label}</dt>
      <dd data-testid={field.id} title={field.hint} className={TONE_CLASS[field.tone ?? "neutral"]}>
        {field.value}
      </dd>
    </div>
  );
}

interface ActionRowProps {
  readonly action: DetailActionView;
  readonly running: boolean;
  readonly onAction: (id: string) => void;
}

/** 动作按钮：kind 决定色档；disabledReason 非空即禁用并给悬浮说明（与页头同口径）。 */
function ActionRow({ action, running, onAction }: ActionRowProps) {
  return (
    <button
      className={KIND_CLASS[action.kind]}
      type="button"
      onClick={() => onAction(action.id)}
      disabled={action.disabledReason !== undefined || running}
      title={action.disabledReason}
      aria-busy={running || undefined}
      data-testid={action.id}
    >
      {running && <RefreshCw size={15} className="icon-spin" aria-hidden="true" />}
      {action.label}
    </button>
  );
}

interface SectionBlockProps {
  readonly section: DetailSectionView;
  readonly busy: readonly string[];
  readonly onAction: (id: string) => void;
}

/** 单个分区：字段表 + 动作组 + 可选说明；tech / danger 两个特例在此分流。 */
function SectionBlock({ section, busy, onAction }: SectionBlockProps) {
  const fields = section.fields.filter(isRenderable);
  const facts = fields.length === 0 ? null : (
    <dl className="account-detail__facts">
      {fields.map((field) => (
        <FieldRow key={field.id} field={field} sectionId={section.id} busy={busy} onAction={onAction} />
      ))}
    </dl>
  );
  // 特例 1：技术细节默认折叠（<details>）；标题由 summary 承担，字段 testid 原样来自 view。
  if (section.id === "tech") {
    return (
      <details className="account-detail__tech">
        <summary>
          <Cpu size={14} aria-hidden="true" />
          {section.title}
        </summary>
        {facts}
      </details>
    );
  }
  // 特例 2：危险分区——危险色边框/标题 + 影响范围说明 + 每个动作点击先二次确认。
  if (section.id === "danger") {
    return (
      <section className="account-detail__section account-detail__section--danger">
        <SectionHeader title={section.title} className="account-detail__section-heading" />
        <div className="account-detail__danger-row">
          {section.hint !== null && <p className="account-detail__danger-hint">{section.hint}</p>}
          <div className="account-detail__actions">
            {section.actions.map((action) => (
              <DetailDangerAction
                key={action.id}
                action={action}
                hint={section.hint}
                busy={busy.includes(action.id)}
                onAction={onAction}
              />
            ))}
          </div>
        </div>
        {facts}
      </section>
    );
  }
  // 常规分区（basics / credential / checkin / actions）：标题 + 字段 + 动作 + 说明。
  return (
    <section className="account-detail__section">
      <SectionHeader title={section.title} className="account-detail__section-heading" />
      {facts}
      {section.actions.length > 0 && (
        <div className="account-detail__actions">
          {section.actions.map((action) => (
            <ActionRow key={action.id} action={action} running={busy.includes(action.id)} onAction={onAction} />
          ))}
        </div>
      )}
      {section.hint !== null && <p className="account-detail__hint">{section.hint}</p>}
    </section>
  );
}

/** 详情页装配：页头（返回 + 标题 + 徽章组）+ 按 sections[] 顺序渲染分区。 */
export function AccountDetailPage({ view, busy, onBack, onAction }: AccountDetailPageProps) {
  // 忙碌位优先级：页面显式传入 > builder 镜像的 view.busy > 空闲（不猜进行中的动作）。
  const running = busy ?? view.busy ?? [];
  return (
    <section className="account-detail" role="region" aria-label="账号详情">
      <header className="page-header">
        <div className="page-header__copy">
          {/* 返回锚点逐字沿用旧详情页（account-detail-back）。 */}
          <button className="btn btn--quiet" type="button" onClick={onBack} data-testid="account-detail-back">
            <ArrowLeft size={15} aria-hidden="true" />
            返回账号列表
          </button>
          <h1 data-page-title="accounts" tabIndex={-1}>{view.header.title}</h1>
          {view.header.subtitle !== undefined && <p className="account-detail__hint">{view.header.subtitle}</p>}
        </div>
        {/* 徽章组：文案与色档来自 builder（与列表行同源），此处不增删。 */}
        <div className="page-header__actions">
          {view.header.badges.map((badge, index) => (
            <Badge key={`${badge.text}-${index}`} tone={badge.tone}>{badge.text}</Badge>
          ))}
        </div>
      </header>
      {view.sections.map((section) => (
        <SectionBlock key={section.id} section={section} busy={running} onAction={onAction} />
      ))}
    </section>
  );
}
