import { createElement, useCallback, useRef, useState, type ReactNode } from "react";
import { Square } from "lucide-react";
import { ConfirmDialog } from "../../../components/shared/ConfirmDialog";
import { notify } from "../../../platform/notify";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import type { EnvironmentDeletePreviewDto } from "../../../types/environment";
import { ENV_TEXTS, fillEnvText } from "../config";
import { EnvironmentNameDialog, type NameDialogValue } from "./EnvironmentNameDialog";
import { EnvironmentDeleteDialog } from "./EnvironmentDeleteDialog";

/**
 * 环境页弹层 hook（P4d-8a-2b）：关闭确认 + 创建 / 改名 / 删除三只生命周期弹层。
 * 旧实现对照 EnvironmentPage.tsx（简写 L）：关闭弹层 L874-894、创建 L814-827、改名 L828-843、
 * 删除 L861-872 与 EnvironmentDeleteDialog L1437-1530、EnvironmentNameDialog L1190-1267。
 * 命令与刷新全部由调用方注入（本文件零 platform 命令、零 invoke、零产品判断）；
 * 忙碌位按旧口径就地持有（弹层自己的 busy，不混进 useEnvironmentBusy 的实例动作位）。
 * 刷新口径逐字照旧页：创建/改名/删除成功后都是整组 load()（= 调用方传 refreshFull）。
 */

/** 注入命令：三条生命周期命令 + 删除规模预览（预览与执行分开，绝不合并）。 */
export interface EnvironmentLifecycleCommands {
  readonly create: (name: string) => Promise<unknown>;
  readonly rename: (envId: string, name: string) => Promise<unknown>;
  readonly delete: (envId: string) => Promise<boolean>;
  /** 删除规模预览（只读；取不到时弹层如实告知且不许确认，旧 L1499）。 */
  readonly preview: (envId: string) => Promise<EnvironmentDeletePreviewDto>;
}

/** 弹层目标（显示名由调用方按视图取，绝不把 env_id 当文案）。 */
export interface EnvDialogTarget {
  readonly envId: string;
  readonly name: string;
}

export interface EnvironmentDialogsInput {
  readonly commands: EnvironmentLifecycleCommands;
  /** 生命周期动作成功后的整组重读（旧页 load()）；失败已由读取层落 error，不重抛。 */
  readonly refresh: () => Promise<void>;
  /** 关闭确认后的执行（页面注入 useEnvironmentActions.closeEnv + refreshLocal）。 */
  readonly confirmClose: (target: EnvDialogTarget) => Promise<void>;
}

export interface EnvironmentDialogsResult {
  /** 四类弹层节点（无打开项时 null）；挂到页面根节点即可。 */
  readonly dialogs: ReactNode;
  readonly openCreate: () => void;
  readonly openRename: (target: EnvDialogTarget) => void;
  readonly openDelete: (target: EnvDialogTarget) => void;
  readonly openClose: (target: EnvDialogTarget) => void;
  /** 关闭弹层是否打开中（仅用于内部判定；页面无需读）。 */
  readonly closeTarget: EnvDialogTarget | null;
}

/** 名称弹层态（旧 NameDialogState L1182-1188）：env 由调用方单独持有，这里只留输入与提示。 */
interface NameSession extends NameDialogValue {
  readonly target: EnvDialogTarget | null; // null = 创建
}

/** 删除弹层态（旧 L1446-1449）：预览/读取失败/执行中/内联错误。 */
interface DeleteSession {
  readonly target: EnvDialogTarget;
  readonly preview: EnvironmentDeletePreviewDto | null;
  readonly loadError: string | null;
  readonly busy: boolean;
  readonly error: string | null;
}

const IDLE_NAME: NameDialogValue = { name: "", error: null, busy: false };

export function useEnvironmentDialogs(input: EnvironmentDialogsInput): EnvironmentDialogsResult {
  const latest = useRef(input);
  latest.current = input;
  const [create, setCreate] = useState<NameSession | null>(null);
  const [rename, setRename] = useState<NameSession | null>(null);
  const [remove, setRemove] = useState<DeleteSession | null>(null);
  const [closeTarget, setCloseTarget] = useState<EnvDialogTarget | null>(null);
  const [closing, setClosing] = useState(false);
  // 预览代次守卫：连开两个环境时晚到的旧预览不得落到新弹层（旧 L1451-1464 的 cancelled 同法）。
  const previewTicket = useRef(0);

  const openCreate = useCallback(() => {
    setCreate({ ...IDLE_NAME, target: null });
  }, []);

  // 改名预填原名（旧 L737：name: env.name）。
  const openRename = useCallback((target: EnvDialogTarget) => {
    setRename({ name: target.name, error: null, busy: false, target });
  }, []);

  // 删除：先开层再取规模（旧 L1451-1464 在弹层挂载后读预览，此处同序）。
  const openDelete = useCallback((target: EnvDialogTarget) => {
    const ticket = ++previewTicket.current;
    setRemove({ target, preview: null, loadError: null, busy: false, error: null });
    void latest.current.commands.preview(target.envId)
      .then((preview) => {
        if (ticket === previewTicket.current) setRemove((current) => current === null ? null : { ...current, preview });
      })
      .catch((reason: unknown) => {
        const message = safeUiErrorMessage(reason, ENV_TEXTS.deletePreviewFailure);
        if (ticket === previewTicket.current) setRemove((current) => current === null ? null : { ...current, loadError: message });
      });
  }, []);

  const openClose = useCallback((target: EnvDialogTarget) => { setCloseTarget(target); }, []);

  // 创建（旧 L821-825）：命令成功 → 整组重读 → 成功通知；失败抛回名称弹层做内联提示。
  const submitCreate = useCallback(async (name: string) => {
    await latest.current.commands.create(name);
    await latest.current.refresh();
    notify.success(fillEnvText(ENV_TEXTS.createdNotice, { name }));
  }, []);

  // 改名（旧 L835-841）：同上，目标 env_id 由弹层会话持有（不拿 env_id 当文案）。
  const submitRename = useCallback(async (name: string) => {
    const target = rename?.target;
    if (target === null || target === undefined) return;
    await latest.current.commands.rename(target.envId, name);
    await latest.current.refresh();
    notify.success(fillEnvText(ENV_TEXTS.renamedNotice, { name }));
  }, [rename]);

  // 删除确认（旧 L1466-1478）：成功先关层再整组重读 + 成功通知；失败内联提示并保留弹层。
  const confirmDelete = useCallback(() => {
    const session = remove;
    if (session === null || session.busy) return;
    setRemove({ ...session, busy: true, error: null });
    void (async () => {
      try {
        await latest.current.commands.delete(session.target.envId);
        setRemove(null); // 旧 L867：回执一到即关层
        await latest.current.refresh();
        notify.success(fillEnvText(ENV_TEXTS.deletedNotice, { name: session.target.name }));
      } catch (reason: unknown) {
        setRemove((current) => current === null ? null : {
          ...current, busy: false, error: safeUiErrorMessage(reason, ENV_TEXTS.deleteFailure) });
      }
    })();
  }, [remove]);

  // 关闭确认（旧 L887-891）：先关层再执行（旧 setCloseDialogEnv(null) → void handleCloseEnv）。
  const confirmCloseNow = useCallback(() => {
    const target = closeTarget;
    if (target === null || closing) return;
    setCloseTarget(null);
    setClosing(true);
    void latest.current.confirmClose(target).finally(() => setClosing(false));
  }, [closeTarget, closing]);

  const cancelDelete = useCallback(() => {
    if (remove?.busy === true) return; // 执行中不关层（ConfirmDialog busy 契约）
    previewTicket.current += 1; // 作废在途预览
    setRemove(null);
  }, [remove]);

  const nameDialog = (session: NameSession, title: string, confirmLabel: string, onChange: (next: NameDialogValue) => void,
    onClose: () => void, onSubmit: (name: string) => Promise<void>) => (
    <EnvironmentNameDialog title={title} confirmLabel={confirmLabel} value={session} onChange={onChange}
      onClose={onClose} onSubmit={onSubmit} />
  );

  const dialogs = (
    <>
      {create !== null && nameDialog(create, ENV_TEXTS.createDialogTitle, ENV_TEXTS.createConfirm,
        (next) => setCreate({ ...create, ...next }), () => setCreate(null), submitCreate)}
      {rename !== null && nameDialog(rename, ENV_TEXTS.renameDialogTitle, ENV_TEXTS.renameConfirm,
        (next) => setRename({ ...rename, ...next }), () => setRename(null), submitRename)}
      {remove !== null && (
        <EnvironmentDeleteDialog envName={remove.target.name} preview={remove.preview} loadError={remove.loadError}
          busy={remove.busy} error={remove.error} onCancel={cancelDelete} onConfirm={confirmDelete} />
      )}
      {/* 关闭实例确认（旧 L874-894 逐字）：标题/两行影响面/按钮文案与 testid 全部照旧。 */}
      {closeTarget !== null && createElement(ConfirmDialog, {
        title: fillEnvText(ENV_TEXTS.closeDialogTitle, { name: closeTarget.name }),
        lines: [
          fillEnvText(ENV_TEXTS.closeDialogLine1, { name: closeTarget.name }),
          ENV_TEXTS.closeDialogLine2,
        ],
        confirmLabel: ENV_TEXTS.closeDialogConfirm,
        confirmIcon: <Square size={15} aria-hidden="true" />,
        busyLabel: ENV_TEXTS.closeDialogBusy,
        busy: closing,
        onCancel: () => setCloseTarget(null),
        onConfirm: confirmCloseNow,
        testId: "env-close-dialog",
      })}
    </>
  );

  return { dialogs, openCreate, openRename, openDelete, openClose, closeTarget };
}
