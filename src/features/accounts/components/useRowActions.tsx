import { useCallback, useRef, useState, type ReactNode } from "react";
import { MasterSwitchDialog, type MasterSwitchTarget } from "../../../components/MasterSwitchDialog";
import { notify } from "../../../platform/notify";
import type { WorkBuddySwitchDto, WorkBuddySwitchPreviewDto } from "../../../types/product";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import { WorkBuddySwitchDialog } from "./WorkBuddySwitchDialog";

/**
 * 行内切号接线（P4c-6b-2d）：账号页的行内动作分发 + 三种切号形态的弹层/忙碌位。
 *
 * 三种形态按产品登记（缺省即 fail-closed，动作 id 只透出给外部观察者）：
 * - master（TRAE 官方切号）：只打开 MasterSwitchDialog——五步事务、备份、插件同步、
 *   force_reset 救援全在组件内，本 hook 一行未改其语义（旧 AccountCenterTrae L1191-1195 同法）。
 * - tool（TRAE 工具账号选择 ≠ 官方切号）：直接调 switch_trae_cn_account，成功后重读缓存。
 * - workbuddy（预览 → 单次确认 → 写凭据 + 过户 + 冷启动）：旧 WorkBuddyPanel L215-283 两段式。
 * 文案逐字沿用旧实现，命令与产品差异全由接缝表注入，本文件零产品判断、零 invoke(。
 */

/** 行内切号目标（按行 id 从本产品数据面解析）：展示名给弹层与通知，accountId 给按 uid 的命令。 */
export interface RowSwitchTarget {
  readonly profileId: string;
  readonly displayName: string;
  /** 产品命令按账号 uid 的场合（WB）给出；TRAE 为 null。 */
  readonly accountId: string | null;
}

/** TRAE 官方切号：动作 id 前缀（account-switch-<profileId>）。 */
export interface MasterSwitchSeam { readonly actionIdPrefix: string }

/** TRAE 工具账号选择：精确动作 id + 命令 + 三条旧文案（旧 L600-619）。 */
export interface ToolSelectSeam {
  readonly actionId: string;
  readonly select: (profileId: string) => Promise<unknown>;
  readonly successCopy: (displayName: string) => string;
  readonly failureCopy: string;
  readonly refreshFailedCopy: string;
}

/** WB 切号：动作 id 前缀（workbuddy-switch-<profileId>）+ 预览/执行命令 + 通知前缀。 */
export interface WorkBuddySwitchSeam {
  readonly actionIdPrefix: string;
  readonly preview: (accountId: string) => Promise<WorkBuddySwitchPreviewDto>;
  readonly run: (accountId: string, confirmed: boolean) => Promise<WorkBuddySwitchDto>;
  readonly notifyPrefix: string;
}

/** 产品接缝：只登记本产品真实存在的切号形态；targetOf 缺目标即当未登记（不猜数据）。 */
export interface PageRowSwitch {
  readonly targetOf: (payload: unknown, rowId: string) => RowSwitchTarget | null;
  readonly master?: MasterSwitchSeam;
  readonly tool?: ToolSelectSeam;
  readonly workbuddy?: WorkBuddySwitchSeam;
}

export interface RowActionsInput {
  /** 本产品的行内切号接缝；undefined = 未登记（所有行内动作只透出）。 */
  readonly wiring: PageRowSwitch | undefined;
  /** 本产品数据面（延迟到动作触发时取：合成后的快照含适配器面，行目标按它解析）。 */
  readonly readPayload: () => unknown;
  /** 成功后重读本机缓存（旧实现每个动作后 refreshOverview / refresh 同法）。 */
  readonly onSettled: () => Promise<void>;
  /** 透出契约不丢：动作 id 仍回给外部观察者（与页头动作同一口径）。 */
  readonly onRowAction?: (rowId: string, actionId: string) => void;
}

export interface RowActionsResult {
  /** 三路忙碌位（builder 据此禁用行内按钮并给原因）。 */
  readonly busy: { readonly switching: boolean; readonly toolSelection: boolean; readonly preview: boolean };
  /** 正在执行的行内动作 id 与行 id：仅该行该按钮转圈（池面多行共用同一动作 id，故要配行）。 */
  readonly busyActionId: string | null;
  readonly busyRowId: string | null;
  readonly handle: (rowId: string, actionId: string) => void;
  /** 弹层槽：官方切号进度弹层 + WB 切号确认弹层（未打开即不渲染）。 */
  readonly dialogs: ReactNode;
}

export function useRowActions(input: RowActionsInput): RowActionsResult {
  // 入参经 ref 取最新值：handle / 异步收尾保持稳定身份，页面重建不换引用。
  const inputRef = useRef(input);
  inputRef.current = input;
  const [masterTarget, setMasterTarget] = useState<MasterSwitchTarget | null>(null);
  // 工具账号选择进行中的 profile_id（非空即该行按钮转圈、其余行禁用）。
  const [toolPending, setToolPending] = useState<string | null>(null);
  const [wbTarget, setWbTarget] = useState<RowSwitchTarget | null>(null);
  const [wbPreview, setWbPreview] = useState<WorkBuddySwitchPreviewDto | null>(null);
  // WB 两段式忙碌位：preview=取影响面 / run=写凭据并过户；以 ref 为准，同 tick 重复点击也拦得住。
  const [wbBusy, setWbBusy] = useState<"preview" | "run" | null>(null);
  const wbRef = useRef(false);
  const toolRef = useRef(false);

  // 工具账号选择（旧 L600-619 逐字）：成功先重读缓存再发成功回执；提交后刷新失败只警告，
  // 不把已完成的选择谎报成失败。
  const runTool = useCallback(async (target: RowSwitchTarget) => {
    const seam = inputRef.current.wiring?.tool;
    if (seam === undefined || toolRef.current) return;
    toolRef.current = true;
    setToolPending(target.profileId);
    let committed = false;
    try {
      await seam.select(target.profileId);
      committed = true;
      await inputRef.current.onSettled();
      notify.success(seam.successCopy(target.displayName));
    } catch (reason: unknown) {
      if (committed) notify.warning(seam.refreshFailedCopy);
      else notify.error(safeUiErrorMessage(reason, seam.failureCopy));
    } finally {
      toolRef.current = false;
      setToolPending(null);
    }
  }, []);

  // WB 切号意图（旧 L220-236）：先取影响面预览；预览失败不开弹层（如实报错比降级弹窗诚实）。
  const runPreview = useCallback(async (target: RowSwitchTarget) => {
    const seam = inputRef.current.wiring?.workbuddy;
    if (seam === undefined || target.accountId === null || wbRef.current) return;
    wbRef.current = true;
    setWbTarget(target);
    setWbPreview(null);
    setWbBusy("preview");
    try {
      setWbPreview(await seam.preview(target.accountId));
    } catch (reason: unknown) {
      setWbTarget(null);
      notify.error(`${seam.notifyPrefix}切号预览未完成——${safeUiErrorMessage(reason, "请确认客户端已登录后重试。")}`);
    } finally {
      wbRef.current = false;
      setWbBusy(null);
    }
  }, []);

  // 行内动作分发：命中接缝即在本页执行，其余一律原样透出（未登记产品 fail-closed）。
  const handle = useCallback((rowId: string, actionId: string) => {
    const { wiring, readPayload, onRowAction } = inputRef.current;
    const target = wiring === undefined ? null : wiring.targetOf(readPayload(), rowId);
    if (wiring !== undefined && target !== null) {
      if (wiring.master !== undefined && actionId.startsWith(wiring.master.actionIdPrefix)) {
        setMasterTarget({ profile_id: target.profileId, display_name: target.displayName });
      } else if (wiring.tool !== undefined && actionId === wiring.tool.actionId) {
        void runTool(target);
      } else if (wiring.workbuddy !== undefined && target.accountId !== null && actionId.startsWith(wiring.workbuddy.actionIdPrefix)) {
        void runPreview(target);
      }
    }
    onRowAction?.(rowId, actionId);
  }, [runPreview, runTool]);

  // WB 切号执行（旧 L238-283 逐字）：单次确认后写凭据并过户；过户失败不回滚切号（警告明示）。
  const runSwitch = useCallback(async () => {
    const seam = inputRef.current.wiring?.workbuddy;
    if (seam === undefined || wbTarget === null || wbTarget.accountId === null || wbBusy !== null) return;
    setWbBusy("run");
    try {
      const receipt = await seam.run(wbTarget.accountId, true);
      // 防御分支：后端约定失败走 Err，switched=false 仍如实报错不伪装成功。
      if (!receipt.switched) throw new Error("workbuddy_switch_verify_failed");
      const transferText = receipt.transfer_executed === true
        ? `历史会话过户完成（${receipt.updated_rows ?? 0} 条本地会话已随行）`
        : "无待过户会话";
      const cloudText = (receipt.cloud_residual_rows ?? 0) > 0
        ? `；原账号 ${receipt.cloud_residual_rows} 条云端会话不随行（不碰云端数据）`
        : "";
      const missing = receipt.missing_body_files ?? 0;
      notify.success(
        `${seam.notifyPrefix}已切换登录到「${receipt.target_label}」，${transferText}${cloudText}。`,
        missing > 0 ? `其中 ${missing} 条会话正文文件缺失，可到库页复核。` : undefined,
      );
      if (receipt.transfer_error) {
        notify.warning(`切号成功，但历史会话过户失败（${receipt.transfer_error}）。`,
          "会话归属可稍后重试过户兜底恢复，切号本身不受影响。");
      }
      if (receipt.post_status && receipt.post_status !== "active") {
        notify.info(`客户端冷启动后登录态为 ${receipt.post_status}；如客户端提示重新登录，属切号预期行为。`);
      }
      setWbTarget(null);
      setWbPreview(null);
      await inputRef.current.onSettled();
    } catch (reason: unknown) {
      notify.error(`${seam.notifyPrefix}切换未完成——${safeUiErrorMessage(reason, "客户端登录态未被改动，请稍后重试。")}`);
    } finally {
      setWbBusy(null);
    }
  }, [wbBusy, wbTarget]);

  const { wiring } = inputRef.current;
  // 正在执行的行内动作 id（该按钮转圈）：工具选择 / WB 两段式（预览与执行都算）/ 官方切号弹层期间。
  const busyActionId = toolPending !== null
    ? wiring?.tool?.actionId ?? null
    : wbTarget !== null && wbBusy !== null
      ? `${wiring?.workbuddy?.actionIdPrefix ?? ""}${wbTarget.profileId}`
      : masterTarget !== null ? `${wiring?.master?.actionIdPrefix ?? ""}${masterTarget.profile_id}` : null;
  // 忙碌位所属行：与 busyActionId 一起定位唯一按钮（工具选择的动作 id 全池同名）。
  const busyRowId = toolPending ?? (wbTarget !== null && wbBusy !== null ? wbTarget.profileId : masterTarget?.profile_id ?? null);

  // 弹层槽：官方切号组件自持 target=null 即不渲染；WB 确认弹层需要预览先到。
  const dialogs = (
    <>
      <MasterSwitchDialog
        target={masterTarget}
        onFinished={inputRef.current.onSettled}
        onClose={() => setMasterTarget(null)}
      />
      {wbTarget !== null && wbPreview !== null && (
        <WorkBuddySwitchDialog
          preview={wbPreview}
          busy={wbBusy === "run"}
          onCancel={() => {
            if (wbBusy !== null) return;
            setWbTarget(null);
            setWbPreview(null);
          }}
          onConfirm={() => void runSwitch()}
        />
      )}
    </>
  );

  return {
    busy: { switching: masterTarget !== null || wbBusy === "run", toolSelection: toolPending !== null, preview: wbBusy === "preview" },
    busyActionId,
    busyRowId,
    handle,
    dialogs,
  };
}