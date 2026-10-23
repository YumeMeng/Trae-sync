/**
 * TRAE 库页读取 hook（P4e-5a）：按 TraeLibrarySnapshot 逐字段取数（27 项 + now 入参注入）。命令经 platform/library
 * （不直接 invoke）；不读 Date.now。首读失败 → 读取面清空 + loadError（错误卡）；刷新失败 → 保留数据，原因经返回值交回。
 * 旁路失败落 null（未读到），不用 [] 冒充「没有」。代次守卫三层：①产品/库/可见性一变领新代次并清态 ②落地前比对领票 ③effect 清理与卸载再作废。
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { fetchMasterHistory, fetchMasterLibraryStats, fetchMasterSessionMessages,
  fetchProductOperationStates, fetchRelayLedger, fetchTraeCnForeignProjects, listEnvironments,
  readEnvironmentState, readMasterBackupChain } from "../../../platform/library";
import { TRAE_CN_LIBRARY_TITLE, isReadOnlyLibrary } from "../../../platform/productRegistry";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import type { MasterHistoryDto, MasterSessionEntryDto, RelayLedgerEntryDto, TraeCnForeignProjectDto } from "../../../types/history";
import type { MasterBackupChainDto, MasterLibraryStatsDto } from "../../../types/masterLibrary";
import type { ProductOperationSnapshotDto } from "../../../types/product";
import type { TraeLibrarySnapshot, TraeViewerSource } from "../traeLibrary";

const TRAE_PRODUCTS: ReadonlySet<string> = new Set(["trae_cn"]); // fail-closed：非本产品不读取、不猜数据面
const MASTER_LIBRARY_ID = "master", POLL_INTERVAL_MS = 5000;
const TRAE_LOAD_FAILURE = "库记录暂时不可读取，请稍后重试。"; // 旧 L1396 兜底句逐字
/** 页面自持 15 项（含插件 tab 与备份对比两个按 tab 激活读取的状态）：原样带入快照，本 hook 不读取、不派生、不覆盖。 */
export type TraeLibraryPageFields = Pick<TraeLibrarySnapshot, "activeTab" | "archiveDraft" | "searchText" | "expandedIds" | "selectedProjectIds" | "selectedSessionIds" | "selectMode" | "archiveView" | "archiveCollapsedKeys" | "viewerTab" | "busy" | "archiveRecovery" | "draftStorageError" | "verificationState" | "pluginTab">;
/** 读取面 12 项（now 另注入）：台账/其他账号项目放宽为 null = 未读到，与「没有」区分。
 * 备份对比（get_master_verification）与插件 tab 按「tab 激活」读取（旧 V69-73 / P135-139），不随主读取批量取。 */
export type TraeLibraryData = Omit<TraeLibrarySnapshot, "ledger" | "foreignProjects"> & {
  readonly ledger: readonly RelayLedgerEntryDto[] | null; readonly foreignProjects: readonly TraeCnForeignProjectDto[] | null;
};
/** 一次读取的结果：刷新失败只交回调用方（不清快照、不翻错误卡）。 */
export type TraeLibraryReadOutcome = { readonly ok: true } | { readonly ok: false; readonly error: string };
const READ_OK: TraeLibraryReadOutcome = { ok: true };
export interface TraeLibraryOptions { readonly productId: string; readonly libraryId: string; readonly libraryDisplayName?: string; readonly active: boolean; readonly now: Date }
export interface TraeLibraryReadResult {
  readonly data: TraeLibraryData; readonly loading: boolean;
  /** force=true 不带指纹（写后强制重读）；默认带指纹（轮询口径，unchanged 直接返回）。 */
  readonly refresh: (options?: { readonly force?: boolean }) => Promise<TraeLibraryReadOutcome>;
  readonly openSession: (session: MasterSessionEntryDto) => Promise<void>; readonly closeSession: () => void;
  readonly loadOlder: () => Promise<void>; // 滚到顶部取更早一页：新页到达才前置拼接，失败保留当前窗口
}
/** 读取面九路（history 为主读取）：旁路失败一律 null，不阻塞主读取。 */
interface TraeReads {
  readonly history: MasterHistoryDto | null; readonly ledger: readonly RelayLedgerEntryDto[] | null; readonly operations: ProductOperationSnapshotDto | null;
  readonly stats: MasterLibraryStatsDto | null; readonly backupChain: MasterBackupChainDto | null;
  readonly currentAccountName: string | null; readonly dataDir: string | null; readonly foreignProjects: readonly TraeCnForeignProjectDto[] | null;
}
const EMPTY_READS: TraeReads = { history: null, ledger: null, operations: null, stats: null, backupChain: null, currentAccountName: null, dataDir: null, foreignProjects: null };
const VIEWER_LOADING: Omit<TraeViewerSource, "session"> = { status: "loading", messages: [], hasMoreBefore: false, messageOffset: 0, loadingOlder: false };
/** 会话消息读取（offset 缺省 = 首页）：status 非 ready 一律 error（旧 L2123-2131 口径）；抛错也落 error，不清会话本身。 */
async function readViewer(session: MasterSessionEntryDto, libraryId: string, offset?: number): Promise<Omit<TraeViewerSource, "session">> {
  try {
    const result = await fetchMasterSessionMessages({ sessionId: session.session_id, libraryId, ...(offset === undefined ? {} : { offset }) });
    const messages = result.status === "ready" ? result.messages : [], ready = result.status === "ready";
    return { status: ready ? "ready" : "error", messages, hasMoreBefore: ready && result.has_more,
      messageOffset: (offset ?? 0) + messages.length, loadingOlder: false };
  } catch {
    return { ...VIEWER_LOADING, status: "error", messageOffset: offset ?? 0 };
  }
}
/** 环境档案（只读数据根不读：工具不管理其登录态）；失败落 null，不冒充「未登录」。 */
async function readEnvInfo(libraryId: string, readOnly: boolean): Promise<Pick<TraeReads, "currentAccountName" | "dataDir">> {
  if (readOnly) return { currentAccountName: null, dataDir: null };
  if (libraryId === MASTER_LIBRARY_ID) {
    const state = await readEnvironmentState();
    return { currentAccountName: state.current_account_name ?? null, dataDir: state.data_dir ?? null };
  }
  const match = (await listEnvironments({ includeSize: false })).find((item) => item.env_id === libraryId) ?? null;
  return { currentAccountName: match?.current_account_name ?? null, dataDir: match?.data_dir ?? null };
}
/** 旁路六路并发：任一失败只让该项落 null；台账只在库记录 ready 时随行刷新。 */
async function readTraeSide(libraryId: string, readOnly: boolean, history: MasterHistoryDto): Promise<Omit<TraeReads, "history">> {
  const scoped = libraryId === MASTER_LIBRARY_ID && !readOnly ? {} : { libraryId };
  const [ledger, operations, stats, backupChain, env, foreignProjects] = await Promise.all([
    history.status === "ready" ? fetchRelayLedger({ libraryId }).catch(() => null) : Promise.resolve(null),
    fetchProductOperationStates({ productId: readOnly ? "trae_cn" : "work_cn", libraryId }).catch(() => null),
    fetchMasterLibraryStats(scoped).catch(() => null),
    readMasterBackupChain(scoped).catch(() => null), readEnvInfo(libraryId, readOnly).catch(() => null),
    readOnly ? fetchTraeCnForeignProjects({ libraryId }).catch(() => null) : Promise.resolve(null),
  ]);
  return { ledger, operations, stats, backupChain, foreignProjects,
    currentAccountName: env?.currentAccountName ?? null, dataDir: env?.dataDir ?? null };
}
/** TRAE 库读取 hook：读命令 9 条 + 会话消息 1 条（首读 / 刷新 / 轮询 / 查看器同一条代次链）。 */
export function useTraeLibrary(page: TraeLibraryPageFields, options: TraeLibraryOptions): TraeLibraryReadResult {
  const { productId, libraryId, libraryDisplayName, active, now } = options, readOnly = isReadOnlyLibrary(libraryId);
  const libraryTitle = readOnly ? TRAE_CN_LIBRARY_TITLE : libraryId === MASTER_LIBRARY_ID ? "主库" : libraryDisplayName?.trim() || "副库";
  const [reads, setReads] = useState<TraeReads>(EMPTY_READS), [viewer, setViewer] = useState<TraeViewerSource | null>(null);
  const [loading, setLoading] = useState(false), [error, setError] = useState<string | null>(null); // error 只装首读失败
  // 代次守卫①：产品 / 库 / 可见性一变立即领新代次并清态（含在途查看器请求）。
  const generation = useRef(0), viewerTicket = useRef(0);
  // 轮询指纹基线（上一轮成功读取的三件套指纹 + 账号身份；force 读取不带）。
  const fingerprint = useRef<MasterHistoryDto["fingerprint"] | null>(null), currentUserId = useRef<string | null>(null);
  const latest = useRef({ active, reads, viewer }); latest.current = { active, reads, viewer }; // 渲染期写入，state 不进依赖
  useLayoutEffect(() => {
    generation.current += 1; viewerTicket.current += 1; fingerprint.current = null; currentUserId.current = null;
    setReads(EMPTY_READS); setViewer(null); setError(null); setLoading(false);
  }, [productId, libraryId, active]);
  const load = useCallback(async (force: boolean): Promise<TraeLibraryReadOutcome> => {
    if (!TRAE_PRODUCTS.has(productId) || !latest.current.active) return READ_OK;
    const ticket = ++generation.current; setLoading(true); setError(null);
    try {
      const history = await fetchMasterHistory({ previous: force ? null : fingerprint.current,
        previousCurrentUserId: force ? null : currentUserId.current, libraryId });
      const side = history.status === "unchanged" ? null : await readTraeSide(libraryId, readOnly, history); // 指纹未变：不刷台账
      if (ticket !== generation.current || side === null) return READ_OK; // 晚到的旧请求：结果已作废
      fingerprint.current = history.fingerprint;
      currentUserId.current = history.status === "ready" ? history.current_user_id : null;
      setReads({ history, ...side });
      return READ_OK;
    } catch (reason: unknown) {
      if (ticket !== generation.current) return READ_OK;
      const message = safeUiErrorMessage(reason, TRAE_LOAD_FAILURE);
      // 首读失败（此前没有 history）→ 清读取面 + 暴露原因；刷新失败（已有数据）→ 保留全部已取回数据。
      if (latest.current.reads.history === null) { setReads(EMPTY_READS); setError(message); }
      return { ok: false, error: message };
    } finally { if (ticket === generation.current) setLoading(false); }
  }, [productId, libraryId, readOnly]);
  useEffect(() => {
    if (!active) return;
    void load(false);
    return () => { generation.current += 1; }; // 代次守卫③：卸载 / 换代作废在途读取
  }, [active, load]);
  // 5 秒指纹轮询：库被 TRAE 写入时自动刷新；瞬态失败静默（下一轮重试，不打断当前树）。
  useEffect(() => {
    if (!active || !TRAE_PRODUCTS.has(productId)) return;
    const timer = window.setInterval(() => void load(false), POLL_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [active, productId, load]);
  const refresh = useCallback((next: { readonly force?: boolean } = {}) => load(next.force === true), [load]);
  const openSession = useCallback(async (session: MasterSessionEntryDto) => {
    const ticket = ++viewerTicket.current;
    setViewer({ session, ...VIEWER_LOADING });
    const patch = await readViewer(session, libraryId);
    if (ticket === viewerTicket.current) setViewer({ session, ...patch });
  }, [libraryId]);
  const closeSession = useCallback(() => { viewerTicket.current += 1; setViewer(null); }, []);
  const loadOlder = useCallback(async () => {
    const current = latest.current.viewer;
    if (current === null || current.status !== "ready" || !current.hasMoreBefore || current.loadingOlder) return;
    const ticket = viewerTicket.current, sessionId = current.session.session_id;
    const live = (previous: TraeViewerSource | null): previous is TraeViewerSource => previous !== null && previous.session.session_id === sessionId && ticket === viewerTicket.current;
    setViewer((previous) => (live(previous) ? { ...previous, loadingOlder: true } : previous));
    const patch = await readViewer(current.session, libraryId, current.messageOffset);
    setViewer((previous) => {
      if (!live(previous)) return previous;
      // 只有新页到达才前置拼接；失败保留当前窗口，只收 loadingOlder（不把 messages 清空）。
      return patch.status === "ready" ? { ...previous, ...patch, messages: [...patch.messages, ...previous.messages] } : { ...previous, loadingOlder: false };
    });
  }, [libraryId]);
  // 快照按 TraeLibrarySnapshot 声明顺序逐项填充：读取面 12 项 + 页面自持 15 项 + now（入参注入）。
  const data: TraeLibraryData = { libraryId, libraryTitle, readOnly, ...reads, ...page, viewer, loadError: error, now };
  return { data, loading, refresh, openSession, closeSession, loadOlder };
}
