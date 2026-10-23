import { Fragment, type ReactNode } from "react";
import { ArrowLeft, Cpu, RefreshCw } from "lucide-react";
import { Badge } from "../../../ui/Badge";
import { SectionHeader } from "../../../ui/SectionHeader";
import type { AccountActionKind, AccountDetailFieldView, DetailActionView, DetailFieldView, DetailSectionView, DetailViewModel } from "../config";
import { DetailDangerAction } from "./DetailDangerAction";
import { DetailEditableField } from "./DetailEditableField";

/**
 * 账号详情页（P4c-7b，纯新增、未接线）：**一个**通用渲染器覆盖两产品的全部详情。
 * 分区顺序、标题、字段、动作、testid 全部来自 buildDetailView 产出的 DetailViewModel：
 * 本文件不拼 testid、不自创文案、不判断 productId、不读数据、不发命令。
 * 三种特例（tech 折叠 / danger 二次确认 / checkin 开关）只在渲染层分支，数据面零差异。
 *
 * P4c-6b-2e-2 增：行内编辑字段（field.edit 非空）与开关字段（field.toggle 非空）两种渲染分支；
 * 编辑与开关的保存出口只有 onFieldSave / onFieldToggle（由页面注入，命令仍在接缝表）。
 */
export interface AccountDetailPageProps {
  view: DetailViewModel;
  /** 进行中的动作 id（builder 已算好 disabledReason；此处只补忙碌态与转圈图标）。 */
  busy?: readonly string[];
  /** 返回账号列表（页头 account-detail-back）。 */
  onBack(): void;
  /** 分区内动作统一出口：动作 id 原样透出，语义归页面。 */
  onAction(id: string): void;
  /**
   * 行内编辑保存出口（P4c-6b-2e-2）：fieldId + trim 后的草稿；true = 成功退出编辑态。
   * 缺省 = 本产品未登记该字段命令，保存恒 false（字段保持只读展示，不臆造写入）。
   */
  onFieldSave?: (fieldId: string, value: string) => Promise<boolean>;
  /**
   * 开关字段翻转出口（P4c-6b-2e-2）：fieldId + 目标态；缺省 = 未登记命令，开关禁用。
   */
  onFieldToggle?: (fieldId: string, next: boolean) => void;
  /** 开关字段的乐观覆盖态（字段 id → 目标态）；未覆盖即用 builder 的权威态。 */
  toggleOverrides?: Readonly<Record<string, boolean>>;
  /**
   * 登录设备插槽（P4c-7c-1）：旧 AccountDetail 把 RemoteDevicesPanel 内嵌在
   * 「操作区之后、危险区之前」（L488-497），本渲染器同样插在该位置。
   * 缺省 = 本产品无设备能力（WB devices=false），不占位也不臆造。
   */
  devicesSlot?: ReactNode;
  /**
   * 弹层插槽（P4c-7c-1）：详情页的二次确认弹层必须渲染在 region 内部
   * （旧 AccountDetail L532-543 同位置），e2e 以「账号详情」region 为作用域取弹层。
   */
  dialogsSlot?: ReactNode;
  /**
   * 危险动作的确认文案（P4c-6b-2e-1）：旧实现的删除确认正文是逐字的「确定删除账号“X”吗？…」，
   * 不是危险区 hint 本身。缺省 = 沿用 action.label + hint（未接线路径与既有测试一字不改）。
   */
  dangerConfirm?: (actionId: string) => DangerConfirmCopy | null;
}

/** 危险动作确认文案（与 shared/ConfirmDialog 的入参同形，只读展示，不含执行语义）。 */
export interface DangerConfirmCopy {
  readonly title: string;
  readonly lines: readonly string[];
  readonly confirmLabel: string;
  readonly busyLabel?: string;
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
/** 字段值收窄：value=null = 本产品不渲染该行（config 契约）；tone/hint/edit/toggle 是可选附加信息。 */
type RenderableField = AccountDetailFieldView & Partial<DetailFieldView> & { readonly value: string };
const isRenderable = (field: AccountDetailFieldView): field is RenderableField => field.value !== null;

interface FieldRowProps {
  readonly field: RenderableField;
  readonly busy: readonly string[];
  readonly onFieldSave: (fieldId: string, value: string) => Promise<boolean>;
  readonly onFieldToggle: (fieldId: string, next: boolean) => void;
  readonly overrides: Readonly<Record<string, boolean>>;
}

/** 字段行三种形态：开关（toggle）/ 行内编辑（edit）/ 只读文本；保存出口一律是页面注入的回调。 */
function FieldRow({ field, busy, onFieldSave, onFieldToggle, overrides }: FieldRowProps) {
  // 开关字段（P4c-6b-2e-2）：on = 乐观覆盖优先，否则 builder 权威态；文案由 onLabel/offLabel 换。
  if (field.toggle !== undefined) {
    const on = overrides[field.id] ?? field.toggle.on;
    return (
      <div className="account-detail__fact">
        <dt>{field.label}</dt>
        <dd>
          {/* testid 落在 label 上（与旧实现同位置：点文字也可切换）。 */}
          <label className="account-center__profile-select" data-testid={field.id}>
            <input
              type="checkbox"
              checked={on}
              disabled={busy.includes(field.id)}
              onChange={() => onFieldToggle(field.id, !on)}
            />
            <span className="account-center__profile-copy">
              <strong>{on ? field.toggle.onLabel : field.toggle.offLabel}</strong>
            </span>
          </label>
        </dd>
      </div>
    );
  }
  // 行内编辑字段：展示态点行进入编辑态（DetailEditableField 自持草稿与编辑态）。
  if (field.edit !== undefined) {
    return (
      <DetailEditableField
        id={field.id}
        label={field.label}
        value={field.value}
        edit={field.edit}
        busy={busy.includes(field.id)}
        onSave={(value) => onFieldSave(field.id, value)}
      />
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
  readonly onFieldSave: (fieldId: string, value: string) => Promise<boolean>;
  readonly onFieldToggle: (fieldId: string, next: boolean) => void;
  readonly overrides: Readonly<Record<string, boolean>>;
  readonly dangerConfirm?: (actionId: string) => DangerConfirmCopy | null;
}

/** 单个分区：字段表 + 动作组 + 可选说明；tech / danger 两个特例在此分流。 */
function SectionBlock({ section, busy, onAction, onFieldSave, onFieldToggle, overrides, dangerConfirm }: SectionBlockProps) {
  const fields = section.fields.filter(isRenderable);
  // 行内编辑字段的保存按钮由 DetailEditableField 自持（编辑态才出现，testid = 字段 id + "-save"）；
  // 分区动作里同 id 的保存动作据此过滤，避免 DOM 里出现两个同锚点按钮。
  const editSaveIds = new Set(fields.filter((field) => field.edit !== undefined).map((field) => `${field.id}-save`));
  const actions = section.actions.filter((action) => !editSaveIds.has(action.id));
  const facts = fields.length === 0 ? null : (
    <dl className="account-detail__facts">
      {fields.map((field) => (
        <FieldRow key={field.id} field={field} busy={busy}
          onFieldSave={onFieldSave} onFieldToggle={onFieldToggle} overrides={overrides} />
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
                confirm={dangerConfirm?.(action.id) ?? undefined}
              />
            ))}
          </div>
        </div>
        {facts}
      </section>
    );
  }
  // 常规分区（basics / credential / checkin / actions）：标题 + 字段 + 动作 + 说明。
  // 行内编辑字段的保存按钮由 DetailEditableField 自持（编辑态才出现），故此处动作组只渲染其余动作。
  return (
    <section className="account-detail__section">
      <SectionHeader title={section.title} className="account-detail__section-heading" />
      {facts}
      {actions.length > 0 && (
        <div className="account-detail__actions">
          {actions.map((action) => (
            <ActionRow key={action.id} action={action} running={busy.includes(action.id)} onAction={onAction} />
          ))}
        </div>
      )}
      {section.hint !== null && <p className="account-detail__hint">{section.hint}</p>}
    </section>
  );
}

/** 详情页装配：页头（返回 + 标题 + 徽章组）+ 按 sections[] 顺序渲染分区。 */
export function AccountDetailPage({
  view, busy, onBack, onAction, onFieldSave, onFieldToggle, toggleOverrides, devicesSlot, dialogsSlot, dangerConfirm,
}: AccountDetailPageProps) {
  // 忙碌位优先级：页面显式传入 > builder 镜像的 view.busy > 空闲（不猜进行中的动作）。
  const running = busy ?? view.busy ?? [];
  // 未接线路径（未传保存出口）：保存恒 false —— 字段保持只读展示，不假装写入成功。
  const saveField = onFieldSave ?? (() => Promise.resolve(false));
  const toggleField = onFieldToggle ?? (() => undefined);
  const overrides = toggleOverrides ?? {};
  // 设备插槽落位：危险区之前（旧布局 L488-497）；无危险区则垫在分区末尾。
  const dangerAt = view.sections.findIndex((section) => section.id === "danger");
  const devicesAt = devicesSlot === undefined ? -1 : dangerAt < 0 ? view.sections.length : dangerAt;
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
      {view.sections.map((section, index) => (
        <Fragment key={section.id}>
          {index === devicesAt && devicesSlot}
          <SectionBlock section={section} busy={running} onAction={onAction} onFieldSave={saveField}
            onFieldToggle={toggleField} overrides={overrides} dangerConfirm={dangerConfirm} />
        </Fragment>
      ))}
      {devicesAt === view.sections.length && devicesSlot}
      {dialogsSlot}
    </section>
  );
}
