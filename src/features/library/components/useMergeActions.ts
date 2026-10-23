import { useCallback, useRef, useState } from "react";
import { useTauriEvents } from "../../../hooks/useTauriEvents";
import { mergeMasterProjects, mergeTraeCnProject, previewTraeCnProjectMerge } from "../../../platform/libraryDelete";
import { mergeWorkBuddyCwdProjects } from "../../../platform/library";
import { notify } from "../../../platform/notify";
import { LIBRARY_TEXTS, fillLibraryText } from "../config";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import type { TraeCnMergePreviewDto, TraeCnMergeProgressEvent } from "../../../types/history";
import type { LibraryForceRefresh } from "./useLibraryDeleteActions";
/**
 * 库页归并动作 hook（P4e-6b，补 P4e-6 未覆盖的归并）：TRAE 项目归并 / Trae CN 跨账号归并 / WB cwd 归并三条独立链。命令一律经
 * platform（零 invoke）；确认层归调用方弹层——本 hook 只产出「预览单 / 订单」并在执行时消费它，计数与文案全由视图与后端预览算
 * 出（执行侧不重算，防确认层与执行层漂移）。纪律：成功 refresh({force:true})，刷新失败只追加警告（已提交的归并不得误报失败）；
 * 失败 notify.error 且不清数据、不刷新、不退选择。CN 进度走 trae-cn-merge-progress 独立监听链（自有代次 + env/kind 校验），不与
 * 读取链或其它动作链共用代次。旧实现对照 LibrarySessionsPanel.tsx（L）/ WorkBuddyLibraryPage.tsx（WB）：项目归并 L1069-1103、
 * CN 归并 L1117-1184、进度文案 L2522-2536、WB cwd 归并 WB162-177 / WB521-537。
 * 候选项目 / 预览单 / 订单：由视图或预览命令产出，执行侧只消费（不重算计数与文案）。
 */
export interface MergeProjectView { readonly projectId: string; readonly name: string; readonly sessionCount: number }
export interface ProjectMergePreview { readonly testId: string; readonly title: string; readonly body: string; readonly confirmLabel: string; readonly busyLabel: string; readonly candidates: readonly MergeProjectView[] }
export interface ProjectMergeOrder { readonly libraryId: string; readonly targetProjectId: string; readonly targetName: string; readonly sourceProjectIds: readonly string[]; readonly movedSessions: number }
export interface CnMergeRow { readonly userId: string; readonly name: string; readonly meta: string }
export interface CnMergePreview { readonly projectId: string; readonly projectName: string; readonly testId: string; readonly title: string; readonly body: string; readonly confirmLabel: string; readonly busyLabel: string; readonly rows: readonly CnMergeRow[]; readonly targetNote: string | null; readonly totalSessions: number; readonly totalMessages: number }
export interface CnMergeOrder { readonly projectId: string; readonly projectName: string; readonly libraryId: string }
export interface CwdMergeOrder { readonly canonical: string; readonly variants: readonly string[]; readonly testId: string; readonly title: string; readonly confirmLabel: string; readonly busyLabel: string; readonly lines: readonly string[] }
export interface MergeActionsInput { readonly refresh: LibraryForceRefresh; readonly refreshCheckup?: () => Promise<unknown> } // refreshCheckup 只 WB 传（旧 WB171）
/** 三条链三个忙碌位（TRAE 两条互斥、WB 独立）：同链重入 + 另一链在跑双判定，收位只收自己那一位。 */
export interface MergeBusy { readonly project: boolean; readonly traeCn: boolean; readonly cwd: boolean }
export interface MergeRunOutcome { readonly ok: boolean; readonly moved: number } // moved 取后端回执，不用视图计数冒充
export interface MergeActionsResult {
  readonly busy: MergeBusy; readonly mergeStage: string | null; // mergeStage = CN 进度文案（独立链）；null = 无进行中归并
  readonly previewProjectMerge: (projects: readonly MergeProjectView[]) => ProjectMergePreview | null;
  readonly chooseProjectMergeTarget: (preview: ProjectMergePreview | null, targetProjectId: string, libraryId: string) => ProjectMergeOrder | null;
  readonly runProjectMerge: (order: ProjectMergeOrder | null) => Promise<MergeRunOutcome>;
  readonly openCnMergePreview: (project: { readonly projectId: string; readonly name: string }, libraryId: string) => Promise<CnMergePreview | null>;
  readonly runCnMerge: (order: CnMergeOrder | null) => Promise<MergeRunOutcome>; readonly previewCwdMerge: (candidate: readonly string[] | null) => CwdMergeOrder | null;
  readonly runCwdMerge: (order: CwdMergeOrder | null) => Promise<MergeRunOutcome>;
}
const IDLE: MergeBusy = { project: false, traeCn: false, cwd: false }, NO_RUN: MergeRunOutcome = { ok: false, moved: 0 };
const MASTER_MERGE_TEST_ID = "merge-confirm", CN_MERGE_TEST_ID = "trae-cn-merge-confirm", UNNAMED_PROJECT = "未命名项目";
// 阶段白名单：载荷 stage 是判别字段，未知阶段一律丢弃（不猜进度，与收编链同法）。
const STAGES: ReadonlySet<string> = new Set<TraeCnMergeProgressEvent["stage"]>(["backing_up", "mapping", "executing", "verifying", "done"]);
/** 后端载荷现只含 stage/current/total/label；env_id / kind 为多库多链区分预留——带则校验，缺则按事件名接受。 */
type MergeProgressPayload = TraeCnMergeProgressEvent & { readonly env_id?: string; readonly kind?: string };
const CN_KINDS: ReadonlySet<string> = new Set(["trae_cn_project_merge", "library.trae_cn_project_merge"]); // 命令名 / 操作标识同源
/** 阶段 → 按钮文案：逐字沿用旧 L2522-2536，句子取 config（本文件不自造）。 */
const STAGE_TEXT: Readonly<Record<string, string>> = { backing_up: LIBRARY_TEXTS.stageBackingUp, mapping: LIBRARY_TEXTS.stageMapping, executing: LIBRARY_TEXTS.stageExecuting, verifying: LIBRARY_TEXTS.stageVerifying, done: LIBRARY_TEXTS.stageDone };
const stageTextOf = (event: TraeCnMergeProgressEvent): string =>
  fillLibraryText(STAGE_TEXT[event.stage] ?? LIBRARY_TEXTS.stageDone, { current: event.current, total: event.total, label: event.label || "会话" });
export function useMergeActions(input: MergeActionsInput): MergeActionsResult {
  const latest = useRef(input); latest.current = input; // 入参经 ref 取最新值：动作身份稳定
  const busyRef = useRef<MergeBusy>({ ...IDLE }); // 防连点唯一真源（同一 tick 内重复点击也拦得住）
  const [busy, setBusy] = useState<MergeBusy>(IDLE), [mergeStage, setMergeStage] = useState<string | null>(null);
  // CN 进度独立链：自有代次票 + 目标库，不与读取代次 / 其它动作链共用（晚到的上一轮事件不推进本轮）。
  const cnRun = useRef<{ readonly ticket: number; readonly libraryId: string } | null>(null), cnTicket = useRef(0);
  const mark = useCallback((patch: Partial<MergeBusy>) => { busyRef.current = { ...busyRef.current, ...patch }; setBusy(busyRef.current); }, []);
  // 写后强制重读：失败只返回 false，不翻转成功回执；体检是旁路，失败不打断回执。
  const reload = useCallback(async (): Promise<boolean> => {
    try { return (await latest.current.refresh({ force: true })).ok; } catch { return false; }
  }, []), reloadCheckup = useCallback(async () => { try { await latest.current.refreshCheckup?.(); } catch { /* 体检失败不打断回执 */ } }, []);
  // 进度订阅（常挂载，不随目标重建：首个阶段事件不会丢）：无进行中归并 / 未知阶段 / 他库 env_id / 他链 kind / 上一轮残留票
  // 五种情况一律丢弃，绝不把不相关载荷当进度推进。
  useTauriEvents<{ "trae-cn-merge-progress": MergeProgressPayload }>({ "trae-cn-merge-progress": (payload) => {
    const run = cnRun.current;
    if (run === null || run.ticket !== cnTicket.current || !STAGES.has(payload.stage)) return;
    if (payload.env_id !== undefined && payload.env_id !== run.libraryId) return;
    if (payload.kind !== undefined && !CN_KINDS.has(payload.kind)) return;
    setMergeStage(stageTextOf(payload));
  } }, []);
  // ===== TRAE 项目归并（master-remap）：候选 → 选保留目标 → 单事务改挂 + 空壳清理（旧 L1069-1103）=====
  const previewProjectMerge = useCallback((projects: readonly MergeProjectView[]): ProjectMergePreview | null => {
    if (projects.length < 2) return null; // 少于两个项目没有合并意义（旧 L1071）
    return { testId: MASTER_MERGE_TEST_ID, title: fillLibraryText(LIBRARY_TEXTS.mergeTitle, { count: projects.length }),
      body: fillLibraryText(LIBRARY_TEXTS.mergeBody, { moved: projects.reduce((sum, item) => sum + item.sessionCount, 0) }),
      confirmLabel: LIBRARY_TEXTS.mergeConfirm, busyLabel: LIBRARY_TEXTS.mergeBusy, candidates: projects };
  }, []);
  const chooseProjectMergeTarget = useCallback((preview: ProjectMergePreview | null, targetProjectId: string, libraryId: string): ProjectMergeOrder | null => {
    if (preview === null) return null;
    const target = preview.candidates.find((item) => item.projectId === targetProjectId);
    const sources = preview.candidates.filter((item) => item.projectId !== targetProjectId);
    if (target === undefined || sources.length === 0) return null; // 目标不在候选内 / 无源：不许凭空下单
    return { libraryId, targetProjectId, targetName: target.name.trim() || UNNAMED_PROJECT, sourceProjectIds: sources.map((item) => item.projectId),
      movedSessions: sources.reduce((sum, item) => sum + item.sessionCount, 0) };
  }, []);
  const runProjectMerge = useCallback(async (order: ProjectMergeOrder | null): Promise<MergeRunOutcome> => {
    if (order === null || busyRef.current.project || busyRef.current.traeCn) return NO_RUN;
    mark({ project: true });
    try {
      const receipt = await mergeMasterProjects({ sourceProjectIds: order.sourceProjectIds, targetProjectId: order.targetProjectId, libraryId: order.libraryId });
      // 回执只用数量与动作结果表述（界面表达纪律）。
      notify.success(fillLibraryText(LIBRARY_TEXTS.mergeReceipt, { moved: receipt.moved_sessions, target: order.targetName, removed: receipt.removed_projects }));
      if (!(await reload())) notify.warning(LIBRARY_TEXTS.refreshFailedAfterAction);
      return { ok: true, moved: receipt.moved_sessions };
    } catch (reason: unknown) { notify.error(safeUiErrorMessage(reason, LIBRARY_TEXTS.actionFailure)); return NO_RUN; } // 失败不清数据
    finally { mark({ project: false }); }
  }, [mark, reload]);
  // ===== Trae CN 归并（只读库唯一写，ADR-0033）：规模预览 → 单次确认 → 备份先行 + 单事务（旧 L1117-1184）=====
  const openCnMergePreview = useCallback(async (project: { readonly projectId: string; readonly name: string }, libraryId: string): Promise<CnMergePreview | null> => {
    try {
      const preview: TraeCnMergePreviewDto = await previewTraeCnProjectMerge({ projectId: project.projectId, libraryId });
      const projectName = project.name.trim() || UNNAMED_PROJECT, totals = { sessions: preview.total_sessions, messages: preview.total_messages };
      return { projectId: project.projectId, projectName, testId: CN_MERGE_TEST_ID, title: fillLibraryText(LIBRARY_TEXTS.cnMergeTitle, { project: projectName }),
        body: fillLibraryText(LIBRARY_TEXTS.cnMergeBody, { ...totals, target: preview.target_account_name }),
        confirmLabel: LIBRARY_TEXTS.cnMergeConfirm, busyLabel: LIBRARY_TEXTS.cnMergeBusy,
        rows: preview.sources.map((source) => ({ userId: source.user_id, name: source.account_name?.trim() || LIBRARY_TEXTS.unregisteredAccount,
          meta: fillLibraryText(LIBRARY_TEXTS.cnMergeItemSuffix, { sessions: source.session_count, messages: source.message_count }) })),
        targetNote: preview.target_exists ? fillLibraryText(LIBRARY_TEXTS.cnMergeTargetNote, { count: preview.target_session_count }) : null,
        totalSessions: preview.total_sessions, totalMessages: preview.total_messages };
    } catch (reason: unknown) { notify.error(safeUiErrorMessage(reason, LIBRARY_TEXTS.mergePreviewFailure)); return null; } // 不弹确认层
  }, []);
  const runCnMerge = useCallback(async (order: CnMergeOrder | null): Promise<MergeRunOutcome> => {
    if (order === null || busyRef.current.traeCn || busyRef.current.project) return NO_RUN;
    mark({ traeCn: true });
    cnRun.current = { ticket: ++cnTicket.current, libraryId: order.libraryId };
    setMergeStage(LIBRARY_TEXTS.cnMergeBusy); // 首个阶段事件到达前用确认层忙碌句，不空窗
    try {
      const receipt = await mergeTraeCnProject({ projectId: order.projectId, libraryId: order.libraryId });
      notify.success(fillLibraryText(LIBRARY_TEXTS.cnMergeReceipt, { moved: receipt.switched_sessions, messages: receipt.moved_messages })
        + (receipt.removed_mirror_rows > 0 ? fillLibraryText(LIBRARY_TEXTS.cnMergeReceiptCleanup, { removed: receipt.removed_mirror_rows }) : "。"));
      if (!(await reload())) notify.warning(LIBRARY_TEXTS.mergeRefreshFailed);
      return { ok: true, moved: receipt.switched_sessions };
    } catch (reason: unknown) { notify.error(safeUiErrorMessage(reason, LIBRARY_TEXTS.cnMergeFailure)); return NO_RUN; } // 数据保持操作前状态
    finally { cnRun.current = null; setMergeStage(null); mark({ traeCn: false }); }
  }, [mark, reload]);
  // ===== WB cwd 归并（canonical 吸收 variants；正文迁移与关→写→重启由后端承担；旧 WB162-177 / WB521-537）=====
  const previewCwdMerge = useCallback((candidate: readonly string[] | null): CwdMergeOrder | null => {
    if (candidate === null || candidate.length < 2 || candidate[0].trim() === "") return null; // 单目录无变体可归并
    const canonical = candidate[0], variants = candidate.slice(1);
    return { canonical, variants, testId: LIBRARY_TEXTS.wbMergeConfirmTestId, title: LIBRARY_TEXTS.wbMergeConfirmTitle,
      confirmLabel: LIBRARY_TEXTS.wbMergeConfirm, busyLabel: LIBRARY_TEXTS.wbMergeConfirmBusy,
      lines: [fillLibraryText(LIBRARY_TEXTS.wbMergeConfirmLine1, { count: variants.length, cwd: canonical, variants: variants.join("、") }),
        LIBRARY_TEXTS.wbMergeConfirmLine2, LIBRARY_TEXTS.wbMergeConfirmLine3] };
  }, []);
  const runCwdMerge = useCallback(async (order: CwdMergeOrder | null): Promise<MergeRunOutcome> => {
    if (order === null || busyRef.current.cwd) return NO_RUN;
    mark({ cwd: true });
    try {
      const receipt = await mergeWorkBuddyCwdProjects(order.canonical, order.variants);
      notify.success(fillLibraryText(LIBRARY_TEXTS.wbMergeReceipt, { moved: receipt.moved_sessions, cwd: order.canonical,
        bodies: receipt.moved_body_files, restarted: receipt.tail.restarted ? LIBRARY_TEXTS.wbRestarted : "" }));
      if (!(await reload())) notify.warning(LIBRARY_TEXTS.refreshFailedAfterAction);
      await reloadCheckup(); // 归并改了正文目录位置：列表重读之后再跑体检（旧 WB171）
      return { ok: true, moved: receipt.moved_sessions };
    } catch (reason: unknown) { notify.error(safeUiErrorMessage(reason, LIBRARY_TEXTS.wbFailureSuffix)); return NO_RUN; } // 数据未被改动
    finally { mark({ cwd: false }); }
  }, [mark, reload, reloadCheckup]);
  return { busy, mergeStage, previewProjectMerge, chooseProjectMergeTarget, runProjectMerge,
    openCnMergePreview, runCnMerge, previewCwdMerge, runCwdMerge };
}