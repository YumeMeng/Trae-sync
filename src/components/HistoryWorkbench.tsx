import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Archive,
  ArrowLeft,
  CheckSquare,
  Clock,
  FolderClosed,
  Layers,
  ListChecks,
  Merge,
  MessageSquare,
  RefreshCw,
  Search,
  Square,
  Trash2,
  Undo2,
  X,
} from "lucide-react";
import type { AppPage } from "./NavigationRail";
import type {
  MasterHistoryDto,
  MasterMergeResultDto,
  MasterProjectEntryDto,
  MasterSessionEntryDto,
  RelayLedgerEntryDto,
} from "../types/history";
import type { SessionMessageDto } from "../types/account_switch";
import { safeUiErrorMessage } from "../utils/safeUiError";
import { SessionBatchBar } from "./SessionBatchBar";

// ============================================================================
// P5-3 历史页主库视图（环境模型 Q4/Q5）：项目左栏 + 会话右栏两栏结构。
// ============================================================================
//
// 数据源 = 主库（environments\master 专属 data_dir），按主库当前登录账号的
// project.user_id 过滤（E1b 可见性口径）；会话接力轨迹徽章由接力台账聚合
// （session_id / from_session_id 逐跳链回完整轨迹）。
//
// 准实时新鲜度：每 5 秒带 previous 指纹轮询 get_master_history，指纹未变
// 返回 unchanged（维持现有列表，不重读）；台账仅在主库记录变化时随行刷新。

interface HistoryWorkbenchProps {
  /** 页面可见时才读取主库记录，避免后台 IPC；隐藏时停止轮询。 */
  active: boolean;
  /** 引导跳转（主库未启动/未登录时去环境页处理）。 */
  onNavigate?: (page: AppPage) => void;
  /** P5-8a-2 嵌入主库详情页 tab：不渲染页级标题，仅保留搜索/筛选/刷新工具行。 */
  embedded?: boolean;
}

type TimeRange = "all" | "today" | "week" | "month";

const TIME_RANGES: ReadonlyArray<{ value: TimeRange; label: string }> = [
  { value: "all", label: "全部" },
  { value: "today", label: "今天" },
  { value: "week", label: "近 7 天" },
  { value: "month", label: "近 30 天" },
];

/** 一段接力轨迹：某账号持有该会话的时间段与其间新增消息数。 */
interface RelayLeg {
  readonly userId: string;
  readonly accountName: string | null;
  /** 该腿开始（上一跳交接时刻；首腿未知为 null）。 */
  readonly fromUnixSeconds: number | null;
  /** 该腿结束（交接时刻；当前腿为 null = 进行中）。 */
  readonly toUnixSeconds: number | null;
  readonly messages: number;
}

/** 会话预览弹层状态。 */
interface PreviewState {
  readonly session: MasterSessionEntryDto;
  readonly status: "loading" | "ready" | "error";
  readonly messages: readonly SessionMessageDto[];
}

export function HistoryWorkbench({ active, onNavigate, embedded = false }: HistoryWorkbenchProps) {
  const [history, setHistory] = useState<MasterHistoryDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [ledger, setLedger] = useState<readonly RelayLedgerEntryDto[]>([]);
  const [selectedProject, setSelectedProject] = useState<string>("all");
  const [timeRange, setTimeRange] = useState<TimeRange>("all");
  const [searchText, setSearchText] = useState("");
  const [preview, setPreview] = useState<PreviewState | null>(null);
  // ===== P5-8a 选择模式 + 归档抽屉（Gmail 式：浏览态无框，选择态浮出批量栏）=====
  // 主列表选择态与归档视图选择态各自独立（切换视图时清空，互不串选）。
  const [selectMode, setSelectMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<ReadonlySet<string>>(new Set());
  const [archiveView, setArchiveView] = useState(false);
  const [archiveSelect, setArchiveSelect] = useState(false);
  const [archiveSelectedIds, setArchiveSelectedIds] = useState<ReadonlySet<string>>(new Set());
  const [batchBusy, setBatchBusy] = useState(false);
  const [batchError, setBatchError] = useState<string | null>(null);
  // 删除二次确认（ADR-0018：列明规模，单次确认）。
  const [deleteConfirm, setDeleteConfirm] = useState<readonly MasterSessionEntryDto[] | null>(null);
  // ===== P5-8c 分组合并：左栏项目选择态 + 合并确认弹层 =====
  const [projectSelect, setProjectSelect] = useState(false);
  const [projectSelectedIds, setProjectSelectedIds] = useState<ReadonlySet<string>>(new Set());
  // 待合并分组集合（弹层内选择保留目标；null = 未打开）。
  const [mergeConfirm, setMergeConfirm] = useState<readonly MasterProjectEntryDto[] | null>(null);
  // 合并完成回执（自然语言一句话；进入下一次分组选择时清除）。
  const [mergeNotice, setMergeNotice] = useState<string | null>(null);
  // 轮询指纹基线（上一轮读取成功的三件套指纹；unchanged 预检依据）。
  const fingerprintRef = useRef<MasterHistoryDto["fingerprint"] | null>(null);
  const pollCancelled = useRef(false);

  const load = useCallback(async (force: boolean) => {
    const next = await invoke<MasterHistoryDto>("get_master_history", {
      previous: force ? null : fingerprintRef.current,
    });
    if (next.status === "unchanged") return;
    setHistory(next);
    setLoadError(null);
    if (next.status === "ready") {
      fingerprintRef.current = next.fingerprint;
      // 台账只在主库记录实际变化时随行刷新（切号后轨迹立即更新）。
      const entries = await invoke<RelayLedgerEntryDto[]>("get_relay_ledger");
      setLedger(entries);
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    void load(false).catch((reason: unknown) => {
      if (!cancelled) setLoadError(safeUiErrorMessage(reason, "主库记录暂时不可读取，请稍后重试。"));
    });
    return () => { cancelled = true; };
  }, [active, load]);

  // 指纹轮询：主库被 TRAE 写入（新会话/新消息）时自动刷新两栏列表。
  useEffect(() => {
    if (!active) return;
    pollCancelled.current = false;
    const poll = async () => {
      if (pollCancelled.current) return;
      try {
        await load(false);
      } catch {
        // 瞬态失败静默：下一轮轮询自动重试，不打断当前列表。
      }
    };
    const timer = window.setInterval(() => void poll(), 5000);
    return () => {
      pollCancelled.current = true;
      window.clearInterval(timer);
    };
  }, [active, load]);

  // 打开会话预览（主库消息流只读）。
  const openPreview = useCallback(async (session: MasterSessionEntryDto) => {
    setPreview({ session, status: "loading", messages: [] });
    try {
      const result = await invoke<{
        session_id: string;
        status: "ready" | "no_master_data" | "read_failed";
        messages: SessionMessageDto[];
      }>("get_master_session_messages", { sessionId: session.session_id });
      setPreview((current) =>
        current?.session.session_id === session.session_id
          ? {
              session,
              status: result.status === "ready" ? "ready" : "error",
              messages: result.status === "ready" ? result.messages : [],
            }
          : current,
      );
    } catch {
      setPreview((current) =>
        current?.session.session_id === session.session_id
          ? { session, status: "error", messages: [] }
          : current,
      );
    }
  }, []);

  const closePreview = useCallback(() => setPreview(null), []);

  // ===== P5-8a 归档数据：hidden_status = voice_discussion 的会话 =====

  /** 归档会话集合（仅 voice_discussion；scheduled_task 等原生隐藏值不入库任何视图）。 */
  const archivedSessions = useMemo(() => {
    if (!history || history.status !== "ready") return [];
    return history.sessions.filter(
      (session) => !session.deleted && session.hidden_status === "voice_discussion",
    );
  }, [history]);

  /**
   * 归档树：模式（work/code）→ 项目分组 → 会话。
   * 恢复时 TRAE 侧栏按模式聚合，归档视图保持同构层级方便对位。
   */
  const archiveTree = useMemo(() => {
    const tree = new Map<string, Map<string, MasterSessionEntryDto[]>>();
    for (const session of archivedSessions) {
      const mode = session.work_mode?.trim() || "未标注";
      const group = tree.get(mode) ?? new Map<string, MasterSessionEntryDto[]>();
      const list = group.get(session.project_id) ?? [];
      list.push(session);
      group.set(session.project_id, list);
      tree.set(mode, group);
    }
    return tree;
  }, [archivedSessions]);

  // ===== P5-8a 批量操作：归档 / 恢复 / 真实删除 =====

  /** 统一批量执行骨架：忙态锁 + 成功后强制刷新主库 + 失败走安全文案。 */
  const runBatch = useCallback(
    async (action: () => Promise<unknown>, onDone: () => void) => {
      setBatchBusy(true);
      setBatchError(null);
      try {
        await action();
        onDone();
        // 归档/恢复/删除都直接改主库文件，强制重读让两栏立即反映新状态。
        await load(true);
      } catch (reason: unknown) {
        setBatchError(safeUiErrorMessage(reason, "操作未完成，请稍后重试。"));
      } finally {
        setBatchBusy(false);
      }
    },
    [load],
  );

  /** 主列表：归档所选（hidden_status → voice_discussion，可逆）。 */
  const archiveSelected = useCallback(() => {
    const ids = [...selectedIds];
    if (ids.length === 0) return;
    void runBatch(
      () => invoke("archive_master_sessions", { sessionIds: ids }),
      () => {
        setSelectMode(false);
        setSelectedIds(new Set());
      },
    );
  }, [runBatch, selectedIds]);

  /** 归档视图：恢复所选（hidden_status 还原 NULL，会话归位原分组）。 */
  const restoreSelected = useCallback(() => {
    const ids = [...archiveSelectedIds];
    if (ids.length === 0) return;
    void runBatch(
      () => invoke("restore_master_sessions", { sessionIds: ids }),
      () => {
        setArchiveSelect(false);
        setArchiveSelectedIds(new Set());
      },
    );
  }, [runBatch, archiveSelectedIds]);

  /** 归档视图：删除所选（先弹确认，确认后走后端先备份再删除）。 */
  const deleteSelected = useCallback(() => {
    const targets = archivedSessions.filter((session) => archiveSelectedIds.has(session.session_id));
    if (targets.length === 0) return;
    setDeleteConfirm(targets);
  }, [archivedSessions, archiveSelectedIds]);

  /** 确认删除：真实删除（后端自动创建备份；失败不动原数据）。 */
  const confirmDelete = useCallback(() => {
    if (!deleteConfirm) return;
    const ids = deleteConfirm.map((session) => session.session_id);
    setDeleteConfirm(null);
    void runBatch(
      () => invoke("delete_master_sessions", { sessionIds: ids }),
      () => {
        setArchiveSelect(false);
        setArchiveSelectedIds(new Set());
      },
    );
  }, [runBatch, deleteConfirm]);

  // ===== P5-8c 分组合并：左栏选择态 → 弹层选保留目标 → 后端备份 + 事务改挂 =====

  // 左栏项目条目（P5-8c 合并目标候选；声明前置避免选择态回调引用滞后）。
  const projects = history?.status === "ready" ? history.projects : [];

  /** 进入左栏分组选择态（清掉上一次的回执，浏览态无框）。 */
  const enterProjectSelect = useCallback(() => {
    setMergeNotice(null);
    setProjectSelect(true);
    setProjectSelectedIds(new Set());
  }, []);

  const exitProjectSelect = useCallback(() => {
    setProjectSelect(false);
    setProjectSelectedIds(new Set());
  }, []);

  /** 打开合并确认弹层（至少选 2 个分组才有合并意义）。 */
  const openMergeConfirm = useCallback(() => {
    if (projectSelectedIds.size < 2) return;
    const targets = projects.filter((project) => projectSelectedIds.has(project.project_id));
    if (targets.length < 2) return;
    setMergeConfirm(targets);
  }, [projects, projectSelectedIds]);

  /** 确认合并：其余分组的全部会话并入保留分组（后端先备份再事务改挂）。 */
  const confirmMerge = useCallback(
    (target: MasterProjectEntryDto) => {
      if (!mergeConfirm) return;
      const sourceIds = mergeConfirm
        .filter((project) => project.project_id !== target.project_id)
        .map((project) => project.project_id);
      const targetName = target.name?.trim() || "未命名项目";
      setMergeConfirm(null);
      void runBatch(
        async () => {
          const result = await invoke<MasterMergeResultDto>("merge_master_projects", {
            sourceProjectIds: sourceIds,
            targetProjectId: target.project_id,
          });
          // 回执只用数量与动作结果表述（界面表达纪律）。
          setMergeNotice(
            `已把 ${result.moved_sessions} 个会话并入「${targetName}」，清理 ${result.removed_projects} 个空分组。`,
          );
        },
        () => {
          setProjectSelect(false);
          setProjectSelectedIds(new Set());
          // 合并后当前筛选的分组可能已不存在，回退全部项目视图。
          if (selectedProject !== "all" && sourceIds.includes(selectedProject)) {
            setSelectedProject("all");
          }
        },
      );
    },
    [mergeConfirm, runBatch, selectedProject],
  );

  /** 勾选切换（选择态会话行点击）。 */
  const toggleSelected = useCallback(
    (sessionId: string, current: ReadonlySet<string>, setter: (next: ReadonlySet<string>) => void) => {
      const next = new Set(current);
      if (next.has(sessionId)) next.delete(sessionId);
      else next.add(sessionId);
      setter(next);
    },
    [],
  );

  // Esc 退出选择模式 / 关闭确认弹窗（Gmail 式操作习惯）。
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (deleteConfirm) setDeleteConfirm(null);
      else if (mergeConfirm) setMergeConfirm(null);
      else if (selectMode) {
        setSelectMode(false);
        setSelectedIds(new Set());
      } else if (archiveSelect) {
        setArchiveSelect(false);
        setArchiveSelectedIds(new Set());
      } else if (projectSelect) {
        setProjectSelect(false);
        setProjectSelectedIds(new Set());
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selectMode, archiveSelect, deleteConfirm, mergeConfirm, projectSelect]);

  // 切换主列表/归档视图时清空选择，避免跨视图残留勾选。
  const enterArchiveView = useCallback(() => {
    setArchiveView(true);
    setSelectMode(false);
    setSelectedIds(new Set());
  }, []);
  const exitArchiveView = useCallback(() => {
    setArchiveView(false);
    setArchiveSelect(false);
    setArchiveSelectedIds(new Set());
  }, []);

  // Esc 关闭预览弹层。
  useEffect(() => {
    if (!preview) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") closePreview();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [preview, closePreview]);

  // ===== 台账聚合：session_id → 完整接力轨迹（时间正序）=====

  /** session_id → 台账条目（换腿后身份索引）。 */
  const ledgerBySessionId = useMemo(() => {
    const map = new Map<string, RelayLedgerEntryDto>();
    for (const entry of ledger) map.set(entry.session_id, entry);
    return map;
  }, [ledger]);

  /** 沿 from_session_id 逐跳链回，返回时间正序的交接记录链。 */
  const relayChainBySession = useMemo(() => {
    const chains = new Map<string, RelayLedgerEntryDto[]>();
    for (const target of ledger) {
      const hops: RelayLedgerEntryDto[] = [];
      let cursor: RelayLedgerEntryDto | undefined = target;
      // 防御：异常数据成环时最多回溯台账条数 + 1 跳，避免死循环。
      let guard = ledger.length + 1;
      while (cursor && guard > 0) {
        hops.unshift(cursor);
        // 显式标注断开控制流窄化的推断环（cursor 窄化 ← previousId ← cursor）。
        const previousId: string | null = cursor.from_session_id;
        cursor = previousId ? ledgerBySessionId.get(previousId) : undefined;
        guard -= 1;
      }
      chains.set(target.session_id, hops);
    }
    return chains;
  }, [ledger, ledgerBySessionId]);

  /** 会话接力轨迹（无台账记录 → null，不显示徽章）。 */
  const legsForSession = useCallback(
    (session: MasterSessionEntryDto): readonly RelayLeg[] | null => {
      const hops = relayChainBySession.get(session.session_id);
      if (!hops || hops.length === 0) return null;
      const legs: RelayLeg[] = [];
      // 首腿：首跳之前的全部消息归属交接前账号。
      legs.push({
        userId: hops[0].from_user_id,
        accountName: hops[0].from_account_name,
        fromUnixSeconds: null,
        toUnixSeconds: hops[0].switched_at_unix_seconds,
        messages: hops[0].message_count_at_switch,
      });
      for (let index = 1; index < hops.length; index += 1) {
        const hop = hops[index];
        const previous = hops[index - 1];
        legs.push({
          userId: previous.to_user_id,
          accountName: previous.to_account_name,
          fromUnixSeconds: previous.switched_at_unix_seconds,
          toUnixSeconds: hop.switched_at_unix_seconds,
          messages: Math.max(hop.message_count_at_switch - previous.message_count_at_switch, 0),
        });
      }
      const last = hops[hops.length - 1];
      // 当前腿：最后一跳交接至今（进行中）。
      legs.push({
        userId: last.to_user_id,
        accountName: last.to_account_name,
        fromUnixSeconds: last.switched_at_unix_seconds,
        toUnixSeconds: null,
        messages: Math.max(session.message_count - last.message_count_at_switch, 0),
      });
      return legs;
    },
    [relayChainBySession],
  );

  // ===== 右栏会话过滤（项目 × 时间段 × 搜索）=====

  const visibleSessions = useMemo(() => {
    if (!history || history.status !== "ready") return [];
    const keyword = searchText.trim();
    const nowSeconds = Date.now() / 1000;
    const rangeSeconds: Record<Exclude<TimeRange, "all">, number> = {
      today: 24 * 3600,
      week: 7 * 24 * 3600,
      month: 30 * 24 * 3600,
    };
    return history.sessions.filter((session) => {
      if (session.deleted) return false;
      // P5-8a：归档（voice_discussion）与原生隐藏值（scheduled_task 等）
      // 均不入主列表——与 TRAE 侧栏白名单口径一致，归档会话走归档视图。
      if (session.hidden_status !== null) return false;
      if (selectedProject !== "all" && session.project_id !== selectedProject) return false;
      if (timeRange !== "all") {
        const updated = session.updated_at_unix_seconds;
        if (!updated) return false;
        if (nowSeconds - updated > rangeSeconds[timeRange]) return false;
      }
      if (keyword && !session.title.includes(keyword)) return false;
      return true;
    });
  }, [history, searchText, selectedProject, timeRange]);

  /** 项目统计：会话数 + 参与账号（台账轨迹聚合；归档/隐藏会话不计入）。 */
  const projectStats = useMemo(() => {
    const sessionCount = new Map<string, number>();
    const members = new Map<string, Set<string>>();
    for (const session of history?.sessions ?? []) {
      if (session.deleted || session.hidden_status !== null) continue;
      sessionCount.set(session.project_id, (sessionCount.get(session.project_id) ?? 0) + 1);
      const legs = legsForSession(session);
      if (!legs) continue;
      const set = members.get(session.project_id) ?? new Set<string>();
      for (const leg of legs) set.add(leg.userId);
      members.set(session.project_id, set);
    }
    return { sessionCount, members };
  }, [history, legsForSession]);

  const projectName = useCallback(
    (projectId: string): string => {
      if (projectId === "all") return "全部项目";
      const found = projects.find((project) => project.project_id === projectId);
      // 后端已把哈希名回退为 absolute_path 尾段；空串占位「未命名项目」。
      return found?.name?.trim() || "未命名项目";
    },
    [projects],
  );

  /**
   * 主列表分组：全部项目视图按项目聚合（组序沿用左栏项目顺序，
   * 组内维持后端时间倒序）；选中单个项目时不分组。
   */
  const sessionGroups = useMemo(() => {
    if (selectedProject !== "all") return null;
    const groups: { projectId: string; sessions: MasterSessionEntryDto[] }[] = [];
    const byProject = new Map<string, MasterSessionEntryDto[]>();
    for (const session of visibleSessions) {
      const list = byProject.get(session.project_id) ?? [];
      list.push(session);
      byProject.set(session.project_id, list);
    }
    for (const project of projects) {
      const sessions = byProject.get(project.project_id);
      if (sessions && sessions.length > 0) groups.push({ projectId: project.project_id, sessions });
    }
    return groups;
  }, [visibleSessions, projects, selectedProject]);

  const currentUserId = history?.current_user_id ?? null;

  // ===== P5-8c 合并弹层：分组 → 会话数（含已归档，不含已删除；规模展示） =====
  const mergeSessionCounts = useMemo(() => {
    const counts = new Map<string, number>();
    if (!history || history.status !== "ready") return counts;
    for (const session of history.sessions) {
      if (session.deleted) continue;
      counts.set(session.project_id, (counts.get(session.project_id) ?? 0) + 1);
    }
    return counts;
  }, [history]);

  // ===== 渲染 =====

  const body = () => {
    if (loadError) {
      return (
        <div className="history-empty" data-testid="history-error">
          <p>{loadError}</p>
          <button
            className="btn"
            type="button"
            onClick={() => void load(true).catch(() => undefined)}
          >
            <RefreshCw size={15} aria-hidden="true" />重试
          </button>
        </div>
      );
    }
    if (!history) {
      return <div className="history-empty" data-testid="history-loading">正在读取主库记录…</div>;
    }
    if (history.status === "no_master_data") {
      return (
        <div className="history-empty" data-testid="history-guide">
          <Layers size={26} strokeWidth={1.6} aria-hidden="true" />
          <p>主库还没有对话数据。</p>
          <p className="history-empty__hint">到环境页启动主库并在 TRAE 内登录一次，对话记录会自动出现在这里。</p>
          {onNavigate && (
            <button
              className="btn btn--primary"
              type="button"
              onClick={() => onNavigate("environment")}
              data-testid="history-guide-launch"
            >
              去环境页启动主库
            </button>
          )}
        </div>
      );
    }
    if (history.status === "no_current_account") {
      return (
        <div className="history-empty" data-testid="history-guide">
          <Layers size={26} strokeWidth={1.6} aria-hidden="true" />
          <p>主库尚未登记登录账号。</p>
          <p className="history-empty__hint">在环境页确认主库已登录账号；切换账号后记录仍会保留。</p>
          {onNavigate && (
            <button
              className="btn btn--primary"
              type="button"
              onClick={() => onNavigate("environment")}
            >
              去环境页查看
            </button>
          )}
        </div>
      );
    }
    if (history.status === "read_failed") {
      return (
        <div className="history-empty" data-testid="history-error">
          <p>主库记录暂时无法读取，请稍后重试。</p>
          <button
            className="btn"
            type="button"
            onClick={() => void load(true).catch(() => undefined)}
          >
            <RefreshCw size={15} aria-hidden="true" />重试
          </button>
        </div>
      );
    }
    // ready 且处于归档视图：单栏归档面板（模式 → 项目分组 → 会话，P5-8a）。
    if (archiveView) {
      const allArchivedSelected =
        archivedSessions.length > 0 &&
        archivedSessions.every((session) => archiveSelectedIds.has(session.session_id));
      return (
        <div className="history-body">
          <section className="sess-panel archive-panel" aria-label="已归档会话">
            {archiveSelect ? (
              <SessionBatchBar
                selectedCount={archiveSelectedIds.size}
                totalCount={archivedSessions.length}
                busy={batchBusy}
                allSelected={allArchivedSelected}
                onSelectAll={() =>
                  setArchiveSelectedIds(
                    allArchivedSelected
                      ? new Set()
                      : new Set(archivedSessions.map((session) => session.session_id)),
                  )
                }
                onDone={() => {
                  setArchiveSelect(false);
                  setArchiveSelectedIds(new Set());
                }}
              >
                <button
                  className="btn btn--primary"
                  type="button"
                  onClick={restoreSelected}
                  disabled={batchBusy || archiveSelectedIds.size === 0}
                  data-testid="batch-restore"
                >
                  <Undo2 size={15} aria-hidden="true" />恢复所选
                </button>
                <button
                  className="btn btn--danger"
                  type="button"
                  onClick={deleteSelected}
                  disabled={batchBusy || archiveSelectedIds.size === 0}
                  data-testid="batch-delete"
                >
                  <Trash2 size={15} aria-hidden="true" />删除所选
                </button>
              </SessionBatchBar>
            ) : (
              <div className="sess-panel__head">
                <button
                  className="btn btn--quiet"
                  type="button"
                  onClick={exitArchiveView}
                  data-testid="history-archive-back"
                >
                  <ArrowLeft size={15} aria-hidden="true" />返回
                </button>
                <b>已归档会话</b>
                <span>{archivedSessions.length} 个会话</span>
                <div className="sess-panel__tools">
                  <button
                    className="btn btn--quiet"
                    type="button"
                    onClick={() => setArchiveSelect(true)}
                    disabled={archivedSessions.length === 0}
                    data-testid="archive-select-mode"
                  >
                    <ListChecks size={15} aria-hidden="true" />选择
                  </button>
                </div>
              </div>
            )}
            {batchError && (
              <div className="batch-error" data-testid="batch-error" role="alert">{batchError}</div>
            )}
            <div className="sess-list archive-list" data-testid="history-archive-list">
              {archivedSessions.length === 0 ? (
                <div className="sess-empty" data-testid="archive-empty">没有已归档的会话</div>
              ) : (
                [...archiveTree.entries()].map(([mode, groups]) => (
                  <div className="archive-mode" key={mode} data-testid={`archive-mode-${mode}`}>
                    <div className="archive-mode__head">{modeLabel(mode)}</div>
                    {[...groups.entries()].map(([projectId, sessions]) => (
                      <div className="archive-group" key={projectId}>
                        <div className="archive-group__head">
                          <span>{projectName(projectId)}</span>
                          <span>{sessions.length} 个会话</span>
                        </div>
                        {sessions.map((session) => (
                          <SessionRow
                            key={session.session_id}
                            session={session}
                            testId={`archive-session-${session.session_id}`}
                            selectMode={archiveSelect}
                            checked={archiveSelectedIds.has(session.session_id)}
                            onToggle={(id) => toggleSelected(id, archiveSelectedIds, setArchiveSelectedIds)}
                            onOpen={(target) => void openPreview(target)}
                            legs={legsForSession(session)}
                            currentUserId={currentUserId}
                          />
                        ))}
                      </div>
                    ))}
                  </div>
                ))
              )}
            </div>
          </section>
        </div>
      );
    }
    // ready：两栏结构。
    return (
      <div className="history-body">
        {/* 左栏：项目（数据源 project 表，统计由会话实时聚合；P5-8c 选择态可合并分组） */}
        <aside className="proj-panel" aria-label="项目列表">
          {projectSelect ? (
            // P5-8c 分组选择态：紧凑批量栏（窄栏允许换行，动作 = 合并所选）。
            (() => {
              const allProjectsSelected =
                projects.length > 0 && projectSelectedIds.size === projects.length;
              return (
                <div
                  className="batch-bar batch-bar--proj"
                  role="toolbar"
                  aria-label="合并分组"
                  data-testid="project-batch-bar"
                >
                  <button
                    className="btn btn--quiet"
                    type="button"
                    onClick={() =>
                      setProjectSelectedIds(
                        allProjectsSelected
                          ? new Set()
                          : new Set(projects.map((project) => project.project_id)),
                      )
                    }
                    disabled={batchBusy}
                    data-testid="project-batch-select-all"
                  >
                    {allProjectsSelected ? (
                      <CheckSquare size={15} aria-hidden="true" />
                    ) : (
                      <Square size={15} aria-hidden="true" />
                    )}
                    {allProjectsSelected ? "取消全选" : "全选"}
                  </button>
                  <span className="batch-bar__count" data-testid="project-batch-count">
                    已选 {projectSelectedIds.size} / {projects.length}
                  </span>
                  <div className="batch-bar__actions">
                    <button
                      className="btn btn--primary"
                      type="button"
                      onClick={openMergeConfirm}
                      disabled={batchBusy || projectSelectedIds.size < 2}
                      title={projectSelectedIds.size < 2 ? "至少选择 2 个分组才能合并" : undefined}
                      data-testid="batch-merge"
                    >
                      <Merge size={15} aria-hidden="true" />合并
                    </button>
                  </div>
                  <button
                    className="btn btn--quiet"
                    type="button"
                    onClick={exitProjectSelect}
                    disabled={batchBusy}
                    aria-label="退出分组选择"
                    data-testid="project-batch-done"
                  >
                    <X size={15} aria-hidden="true" />
                  </button>
                </div>
              );
            })()
          ) : (
            <div className="proj-panel__head">
              <b>项目</b>
              <span>{projects.length} 个项目 · {visibleSessions.length} 个会话</span>
              <div className="proj-panel__tools">
                <button
                  className="btn btn--quiet"
                  type="button"
                  onClick={enterProjectSelect}
                  disabled={projects.length < 2}
                  title={projects.length < 2 ? "至少 2 个分组才能合并" : "合并分组"}
                  data-testid="project-merge-entry"
                >
                  <Merge size={15} aria-hidden="true" />合并
                </button>
              </div>
            </div>
          )}
          {mergeNotice && (
            <div className="batch-notice" role="status" data-testid="merge-notice">{mergeNotice}</div>
          )}
          <div className="proj-list" role="list">
            {!projectSelect && (
              <button
                type="button"
                role="listitem"
                className={`proj-item ${selectedProject === "all" ? "proj-item--active" : ""}`}
                onClick={() => setSelectedProject("all")}
                data-testid="history-project-all"
              >
                <span className="proj-item__icon" aria-hidden="true"><Layers size={15} strokeWidth={1.8} /></span>
                <span className="proj-item__main">
                  <span className="proj-item__name">全部项目</span>
                  <span className="proj-item__meta">{projectStats.sessionCount.size ? [...projectStats.sessionCount.values()].reduce((a, b) => a + b, 0) : 0} 会话 · 全部记录</span>
                </span>
              </button>
            )}
            {projects.map((project) => {
              const count = projectStats.sessionCount.get(project.project_id) ?? 0;
              const memberIds = [...(projectStats.members.get(project.project_id) ?? [])];
              const projectChecked = projectSelectedIds.has(project.project_id);
              // 选择态高亮勾选行，浏览态高亮当前筛选（互斥展示）。
              const rowState = projectSelect
                ? projectChecked
                  ? "proj-item--checked"
                  : ""
                : selectedProject === project.project_id
                  ? "proj-item--active"
                  : "";
              return (
                <button
                  key={project.project_id}
                  type="button"
                  role="listitem"
                  className={`proj-item ${rowState}`}
                  onClick={() =>
                    projectSelect
                      ? toggleSelected(project.project_id, projectSelectedIds, setProjectSelectedIds)
                      : setSelectedProject(project.project_id)
                  }
                  aria-pressed={projectSelect ? projectChecked : undefined}
                  // 项目文件夹路径收进悬浮提示，不占主视野（界面表达纪律）。
                  title={project.absolute_path?.trim() || undefined}
                  data-testid={`history-project-${project.project_id}`}
                >
                  <span className="proj-item__icon" aria-hidden="true">
                    {projectSelect ? (
                      projectChecked ? <CheckSquare size={15} strokeWidth={1.8} /> : <Square size={15} strokeWidth={1.8} />
                    ) : (
                      <FolderClosed size={15} strokeWidth={1.8} />
                    )}
                  </span>
                  <span className="proj-item__main">
                    <span className="proj-item__name">{project.name?.trim() || "未命名项目"}</span>
                    <span className="proj-item__meta">{count} 会话</span>
                  </span>
                  {!projectSelect && memberIds.length > 0 && (
                    <span className="relay-chain relay-chain--compact" aria-hidden="true">
                      {memberIds.slice(0, 3).map((userId) => (
                        <RelayAvatar key={userId} userId={userId} name={nameOfLeg(ledger, userId)} />
                      ))}
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        </aside>

        {/* 右栏：会话（数据源 chat_session + 接力台账轨迹徽章） */}
        <section className="sess-panel" aria-label="会话列表">
          {(() => {
            const allVisibleSelected =
              visibleSessions.length > 0 &&
              visibleSessions.every((session) => selectedIds.has(session.session_id));
            return selectMode ? (
              <SessionBatchBar
                selectedCount={selectedIds.size}
                totalCount={visibleSessions.length}
                busy={batchBusy}
                allSelected={allVisibleSelected}
                onSelectAll={() =>
                  setSelectedIds(
                    allVisibleSelected
                      ? new Set()
                      : new Set(visibleSessions.map((session) => session.session_id)),
                  )
                }
                onDone={() => {
                  setSelectMode(false);
                  setSelectedIds(new Set());
                }}
              >
                <button
                  className="btn btn--primary"
                  type="button"
                  onClick={archiveSelected}
                  disabled={batchBusy || selectedIds.size === 0}
                  data-testid="batch-archive"
                >
                  <Archive size={15} aria-hidden="true" />归档所选
                </button>
              </SessionBatchBar>
            ) : (
              <div className="sess-panel__head">
                <b>{projectName(selectedProject)}</b>
                <span>{visibleSessions.length} 个会话</span>
                <div className="sess-panel__tools">
                  <button
                    className="btn btn--quiet"
                    type="button"
                    onClick={() => setSelectMode(true)}
                    disabled={visibleSessions.length === 0}
                    data-testid="history-select-mode"
                  >
                    <ListChecks size={15} aria-hidden="true" />选择
                  </button>
                  <button
                    className="btn btn--quiet"
                    type="button"
                    onClick={enterArchiveView}
                    data-testid="history-archive-entry"
                  >
                    <Archive size={15} aria-hidden="true" />
                    {archivedSessions.length > 0 ? `已归档 ${archivedSessions.length}` : "已归档"}
                  </button>
                </div>
              </div>
            );
          })()}
          {batchError && (
            <div className="batch-error" data-testid="batch-error" role="alert">{batchError}</div>
          )}
          <div className="sess-list" data-testid="history-session-list">
            {visibleSessions.length === 0 ? (
              <div className="sess-empty" data-testid="history-empty-sessions">
                {history.sessions.length === 0
                  ? "当前账号还没有对话记录"
                  : "没有符合条件的会话"}
              </div>
            ) : sessionGroups ? (
              // 全部项目视图：按项目分组（组头 = 项目名 + 会话数）。
              sessionGroups.map((group) => (
                <div
                  className="sess-group"
                  key={group.projectId}
                  data-testid={`history-session-group-${group.projectId}`}
                >
                  <div className="sess-group__head">
                    <span>{projectName(group.projectId)}</span>
                    <span>{group.sessions.length} 个会话</span>
                  </div>
                  {group.sessions.map((session) => (
                    <SessionRow
                      key={session.session_id}
                      session={session}
                      testId={`history-session-${session.session_id}`}
                      selectMode={selectMode}
                      checked={selectedIds.has(session.session_id)}
                      onToggle={(id) => toggleSelected(id, selectedIds, setSelectedIds)}
                      onOpen={(target) => void openPreview(target)}
                      legs={legsForSession(session)}
                      currentUserId={currentUserId}
                    />
                  ))}
                </div>
              ))
            ) : (
              visibleSessions.map((session) => (
                <SessionRow
                  key={session.session_id}
                  session={session}
                  testId={`history-session-${session.session_id}`}
                  selectMode={selectMode}
                  checked={selectedIds.has(session.session_id)}
                  onToggle={(id) => toggleSelected(id, selectedIds, setSelectedIds)}
                  onOpen={(target) => void openPreview(target)}
                  legs={legsForSession(session)}
                  currentUserId={currentUserId}
                />
              ))
            )}
          </div>
        </section>
      </div>
    );
  };

  // 工具行（搜索/时间范围/刷新）：页面态嵌在页级 header 内，嵌入态独立成行。
  const actionsBar = (
    <div className="page-header__actions">
      <div className="history-search">
        <Search size={14} aria-hidden="true" />
        <input
          type="text"
          placeholder="搜索会话标题…"
          value={searchText}
          onChange={(event) => setSearchText(event.target.value)}
          data-testid="history-search-input"
          aria-label="搜索会话标题"
        />
      </div>
      <div className="history-chips" role="group" aria-label="时间范围">
        {TIME_RANGES.map((range) => (
          <button
            key={range.value}
            type="button"
            className={`history-chip ${timeRange === range.value ? "history-chip--active" : ""}`}
            onClick={() => setTimeRange(range.value)}
            data-testid={`history-time-${range.value}`}
          >
            {range.label}
          </button>
        ))}
      </div>
      <button
        className="btn"
        type="button"
        onClick={() => void load(true).catch(() => undefined)}
        title="同步主库最新记录"
        data-testid="history-refresh"
      >
        <RefreshCw size={15} aria-hidden="true" />刷新
      </button>
    </div>
  );

  return (
    <section
      className={`history-page${embedded ? " history-page--embedded" : ""}`}
      role="region"
      aria-label={embedded ? "对话列表" : "历史"}
    >
      {embedded ? (
        actionsBar
      ) : (
        <header className="page-header">
          <div className="page-header__copy">
            <h1 data-page-title="history" tabIndex={-1}>历史</h1>
            <p>主库对话记录 · 自动同步，无需授权扫描</p>
          </div>
          {actionsBar}
        </header>
      )}

      {body()}

      {preview && (
        <SessionPreview
          preview={preview}
          projectName={projectName(preview.session.project_id)}
          legs={legsForSession(preview.session)}
          currentUserId={currentUserId}
          onClose={closePreview}
        />
      )}

      {/* P5-8a 删除二次确认：列明会话数与消息量（ADR-0018 单次确认） */}
      {deleteConfirm && (
        <DeleteConfirmDialog
          sessions={deleteConfirm}
          onCancel={() => setDeleteConfirm(null)}
          onConfirm={confirmDelete}
        />
      )}

      {/* P5-8c 合并确认弹层：选择保留分组（破坏性批量迁移，单次确认） */}
      {mergeConfirm && (
        <MergeConfirmDialog
          projects={mergeConfirm}
          sessionCounts={mergeSessionCounts}
          onCancel={() => setMergeConfirm(null)}
          onConfirm={confirmMerge}
        />
      )}
    </section>
  );
}

// ============================================================================
// P5-8a 会话行（主列表与归档视图共用）：选择态显示勾选框，点击切换勾选
// ============================================================================

function SessionRow({
  session,
  testId,
  selectMode,
  checked,
  onToggle,
  onOpen,
  legs,
  currentUserId,
}: {
  session: MasterSessionEntryDto;
  testId: string;
  selectMode: boolean;
  checked: boolean;
  onToggle: (sessionId: string) => void;
  onOpen: (session: MasterSessionEntryDto) => void;
  legs: readonly RelayLeg[] | null;
  currentUserId: string | null;
}) {
  return (
    <button
      type="button"
      className={`sess-row ${selectMode ? "sess-row--select" : ""} ${checked ? "sess-row--checked" : ""}`}
      onClick={() => (selectMode ? onToggle(session.session_id) : onOpen(session))}
      aria-pressed={selectMode ? checked : undefined}
      data-testid={testId}
    >
      {selectMode && (
        <span className="sess-row__check" aria-hidden="true">
          {checked ? <CheckSquare size={16} /> : <Square size={16} />}
        </span>
      )}
      <span className="sess-row__main">
        <span className="sess-row__title">{session.title?.trim() || "未命名会话"}</span>
        <span className="sess-row__meta">
          <span><Clock size={11} aria-hidden="true" />{formatSessionTime(session.updated_at_unix_seconds)}</span>
          <span><MessageSquare size={11} aria-hidden="true" />{session.message_count} 条</span>
        </span>
      </span>
      {legs && <RelayChain legs={legs} currentUserId={currentUserId} />}
    </button>
  );
}

// ============================================================================
// P5-8a 删除确认弹窗：列明规模，确认后执行真实删除（后端先备份）
// ============================================================================

function DeleteConfirmDialog({
  sessions,
  onCancel,
  onConfirm,
}: {
  sessions: readonly MasterSessionEntryDto[];
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const totalMessages = sessions.reduce((sum, session) => sum + session.message_count, 0);
  return (
    <div
      className="preview-veil preview-veil--open"
      onClick={(event) => {
        if (event.target === event.currentTarget) onCancel();
      }}
      data-testid="delete-confirm"
    >
      <div
        className="preview confirm-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="确认删除会话"
      >
        <div className="preview__head">
          <div className="preview__head-main">
            <div className="preview__title">删除 {sessions.length} 个会话</div>
            <div className="preview__meta">
              <span>共 {totalMessages} 条消息</span>
              <span>删除前会自动创建主库数据备份</span>
            </div>
          </div>
          <button
            className="btn"
            type="button"
            onClick={onCancel}
            data-testid="delete-confirm-cancel"
          >
            <X size={15} aria-hidden="true" />取消
          </button>
        </div>
        <div className="preview__body">
          <p className="confirm-dialog__text">
            删除后这些会话将从主库永久移除，无法在应用内恢复（备份文件保留，可人工恢复）。
          </p>
          <ul className="confirm-dialog__list">
            {sessions.map((session) => (
              <li key={session.session_id} data-testid={`delete-confirm-item-${session.session_id}`}>
                {session.title?.trim() || "未命名会话"}
                <span> · {session.message_count} 条消息</span>
              </li>
            ))}
          </ul>
        </div>
        <div className="confirm-dialog__foot">
          <button className="btn" type="button" onClick={onCancel}>
            取消
          </button>
          <button
            className="btn btn--danger"
            type="button"
            onClick={onConfirm}
            data-testid="delete-confirm-ok"
          >
            <Trash2 size={15} aria-hidden="true" />确认删除
          </button>
        </div>
      </div>
    </div>
  );
}

// ============================================================================
// P5-8c 合并确认弹窗：选择保留的分组，其余分组会话全部并入（后端先备份）
// ============================================================================

function MergeConfirmDialog({
  projects,
  sessionCounts,
  onCancel,
  onConfirm,
}: {
  /** 待合并分组集合（其中一个被选为保留目标）。 */
  projects: readonly MasterProjectEntryDto[];
  /** 分组 → 会话数（含已归档，不含已删除；弹层规模展示）。 */
  sessionCounts: ReadonlyMap<string, number>;
  onCancel: () => void;
  onConfirm: (target: MasterProjectEntryDto) => void;
}) {
  // 默认保留列表第一个分组；用户可在弹层内改选。
  const [targetId, setTargetId] = useState(projects[0]?.project_id ?? "");
  const target = projects.find((project) => project.project_id === targetId) ?? projects[0];
  // 将被移动的会话总量 = 非保留分组的会话数之和。
  const movedSessions = projects
    .filter((project) => project.project_id !== target?.project_id)
    .reduce((sum, project) => sum + (sessionCounts.get(project.project_id) ?? 0), 0);
  return (
    <div
      className="preview-veil preview-veil--open"
      onClick={(event) => {
        if (event.target === event.currentTarget) onCancel();
      }}
      data-testid="merge-confirm"
    >
      <div
        className="preview confirm-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="确认合并分组"
      >
        <div className="preview__head">
          <div className="preview__head-main">
            <div className="preview__title">合并 {projects.length} 个分组</div>
            <div className="preview__meta">
              <span>{movedSessions} 个会话将移入保留的分组（含已归档）</span>
              <span>合并前会自动创建主库数据备份</span>
            </div>
          </div>
          <button
            className="btn"
            type="button"
            onClick={onCancel}
            data-testid="merge-confirm-cancel"
          >
            <X size={15} aria-hidden="true" />取消
          </button>
        </div>
        <div className="preview__body">
          <p className="confirm-dialog__text">
            选择要保留的分组：其余分组的全部会话都会移入它，移空的分组会被清理。误合并可用备份人工恢复。
          </p>
          <ul className="confirm-dialog__list confirm-dialog__list--choice">
            {projects.map((project) => (
              <li key={project.project_id}>
                <label className="merge-choice">
                  <input
                    type="radio"
                    name="merge-target"
                    checked={project.project_id === target?.project_id}
                    onChange={() => setTargetId(project.project_id)}
                    data-testid={`merge-target-${project.project_id}`}
                  />
                  <span className="merge-choice__name">{project.name?.trim() || "未命名项目"}</span>
                  <span className="merge-choice__meta">
                    {sessionCounts.get(project.project_id) ?? 0} 个会话
                  </span>
                </label>
              </li>
            ))}
          </ul>
        </div>
        <div className="confirm-dialog__foot">
          <button className="btn" type="button" onClick={onCancel}>
            取消
          </button>
          <button
            className="btn btn--primary"
            type="button"
            onClick={() => target && onConfirm(target)}
            data-testid="merge-confirm-ok"
          >
            <Merge size={15} aria-hidden="true" />确认合并
          </button>
        </div>
      </div>
    </div>
  );
}

/** 归档视图模式标签（work_mode 列值 → 展示名）。 */
function modeLabel(mode: string): string {
  if (mode === "work") return "Work 模式";
  if (mode === "code") return "Code 模式";
  if (mode === "未标注") return "未标注模式";
  return mode;
}

// ============================================================================
// 会话预览弹层：接力轨迹时间线 + 主库消息流
// ============================================================================

function SessionPreview({
  preview,
  projectName,
  legs,
  currentUserId,
  onClose,
}: {
  preview: PreviewState;
  projectName: string;
  legs: readonly RelayLeg[] | null;
  currentUserId: string | null;
  onClose: () => void;
}) {
  const { session } = preview;
  return (
    <div
      className="preview-veil preview-veil--open"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
      data-testid="history-preview"
    >
      <div
        className="preview"
        role="dialog"
        aria-modal="true"
        aria-label={`会话预览：${session.title || "未命名会话"}`}
      >
        <div className="preview__head">
          <div className="preview__head-main">
            <div className="preview__title">{session.title?.trim() || "未命名会话"}</div>
            <div className="preview__meta">
              <span>{projectName}</span>
              <span>{session.message_count} 条消息</span>
              <span>最后活动 {formatSessionTime(session.updated_at_unix_seconds)}</span>
            </div>
          </div>
          <button
            className="btn"
            type="button"
            onClick={onClose}
            data-testid="history-preview-close"
            aria-label="关闭预览"
          >
            <X size={15} aria-hidden="true" />关闭
          </button>
        </div>
        <div className="preview__body" data-testid="preview-content">
          {legs && legs.length > 0 && (
            <>
              <div className="preview__section">接力记录（谁在什么时候用过这条会话）</div>
              <div className="leg-timeline">
                {legs.map((leg, index) => (
                  <div className="leg-step" key={`${leg.userId}-${index}`}>
                    <div className="leg-step__rail">
                      <span className="leg-step__dot">
                        <RelayAvatar userId={leg.userId} name={leg.accountName} />
                      </span>
                      <span className="leg-step__line" />
                    </div>
                    <div className="leg-step__body">
                      <div className="leg-step__name">
                        {leg.accountName ?? "已移除账号"}
                        {currentUserId !== null && leg.userId === currentUserId && (
                          <span className="leg-step__badge">当前账号</span>
                        )}
                      </div>
                      <div className="leg-step__meta">
                        {formatLegSpan(leg)} · {leg.messages} 条消息
                      </div>
                    </div>
                  </div>
                ))}
              </div>
              <div className="preview__section preview__section--gap">消息预览</div>
            </>
          )}
          {preview.status === "loading" && <div className="sess-empty">正在读取消息…</div>}
          {preview.status === "error" && (
            <div className="sess-empty">消息暂时无法读取，请稍后重试。</div>
          )}
          {preview.status === "ready" && preview.messages.length === 0 && (
            <div className="sess-empty">该会话没有可展示的消息。</div>
          )}
          {preview.status === "ready" &&
            preview.messages.map((message) => (
              <div
                className={`msg ${message.role === "user" ? "msg--user" : ""}`}
                key={message.message_id}
              >
                <div className="msg__role">{message.role === "user" ? "用户" : "AI"}</div>
                {message.content.kind === "text" ? (
                  <div className="msg__text">{message.content.text}</div>
                ) : (
                  <div className="msg__text msg__text--trace">
                    任务轨迹 · {message.content.step_count} 步
                    {message.content.thoughts.length > 0 && (
                      <ul className="msg__thoughts">
                        {message.content.thoughts.map((thought, index) => (
                          <li key={index}>{thought}</li>
                        ))}
                      </ul>
                    )}
                  </div>
                )}
              </div>
            ))}
        </div>
      </div>
    </div>
  );
}

// ============================================================================
// 接力轨迹徽章：头像链 + 悬停浮层（fixed 定位 + 视口钳制，末行不被裁剪）
// ============================================================================

function RelayChain({
  legs,
  currentUserId,
}: {
  legs: readonly RelayLeg[];
  currentUserId: string | null;
}) {
  const chainRef = useRef<HTMLSpanElement>(null);
  const popRef = useRef<HTMLSpanElement>(null);

  const handleMouseEnter = () => {
    const chain = chainRef.current;
    const pop = popRef.current;
    if (!chain || !pop) return;
    const rect = chain.getBoundingClientRect();
    // 先临时可见取实际尺寸（不触发重排闪烁：visibility 隐藏期间测量）。
    pop.style.visibility = "hidden";
    pop.classList.add("relay-pop--measuring");
    const width = pop.offsetWidth || 240;
    const height = pop.offsetHeight || 120;
    pop.classList.remove("relay-pop--measuring");
    pop.style.visibility = "";
    const left = Math.max(8, Math.min(rect.left, window.innerWidth - width - 12));
    const top =
      rect.bottom + 8 + height > window.innerHeight - 8
        ? Math.max(8, rect.top - height - 8) // 下方放不下时向上展开
        : rect.bottom + 8;
    pop.style.left = `${left}px`;
    pop.style.top = `${top}px`;
  };

  const show = legs.slice(0, 3);
  const rest = legs.length - show.length;

  return (
    <span
      className="relay-chain"
      ref={chainRef}
      onMouseEnter={handleMouseEnter}
      data-testid="history-relay-chain"
    >
      {show.map((leg, index) => (
        <RelayAvatar
          key={`${leg.userId}-${index}`}
          userId={leg.userId}
          name={leg.accountName}
        />
      ))}
      {rest > 0 && <span className="relay-chain__more">+{rest}</span>}
      <span className="relay-pop" ref={popRef}>
        <span className="relay-pop__title">接力记录 · {legs.length} 个账号使用过</span>
        {legs.map((leg, index) => (
          <span className="relay-pop__row" key={`${leg.userId}-pop-${index}`}>
            <RelayAvatar userId={leg.userId} name={leg.accountName} />
            <span className="relay-pop__name">
              {leg.accountName ?? "已移除账号"}
              {currentUserId !== null && leg.userId === currentUserId && "（当前账号）"}
            </span>
            <span className="relay-pop__meta">
              {formatLegSpan(leg)}
              <br />
              {leg.messages} 条消息
            </span>
          </span>
        ))}
      </span>
    </span>
  );
}

// ============================================================================
// 工具：头像、时间格式化
// ============================================================================

/** 账号头像：首字符 + 按 user_id 哈希的稳定色调（同一账号跨页面颜色一致）。 */
function RelayAvatar({ userId, name }: { userId: string; name: string | null }) {
  const letter = (name ?? "").trim().charAt(0).toUpperCase() || "?";
  let hash = 0;
  for (let index = 0; index < userId.length; index += 1) {
    hash = (hash * 31 + userId.charCodeAt(index)) >>> 0;
  }
  return (
    <span className={`relay-avatar relay-avatar--t${hash % 6}`} aria-hidden="true">
      {letter}
    </span>
  );
}

/** 从台账反查账号显示名（项目参与头像用；无记录返回 null）。 */
function nameOfLeg(ledger: readonly RelayLedgerEntryDto[], userId: string): string | null {
  for (const entry of ledger) {
    if (entry.from_user_id === userId) return entry.from_account_name;
    if (entry.to_user_id === userId) return entry.to_account_name;
  }
  return null;
}

/** 会话时间：今天/昨天/MM-DD HH:mm（跨年补年份）。 */
function formatSessionTime(unixSeconds: number | null): string {
  if (!unixSeconds) return "时间未知";
  const date = new Date(unixSeconds * 1000);
  const now = new Date();
  const pad = (value: number) => String(value).padStart(2, "0");
  const hhmm = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  const startOfDay = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const dayDiff = Math.round((startOfDay(now) - startOfDay(date)) / 86_400_000);
  if (dayDiff === 0) return `今天 ${hhmm}`;
  if (dayDiff === 1) return `昨天 ${hhmm}`;
  const sameYear = date.getFullYear() === now.getFullYear();
  const monthDay = `${date.getMonth() + 1}月${date.getDate()}日`;
  return sameYear ? `${monthDay} ${hhmm}` : `${date.getFullYear()}年${monthDay} ${hhmm}`;
}

/** 接力腿时间段：交接时刻区间（进行中腿显示「至今」）。 */
function formatLegSpan(leg: RelayLeg): string {
  const from = leg.fromUnixSeconds === null ? "最早" : formatSessionTime(leg.fromUnixSeconds);
  const to = leg.toUnixSeconds === null ? "至今" : formatSessionTime(leg.toUnixSeconds);
  return `${from} → ${to}`;
}
