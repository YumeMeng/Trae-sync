/**
 * WB 库页读取 hook（P4e-5b）：按 WorkBuddyLibrarySnapshot 逐字段取数（15 项 + now 入参注入）。命令经 platform/library
 * （不直接 invoke）；不读 Date.now。首读失败 → 读取面清空 + loadError（错误卡）；刷新失败 → 保留数据，原因经返回值交回。
 * 旁路失败落 null（未读到），不用 [] 冒充「没有」：stats / groups / accounts / checkup / messages 各自降级互不阻塞。
 * 代次守卫三层：①产品/可见性/过滤条件一变领新代次并清态 ②落地前比对领票 ③effect 清理与卸载再作废。
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  fetchWorkBuddyLibraryCheckup, fetchWorkBuddyLibraryHistory, fetchWorkBuddyLibraryStats,
  fetchWorkBuddySessionMessages, listWorkBuddyAccounts,
} from "../../../platform/library";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import type { WorkBuddyLoginSummaryDto } from "../../../types/product";
import type { WorkBuddyCwdGroupDto, WorkBuddyLibraryCheckupDto, WorkBuddyLibraryStatsDto, WorkBuddySessionMessagesDto } from "../../../types/workbuddyLibrary";
import type { WorkBuddyLibrarySnapshot } from "../workbuddyLibrary";

const WB_PRODUCTS: ReadonlySet<string> = new Set(["workbuddy"]); // fail-closed：非本产品不读取、不猜数据面
const WB_LOAD_FAILURE = "对话库暂不可读取，请稍后重试。"; // 旧 WB73 兜底句逐字
const MESSAGES_FAILURE = "消息读取未完成"; // 旧 WB105 前缀逐字，原因由 safeUiError 词表补
/** 页面自持 5 项：原样带入快照，本 hook 不读取、不派生、不覆盖。 */
export type WbLibraryPageFields = Pick<WorkBuddyLibrarySnapshot, "uidFilter" | "includeDeleted" | "selectedIds" | "writeBusy" | "mergeBusy">;
/** 读取面 10 项（now 另注入）：groups / accounts 放宽为 null = 未读到，与「没有」区分。 */
export type WbLibraryData = Omit<WorkBuddyLibrarySnapshot, "groups" | "accounts"> & {
  readonly groups: readonly WorkBuddyCwdGroupDto[] | null; readonly accounts: readonly WorkBuddyLoginSummaryDto[] | null;
};
/** 一次读取的结果：刷新失败只交回调用方（不清快照、不翻错误卡）。 */
export type WbLibraryReadOutcome = { readonly ok: true } | { readonly ok: false; readonly error: string };
const READ_OK: WbLibraryReadOutcome = { ok: true };
export interface WbLibraryOptions { readonly productId: string; readonly active: boolean; readonly now: Date }
export interface WbLibraryReadResult {
  readonly data: WbLibraryData; readonly loading: boolean; readonly checking: boolean;
  /** 刷新：只重读统计 / 分组 / 账号（旧 WB60-79 口径）；体检是重活，由 runCheckup 显式重跑。 */
  readonly refresh: () => Promise<WbLibraryReadOutcome>;
  readonly runCheckup: () => Promise<WbLibraryReadOutcome>;
  /** 打开会话（行内时间线）：读 jsonl 正文；失败即收起展开（旧 WB98-110 同口径），原因经返回值交回。 */
  readonly openSession: (sessionId: string) => Promise<WbLibraryReadOutcome>;
  readonly closeSession: () => void;
}
/** 读取面六路：任一失败只让该项落 null，不阻塞其余路。 */
interface WbReads {
  readonly stats: WorkBuddyLibraryStatsDto | null; readonly groups: readonly WorkBuddyCwdGroupDto[] | null;
  readonly accounts: readonly WorkBuddyLoginSummaryDto[] | null; readonly checkup: WorkBuddyLibraryCheckupDto | null;
  readonly messages: WorkBuddySessionMessagesDto | null; readonly openSessionId: string | null;
}
const EMPTY_READS: WbReads = { stats: null, groups: null, accounts: null, checkup: null, messages: null, openSessionId: null };
/** 主读取三路并发：groups 是「有没有会话」的唯一判据（失败即首读失败）；stats / accounts 旁路降级 null。 */
async function readWbCore(uidFilter: string, includeDeleted: boolean): Promise<Pick<WbReads, "stats" | "groups" | "accounts">> {
  const [stats, groups, accounts] = await Promise.allSettled([
    fetchWorkBuddyLibraryStats(), fetchWorkBuddyLibraryHistory(uidFilter === "" ? null : uidFilter, includeDeleted),
    listWorkBuddyAccounts(),
  ]);
  if (groups.status === "rejected") throw groups.reason; // 主读取：抛错上交 load（旧 WB68-74 同口径）
  return { groups: groups.value, stats: stats.status === "fulfilled" ? stats.value : null,
    accounts: accounts.status === "fulfilled" ? accounts.value : null };
}
/** WB 库读取 hook：读命令 4 条 + 会话消息 1 条（首读 / 刷新 / 体检 / 消息三条独立代次链）。 */
export function useWbLibrary(page: WbLibraryPageFields, options: WbLibraryOptions): WbLibraryReadResult {
  const { productId, active, now } = options, { uidFilter, includeDeleted } = page;
  const [reads, setReads] = useState<WbReads>(EMPTY_READS), [messagesLoading, setMessagesLoading] = useState(false);
  const [loading, setLoading] = useState(false), [checking, setChecking] = useState(false), [error, setError] = useState<string | null>(null);
  // 代次守卫①：产品 / 可见性 / 过滤条件一变立即领新代次并清态；三条链各自持票（体检与主读取并发，互不作废）。
  const generation = useRef(0), checkupTicket = useRef(0), messageTicket = useRef(0);
  const latest = useRef({ active, reads }); latest.current = { active, reads }; // 渲染期写入，state 不进依赖
  useLayoutEffect(() => {
    generation.current += 1; checkupTicket.current += 1; messageTicket.current += 1;
    setReads(EMPTY_READS); setError(null); setLoading(false); setChecking(false); setMessagesLoading(false);
  }, [productId, active, uidFilter, includeDeleted]);
  const load = useCallback(async (): Promise<WbLibraryReadOutcome> => {
    if (!WB_PRODUCTS.has(productId) || !latest.current.active) return READ_OK;
    const ticket = ++generation.current; setLoading(true); setError(null);
    try {
      const core = await readWbCore(uidFilter, includeDeleted);
      if (ticket !== generation.current) return READ_OK; // 晚到的旧请求：结果已作废
      setReads((previous) => ({ ...previous, ...core })); // 体检沿用上次报告，不随刷新重算
      return READ_OK;
    } catch (reason: unknown) {
      if (ticket !== generation.current) return READ_OK;
      const message = safeUiErrorMessage(reason, WB_LOAD_FAILURE);
      // 首读失败（此前没有 groups）→ 清读取面 + 暴露原因；刷新失败（已有数据）→ 保留全部已取回数据。
      if (latest.current.reads.groups === null) { setReads(EMPTY_READS); setError(message); }
      return { ok: false, error: message };
    } finally { if (ticket === generation.current) setLoading(false); }
  }, [productId, uidFilter, includeDeleted]);
  const runCheckup = useCallback(async (): Promise<WbLibraryReadOutcome> => {
    if (!WB_PRODUCTS.has(productId) || !latest.current.active) return READ_OK;
    const ticket = ++checkupTicket.current; setChecking(true);
    try {
      const checkup = await fetchWorkBuddyLibraryCheckup();
      if (ticket === checkupTicket.current) setReads((previous) => ({ ...previous, checkup }));
      return READ_OK;
    } catch (reason: unknown) {
      if (ticket !== checkupTicket.current) return READ_OK;
      return { ok: false, error: safeUiErrorMessage(reason, "请稍后重试。") }; // 旧 WB86 只提示，不清已有报告
    } finally { if (ticket === checkupTicket.current) setChecking(false); }
  }, [productId]);
  useEffect(() => {
    if (!active) return;
    void load();
    void runCheckup(); // 首读同时跑一次体检（旧 WB92-96 同口径）；失败不影响主读取。
    return () => { generation.current += 1; checkupTicket.current += 1; }; // 代次守卫③：卸载 / 换代作废在途读取
  }, [active, load, runCheckup]);
  const openSession = useCallback(async (sessionId: string): Promise<WbLibraryReadOutcome> => {
    const ticket = ++messageTicket.current;
    setReads((previous) => ({ ...previous, openSessionId: sessionId, messages: null })); setMessagesLoading(true);
    try {
      const messages = await fetchWorkBuddySessionMessages(sessionId, uidFilter === "" ? null : uidFilter);
      if (ticket === messageTicket.current) setReads((previous) => ({ ...previous, messages }));
      return READ_OK;
    } catch (reason: unknown) {
      // 失败即收起展开（旧 WB104-108 同口径）：不留「暂无内容」的假空态；原因经返回值交回调用方提示。
      if (ticket === messageTicket.current) setReads((previous) => ({ ...previous, openSessionId: null, messages: null }));
      return { ok: false, error: `${MESSAGES_FAILURE}——${safeUiErrorMessage(reason, "请稍后重试。")}` };
    } finally { if (ticket === messageTicket.current) setMessagesLoading(false); }
  }, [uidFilter]);
  const closeSession = useCallback(() => {
    messageTicket.current += 1;
    setReads((previous) => ({ ...previous, openSessionId: null, messages: null })); setMessagesLoading(false);
  }, []);
  // 快照按 WorkBuddyLibrarySnapshot 声明顺序逐项填充：读取面 10 项 + 页面自持 5 项 + now（入参注入）。
  const data: WbLibraryData = { ...reads, ...page, loading, checking, messagesLoading, loadError: error, now };
  return { data, loading, checking, refresh: load, runCheckup, openSession, closeSession };
}
