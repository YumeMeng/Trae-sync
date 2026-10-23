import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { fetchMasterVerification } from "../../../platform/library";
import { notify } from "../../../platform/notify";
import { safeUiErrorMessage } from "../../../utils/safeUiError";
import { LIBRARY_TEXTS } from "../config";
import type { VerificationState } from "../infoTab";

/**
 * 库信息 tab 的备份对比读取（P4e-8b-3）：进 tab 首读 / 基准切换重读 / 重新校验。
 * 旧实现逐条对照 src/components/MasterVerificationPanel.tsx（下称 V）：
 * V42-47 挂载守卫、V49-67 load（回显实际基准 + 失败页内错误行）、V69-73 「tab 可见才读」的 active 语义、
 * V75-80 切换基准（清报告后按新基准重读）。
 * 命令经 platform/library（本文件零 invoke）；读取失败同时给页内错误行（V96）与一次失败通知，保留上次报告。
 * 读取时机：active 由「页面可见 + tab=info」双条件决定——切走即停读，切回按当前基准重读（旧页同口径）。
 */
export interface VerificationInput {
  /** 库实例 id（缺省主库；副库 / 只读数据根按库读取）。 */
  readonly libraryId: string;
  /** 库信息 tab 当前可见（页面 active 且 tab=info）：不可见不发起请求。 */
  readonly active: boolean;
}
export interface VerificationResult {
  readonly state: VerificationState;
  /** 工具行动作出口（verification-refresh）：命中返回 true。 */
  readonly handleAction: (actionId: string) => boolean;
  /** 对比基准切换（null = 最新备份）：清报告并按新基准重读。 */
  readonly chooseBase: (stamp: number | null) => void;
}
const REFRESH_ACTION = "verification-refresh";

export function useLibraryVerification(input: VerificationInput): VerificationResult {
  const { libraryId, active } = input;
  const [report, setReport] = useState<VerificationState["report"]>(null);
  const [stamp, setStamp] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ticket = useRef(0);
  const latest = useRef({ libraryId, active }); latest.current = { libraryId, active };
  // 换库即清态：不把上个库的备份对比挂到新库上（代次票同时作废在途读取）。
  useLayoutEffect(() => {
    ticket.current += 1;
    setReport(null); setStamp(null); setError(null); setLoading(false);
  }, [libraryId]);
  const load = useCallback(async (baseStamp: number | null): Promise<void> => {
    if (!latest.current.active) return;
    const id = latest.current.libraryId, current = ++ticket.current;
    setLoading(true);
    try {
      const next = await fetchMasterVerification({ backupStamp: baseStamp, ...(id === "master" ? {} : { libraryId: id }) });
      if (current !== ticket.current) return; // 晚到的旧请求：结果已作废
      setReport(next); setError(null);
      setStamp(next.backup.backup_stamp); // 回显实际使用的基准（stamp 无效时后端回退最新，V58-59）
    } catch (reason: unknown) {
      if (current !== ticket.current) return;
      const message = safeUiErrorMessage(reason, LIBRARY_TEXTS.verifyLoadFailure);
      setError(message); // 保留上次报告（V61-64）：页内错误行与通知同时给，不清已取回数据
      notify.error(message);
    } finally { if (current === ticket.current) setLoading(false); }
  }, []);
  // 进 tab 自动执行；基准变化（用户切基准）时重查（V69-73 逐字语义）。
  useEffect(() => {
    if (!active) return;
    void load(stamp);
    return () => { ticket.current += 1; }; // 切走 / 卸载：在途读取作废
  }, [active, stamp, load]);
  const chooseBase = useCallback((next: number | null) => {
    setReport(null); // 旧 V76-79：切换基准先清报告，避免展示另一基准的旧差异
    setStamp(next);
  }, []);
  const handleAction = useCallback((actionId: string): boolean => {
    if (actionId !== REFRESH_ACTION) return false;
    void load(stamp); // 重新校验（V84-93）
    return true;
  }, [load, stamp]);
  return { state: { report, stamp, loading, error }, handleAction, chooseBase };
}
