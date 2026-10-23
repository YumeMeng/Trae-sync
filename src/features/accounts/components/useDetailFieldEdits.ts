import { useCallback, useRef, useState } from "react";
import { notify } from "../../../platform/notify";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import { ACCOUNT_TEXTS } from "../config";
import type { DetailActionTarget, DetailTextEditAction, DetailToggleAction, PageDetailActions } from "../detailActionCommands";

/**
 * 行内编辑与开关字段的分发（P4c-6b-2e-2）：备注名 / 手机号补录 / 自动签到开关 / 归档开关。
 *
 * 命令与文案全部由 detailActionCommands 的接缝注入（本文件零 platform 命令、零产品判断、零 invoke）。
 * 语义逐字沿用旧实现：备注名 AccountDetail L97-106、手机号补录 L108-115 + AccountCenterTrae L505-525、
 * 自动签到开关 L435-442、WB 备注名 L80-94 / 归档 L96-109 / 参与开关 L111-129。
 *
 * **乐观更新策略（仅开关）**：点击即翻本地覆盖态，命令成功且缓存重读完成后撤掉覆盖（回到权威值），
 * 失败则把覆盖回滚成原值并 notify.error——编辑态与草稿不受影响（失败保留编辑态由调用方 onSave 的
 * false 返回表达）。文本编辑不做乐观更新：成功才退出编辑态，失败保留输入。
 */
export interface DetailFieldEditsInput {
  readonly seam: PageDetailActions | undefined;
  readonly rowId: string;
  /** 本产品数据面（触发时才取，保证用最新快照解析目标）。 */
  readonly readPayload: () => unknown;
  /** 成功后重读本机缓存（旧实现每个动作后 refreshOverview / refresh 同法）。 */
  readonly onSettled: () => Promise<void>;
  /**
   * G11 查重确认层（P4c-6b-2e-3）：命中时弹层并等用户决定；true = 已强行写入。
   * 缺省 = 不做查重（未接线路径与既有测试一字不改）。
   */
  readonly confirmDuplicate?: (request: { readonly ownerName: string; readonly write: () => Promise<void> }) => Promise<boolean>;
}

export interface DetailFieldEditsResult {
  /** 正在保存的字段 id（只影响该字段的保存按钮与输入禁用）。 */
  readonly busy: readonly string[];
  /** 开关字段的乐观覆盖态（字段 id → 目标态）；未覆盖即用 builder 的权威态。 */
  readonly toggles: Readonly<Record<string, boolean>>;
  /** 保存文本字段：true = 成功（退出编辑态）；false = 失败/未登记（保留编辑态）。 */
  readonly save: (fieldId: string, value: string) => Promise<boolean>;
  /** 翻转开关字段：乐观置位 → 命令 → 重读缓存后撤覆盖；失败回滚并提示。 */
  readonly toggle: (fieldId: string, next: boolean) => Promise<boolean>;
}

/** 失败文案：前缀（旧 WB「WorkBuddy：」）+ 中段（旧 WB「备注名保存失败——」）+ 映射后原因。 */
const failureText = (reason: unknown, action: {
  readonly failurePrefix?: string; readonly failureLabel?: string; readonly failureFallback?: string;
}): string => `${action.failurePrefix ?? ""}${action.failureLabel ?? ""}${safeUiErrorMessage(reason, action.failureFallback ?? ACCOUNT_TEXTS.failureFallback)}`;

export function useDetailFieldEdits(input: DetailFieldEditsInput): DetailFieldEditsResult {
  const inputRef = useRef(input);
  inputRef.current = input;
  const [pending, setPending] = useState<string | null>(null);
  const [toggles, setToggles] = useState<Readonly<Record<string, boolean>>>({});

  // 目标每次触发都重新解析（数据面可能已刷新）；找不到即 null（不猜目标）。
  const targetOf = useCallback((): DetailActionTarget | null => {
    const { seam, readPayload, rowId } = inputRef.current;
    return seam === undefined || rowId === "" ? null : seam.targetOf(readPayload(), rowId);
  }, []);

  /** 文本字段命令按字段 id 查表（备注名 / 手机号补录）；未登记即 null（只读）。 */
  const textActionOf = useCallback((fieldId: string): DetailTextEditAction | null => {
    const seam = inputRef.current.seam;
    if (seam === undefined) return null;
    if (seam.displayName?.id === fieldId) return seam.displayName;
    if (seam.mobile?.id === fieldId) return seam.mobile;
    return null;
  }, []);

  /** 开关命令按字段 id 查表（自动签到 / 归档）；未登记即 null（只读）。 */
  const toggleActionOf = useCallback((fieldId: string): DetailToggleAction | null => {
    const seam = inputRef.current.seam;
    if (seam === undefined) return null;
    if (seam.autoCheckin?.id === fieldId) return seam.autoCheckin;
    if (seam.archive?.id === fieldId) return seam.archive;
    return null;
  }, []);

  const save = useCallback(async (fieldId: string, value: string): Promise<boolean> => {
    const action = textActionOf(fieldId);
    const target = targetOf();
    if (action === null || target === null) return false;
    setPending(fieldId);
    try {
      // G11 第三层查重（旧 AccountCenterTrae L505-525）：命中即弹确认层，确认后同一命令强行写入；
      // 用户取消 = 本次未保存（返回 false 让字段保留编辑态与草稿）。
      const ownerName = action.conflictOf?.(inputRef.current.readPayload(), target, value) ?? null;
      if (ownerName !== null) {
        const confirm = inputRef.current.confirmDuplicate;
        if (confirm === undefined) return false; // 未接线确认层：fail-closed，不静默写入
        // 确认层自行完成写入 + 重读 + 成功回执（旧 confirmDuplicateMobile L535-538 同法），此处不重复。
        return await confirm({ ownerName, write: () => action.run(target, value) });
      }
      await action.run(target, value);
      await inputRef.current.onSettled();
      notify.success(value === "" ? action.clearedCopy : action.savedCopy);
      return true;
    } catch (reason: unknown) {
      notify.error(failureText(reason, action));
      return false;
    } finally {
      setPending(null);
    }
  }, [targetOf, textActionOf]);

  const toggle = useCallback(async (fieldId: string, next: boolean): Promise<boolean> => {
    const action = toggleActionOf(fieldId);
    const target = targetOf();
    if (action === null || target === null) return false;
    setToggles((prev) => ({ ...prev, [fieldId]: next })); // 乐观置位：点击即翻
    setPending(fieldId);
    try {
      await action.run(target, next);
      await inputRef.current.onSettled();
      // 成功且缓存已重读：撤掉覆盖，回到 builder 的权威态（避免与刷新结果长期分叉）。
      setToggles((prev) => { const rest = { ...prev }; delete rest[fieldId]; return rest; });
      notify.success(next ? action.onCopy : action.offCopy);
      return true;
    } catch (reason: unknown) {
      setToggles((prev) => ({ ...prev, [fieldId]: !next })); // 失败回滚到原态
      notify.error(failureText(reason, action));
      return false;
    } finally {
      setPending(null);
    }
  }, [targetOf, toggleActionOf]);

  return { busy: pending === null ? [] : [pending], toggles, save, toggle };
}
