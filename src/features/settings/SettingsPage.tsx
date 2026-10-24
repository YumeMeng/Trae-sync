import { ConfirmDialog } from "../../components/shared/ConfirmDialog";
import { settingsCapabilitiesFor, type SettingsView } from "./config";
import { settingsViewFor, type SettingsSnapshot } from "./registry";
import { AutoCheckinSection } from "./components/AutoCheckinSection";
import { BackupSection } from "./components/BackupSection";
import { BackupLocationDialog, BackupRestoreDialog } from "./components/BackupDialogs";
import { useTraeSettings } from "./components/useTraeSettings";
import { useWorkBuddySettings } from "./components/useWorkBuddySettings";
import type { SettingsPageData } from "./components/useSettings";
import type { SettingsActions } from "./components/settingsTypes";

/**
 * 设置页装配（P4f-5）：页头 + 签到区 + 备份区 + 弹层（位置 / 恢复 / 单次确认）。
 * 数据由 useTraeSettings / useWorkBuddySettings 提供（命令经 platform/settings），视图模型由
 * registry.settingsViewFor 查表产出；本文件零产品相等判断、零 invoke(、零命令名、零自创文案——
 * 标题、锚点、禁用原因一律取自 SettingsView，动作 id 由各 hook 的 busyIds 注入。
 * 两产品差异全部在 builder 与能力表：签到参与清单、备份七项能力、弹层文案都按视图/能力出现。
 */

/** 每产品允许读取的 hook 开关（新增产品只加一行；页面不写 productId 相等判断）。 */
const READ_GATES: Record<string, { readonly trae: boolean; readonly workbuddy: boolean }> = {
  trae_cn: { trae: true, workbuddy: false },
  workbuddy: { trae: false, workbuddy: true },
};
/** 未登记产品：两路都不读（fail-closed，页面按 registry 空视图渲染）。 */
const NO_GATES = { trae: false, workbuddy: false } as const;
/** 未登记产品的动作出口：全部空实现（不猜命令，也不假装能操作）。 */
const NO_ACTIONS: SettingsActions = {
  toggleAuto: () => undefined, changeTime: () => undefined, toggleParticipation: () => undefined,
  participateAll: () => undefined, createBackup: () => undefined, startRestore: () => undefined,
  toggleSelect: () => undefined, deleteSelected: () => undefined, openLocation: () => undefined,
  toggleCredentials: () => undefined, changeRetention: () => undefined, retry: () => undefined,
};
/** 未登记产品无数据面：空快照（无弹层、无忙碌位）。 */
const NO_DATA: SettingsPageData = {
  trae: null, workbuddy: null, actions: NO_ACTIONS, location: null, restore: null, confirm: null, busyIds: [],
};

export interface SettingsPageProps {
  /** 页面可见时才读取设置，避免后台 IPC（旧 TraeSettingsVariant 口径，WB 一并取齐）。 */
  readonly active: boolean;
  /** 当前产品协议值：只用于查读取门与视图登记表，页面内不做任何产品判断。 */
  readonly productId: string;
}

export function SettingsPage({ active, productId }: SettingsPageProps) {
  // 两路 hook 恒被调用（React 顺序稳定），可见性由读取门表决定：非当前产品恒 false，不发 IPC。
  const gates = READ_GATES[productId] ?? NO_GATES;
  const trae = useTraeSettings(active && gates.trae);
  const workbuddy = useWorkBuddySettings(active && gates.workbuddy);
  const data: SettingsPageData = gates.trae ? trae : gates.workbuddy ? workbuddy : NO_DATA;
  // 快照判别字段与载荷字段同名同值（registry 契约）：按数据实际产出选一支，不按 productId 判断。
  const snapshot: SettingsSnapshot = data.trae !== null
    ? { productId: "trae_cn", trae: data.trae }
    : data.workbuddy !== null
      ? { productId: "workbuddy", workbuddy: data.workbuddy }
      : { productId } as unknown as SettingsSnapshot;
  const view: SettingsView = settingsViewFor(snapshot);
  const actions = data.actions;
  const restore = data.restore;
  // 恢复弹层的 stamp 展示文案取对应链行（两产品时间格式不同，页面不重复格式化）。
  const stampLabel = restore === null ? "" : view.backup?.rows
    .find((row) => row.stampUnixSeconds === restore.stamp)?.stampLabel ?? "";
  return (
    <section className="settings-panel" role="region" aria-label={view.header.regionLabel}>
      <header className="page-header">
        <div className="page-header__copy">
          <span className="page-header__eyebrow">{view.header.eyebrow}</span>
          <h1 data-page-title={view.header.pageTitleAttr} tabIndex={-1}>{view.header.title}</h1>
        </div>
      </header>

      {/* 签到区（旧 L269-316 / WBA）：分区标题由组件自带，页面只按视图有无决定渲染。 */}
      {view.autoCheckin !== null && (
        <AutoCheckinSection view={view.autoCheckin} onToggle={actions.toggleAuto}
          onTimeChange={actions.changeTime} onParticipationChange={actions.toggleParticipation}
          onParticipateAll={actions.participateAll} />
      )}

      {/* 备份区（旧 L318-451 / WB L103-207）：TRAE 链未读到即整区消失（视图 null），WB 壳恒在。 */}
      <BackupSection view={view.backup} busy={data.busyIds} onCreate={actions.createBackup}
        onRestore={actions.startRestore} onToggleSelect={actions.toggleSelect}
        onDeleteSelected={actions.deleteSelected} onChangeLocation={actions.openLocation}
        onToggleCredentials={actions.toggleCredentials} onRetentionChange={actions.changeRetention}
        onRetry={actions.retry} />

      {/* 位置变更弹层（TRAE 独有，旧 L452-504）：WB 无该能力，data.location 恒 null。 */}
      {data.location !== null && (
        <BackupLocationDialog value={data.location.value} busy={data.location.busy}
          onChange={data.location.onChange} onCancel={data.location.onCancel}
          onConfirm={data.location.onConfirm} />
      )}

      {/* 恢复确认弹层（两产品两套文案，旧 L505-524 / WB L227-244）：能力表 safetyChain 决定文案与锚点。 */}
      {restore !== null && (
        <BackupRestoreDialog stampUnixSeconds={restore.stamp} stampLabel={stampLabel}
          capabilities={settingsCapabilitiesFor(view.productId).backup} busy={restore.busy}
          onCancel={restore.onCancel} onConfirm={restore.onConfirm} />
      )}

      {/* 创建/删除单次确认（WB 独有，旧 WB L210-262）：TRAE 无确认层，data.confirm 恒 null。 */}
      {data.confirm !== null && (
        <ConfirmDialog title={data.confirm.title} lines={data.confirm.lines}
          confirmLabel={data.confirm.confirmLabel} busyLabel={data.confirm.busyLabel}
          busy={data.confirm.busy} danger={data.confirm.danger} onCancel={data.confirm.onCancel}
          onConfirm={data.confirm.onConfirm} testId={data.confirm.testId} />
      )}
    </section>
  );
}
