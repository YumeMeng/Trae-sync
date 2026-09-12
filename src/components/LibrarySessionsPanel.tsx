import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Archive,
  ArrowLeft,
  Bot,
  CheckSquare,
  ChevronDown,
  ChevronRight,
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
  MasterArchiveApplyResultDto,
  MasterMergeResultDto,
  MasterProjectEntryDto,
  MasterSessionEntryDto,
  RelayLedgerEntryDto,
} from "../types/history";
import type { SessionMessageDto } from "../types/account_switch";
import { safeUiErrorMessage } from "../utils/safeUiError";

// ============================================================================
// 库对话面板（G21 两栏式 + G22 统一选择模式，ADR-0025 Library 抽象）
// ============================================================================
//
// 左栏 = 项目树（项目行原地展开会话子级；「未关联文件夹」末位合并组；
// 顶部搜索；底部固定区 = 已归档入口 + 选择按钮）。
// 右栏 = 对话查看器（常驻，取代旧预览弹层）：消息 tab 聊天流按接力账号
// 着色（chat_message 不带发送者身份，归属按接力台账时间轴推导），
// 接力 tab 展示持有时间线。
//
// 数据链路按库实例参数化（ADR-0025）：宿主页注入 library（非敏感 id +
// 显示名），数据目录 / raw key / 当前 user_id 的解析全部留在 Rust 侧。
//
// 准实时新鲜度：每 5 秒带 previous 指纹与账号身份轮询 get_master_history，
// 指纹和账号都未变才返回 unchanged；台账仅在库记录变化时随行刷新。

/** ADR-0025 库描述：宿主页注入的库实例引用（非敏感 id + 显示名）。 */
export interface LibraryRef {
  readonly id: string;
  readonly displayName?: string;
}

interface LibrarySessionsPanelProps {
  /** 页面可见时才读取库记录，避免后台 IPC；隐藏时停止轮询。 */
  active: boolean;
  /** 引导跳转（库未启动/未登录时去环境页处理）。 */
  onNavigate?: (page: AppPage) => void;
  /** 嵌入库详情页 tab：不渲染页级标题。 */
  embedded?: boolean;
  /** 库实例（缺省主库；副库落地时同一组件零改动复用）。 */
  library?: LibraryRef;
}

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

/** 右栏查看器状态（选中会话 + 消息流读取进度）。 */
interface ViewerState {
  readonly session: MasterSessionEntryDto;
  readonly status: "loading" | "ready" | "error";
  readonly messages: readonly SessionMessageDto[];
  readonly hasMoreBefore: boolean;
  readonly messageOffset: number;
  readonly loadingOlder: boolean;
}

/** 「未关联文件夹」合并组哨兵 id（G19：空名项目归并一组，会话平铺）。 */
const UNLINKED_GROUP_ID = "__unlinked__";

/** 消息流分页大小（后端按时间倒序取页后反转为升序）。 */
const CHAT_WINDOW_LIMIT = 2000;

/** 归档草稿存储键前缀；按库隔离，应用重启后仍可继续调整。 */
const ARCHIVE_DRAFT_STORAGE_PREFIX = "trae-sync:archive-draft:";

interface ArchiveDraft {
  readonly archiveSessionIds: readonly string[];
  readonly restoreSessionIds: readonly string[];
}

const EMPTY_ARCHIVE_DRAFT: ArchiveDraft = {
  archiveSessionIds: [],
  restoreSessionIds: [],
};

function archiveDraftStorageKey(libraryId: string): string {
  return `${ARCHIVE_DRAFT_STORAGE_PREFIX}${encodeURIComponent(libraryId)}`;
}

/** 读取本地草稿；坏数据按空草稿处理，不能阻断历史列表。 */
function readArchiveDraft(libraryId: string): ArchiveDraft {
  try {
    const raw = window.localStorage.getItem(archiveDraftStorageKey(libraryId));
    if (!raw) return EMPTY_ARCHIVE_DRAFT;
    const value: unknown = JSON.parse(raw);
    if (!value || typeof value !== "object") return EMPTY_ARCHIVE_DRAFT;
    const record = value as { archiveSessionIds?: unknown; restoreSessionIds?: unknown };
    const strings = (candidate: unknown): string[] =>
      Array.isArray(candidate)
        ? [...new Set(candidate.filter((item): item is string => typeof item === "string"))]
        : [];
    const archiveSessionIds = strings(record.archiveSessionIds);
    const restoreSessionIds = strings(record.restoreSessionIds).filter(
      (id) => !archiveSessionIds.includes(id),
    );
    return { archiveSessionIds, restoreSessionIds };
  } catch {
    return EMPTY_ARCHIVE_DRAFT;
  }
}

export function LibrarySessionsPanel({
  active,
  onNavigate,
  embedded = false,
  library = { id: "master" },
}: LibrarySessionsPanelProps) {
  // 库显示名（文案随库实例变化，ADR-0025；缺省主库）。
  const libName = library.displayName?.trim() || "主库";

  const [history, setHistory] = useState<MasterHistoryDto | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [ledger, setLedger] = useState<readonly RelayLedgerEntryDto[]>([]);
  // 左栏搜索（过滤树内会话标题）。
  const [searchText, setSearchText] = useState("");
  // 项目树展开状态（本地记忆；搜索时自动全展开哨兵除外）。
  const [expandedIds, setExpandedIds] = useState<ReadonlySet<string>>(new Set());
  // 归档视图（同一棵树灰显，层级 模式 → 项目 → 会话）。
  const [archiveView, setArchiveView] = useState(false);
  // 右栏查看器（null = 空态引导）。
  const [viewer, setViewer] = useState<ViewerState | null>(null);
  const [viewerTab, setViewerTab] = useState<"messages" | "relay">("messages");
  // ===== G22 统一选择模式：一套勾选状态同时覆盖项目行（合并源）与会话行 =====
  const [selectMode, setSelectMode] = useState(false);
  const [selectedSessionIds, setSelectedSessionIds] = useState<ReadonlySet<string>>(new Set());
  const [selectedProjectIds, setSelectedProjectIds] = useState<ReadonlySet<string>>(new Set());
  const [batchBusy, setBatchBusy] = useState(false);
  const [batchError, setBatchError] = useState<string | null>(null);
  // 归档/恢复先落在本地草稿，用户点击应用时才一次性写入主库。
  const [archiveDraft, setArchiveDraft] = useState<ArchiveDraft>(() =>
    readArchiveDraft(library.id),
  );
  // 操作回执（归档/恢复/删除/合并完成后的一句话提示，行内展示）。
  const [actionNotice, setActionNotice] = useState<string | null>(null);
  // 删除二次确认（ADR-0018：列明规模，单次确认）。
  const [deleteConfirm, setDeleteConfirm] = useState<readonly MasterSessionEntryDto[] | null>(null);
  // 合并确认弹层（两步：选保留目标 → 确认页）。
  const [mergeConfirm, setMergeConfirm] = useState<readonly MasterProjectEntryDto[] | null>(null);
  // 轮询指纹基线（上一轮读取成功的三件套指纹；unchanged 预检依据）。
  const fingerprintRef = useRef<MasterHistoryDto["fingerprint"] | null>(null);
  // 指纹相同但主库切号时文件可能未变，身份也必须参与 unchanged 判定。
  const currentUserIdRef = useRef<string | null>(null);
  const pollCancelled = useRef(false);

  const archiveDraftHasChanges =
    archiveDraft.archiveSessionIds.length > 0 || archiveDraft.restoreSessionIds.length > 0;

  // 切换库实例时切换对应草稿；草稿不因组件卸载或应用重启丢失。
  useEffect(() => {
    setArchiveDraft(readArchiveDraft(library.id));
  }, [library.id]);

  useEffect(() => {
    try {
      const key = archiveDraftStorageKey(library.id);
      if (!archiveDraftHasChanges) window.localStorage.removeItem(key);
      else window.localStorage.setItem(key, JSON.stringify(archiveDraft));
    } catch {
      // 本地存储不可用时仍允许本次会话继续操作，提交失败不会隐藏草稿。
    }
  }, [archiveDraft, archiveDraftHasChanges, library.id]);

  const load = useCallback(
    async (force: boolean) => {
      const next = await invoke<MasterHistoryDto>("get_master_history", {
        previous: force ? null : fingerprintRef.current,
        previousCurrentUserId: force ? null : currentUserIdRef.current,
        libraryId: library.id,
      });
      if (next.status === "unchanged") return;
      setHistory(next);
      setLoadError(null);
      fingerprintRef.current = next.fingerprint;
      currentUserIdRef.current = next.status === "ready" ? next.current_user_id : null;
      if (next.status === "ready") {
        // 台账只在库记录实际变化时随行刷新（切号后轨迹立即更新）。
        const entries = await invoke<RelayLedgerEntryDto[]>("get_relay_ledger", {
          libraryId: library.id,
        });
        setLedger(entries);
      } else {
        setLedger([]);
      }
    },
    [library.id],
  );

  // 切换库实例时不能复用上一库的指纹或账号身份。
  useEffect(() => {
    fingerprintRef.current = null;
    currentUserIdRef.current = null;
    setHistory(null);
    setLedger([]);
    setViewer(null);
  }, [library.id]);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    void load(false).catch((reason: unknown) => {
      if (!cancelled) setLoadError(safeUiErrorMessage(reason, "库记录暂时不可读取，请稍后重试。"));
    });
    return () => {
      cancelled = true;
    };
  }, [active, load]);

  // 指纹轮询：库被 TRAE 写入（新会话/新消息）时自动刷新两栏。
  useEffect(() => {
    if (!active) return;
    pollCancelled.current = false;
    const poll = async () => {
      if (pollCancelled.current) return;
      try {
        await load(false);
      } catch {
        // 瞬态失败静默：下一轮轮询自动重试，不打断当前树。
      }
    };
    const timer = window.setInterval(() => void poll(), 5000);
    return () => {
      pollCancelled.current = true;
      window.clearInterval(timer);
    };
  }, [active, load]);

  // ===== 右栏查看器：选中会话 → 读取消息流 =====

  const openSession = useCallback(
    async (session: MasterSessionEntryDto) => {
      setViewerTab("messages");
      setViewer({
        session,
        status: "loading",
        messages: [],
        hasMoreBefore: false,
        messageOffset: 0,
        loadingOlder: false,
      });
      try {
        const result = await invoke<{
          session_id: string;
          status: "ready" | "no_master_data" | "read_failed";
          messages: SessionMessageDto[];
          has_more?: boolean;
        }>("get_master_session_messages", { sessionId: session.session_id, libraryId: library.id });
        const messages = result.status === "ready" ? result.messages : [];
        setViewer((current) =>
          current?.session.session_id === session.session_id
            ? {
                session,
                status: result.status === "ready" ? "ready" : "error",
                messages,
                hasMoreBefore:
                  result.status === "ready" &&
                  (result.has_more ?? messages.length === CHAT_WINDOW_LIMIT),
                messageOffset: messages.length,
                loadingOlder: false,
              }
            : current,
        );
      } catch {
        setViewer((current) =>
          current?.session.session_id === session.session_id
            ? {
                session,
                status: "error",
                messages: [],
                hasMoreBefore: false,
                messageOffset: 0,
                loadingOlder: false,
              }
            : current,
        );
      }
    },
    [library.id],
  );

  const closeViewer = useCallback(() => setViewer(null), []);

  /** 查看器滚到顶部时继续取更早的一页，并把新消息拼到当前窗口前面。 */
  const loadOlderMessages = useCallback(async () => {
    const current = viewer;
    if (
      !current ||
      current.status !== "ready" ||
      !current.hasMoreBefore ||
      current.loadingOlder
    ) {
      return;
    }
    setViewer((previous) =>
      previous?.session.session_id === current.session.session_id
        ? { ...previous, loadingOlder: true }
        : previous,
    );
    try {
      const result = await invoke<{
        status: "ready" | "no_master_data" | "read_failed";
        messages: SessionMessageDto[];
        has_more?: boolean;
      }>("get_master_session_messages", {
        sessionId: current.session.session_id,
        libraryId: library.id,
        offset: current.messageOffset,
      });
      setViewer((previous) => {
        if (!previous || previous.session.session_id !== current.session.session_id) return previous;
        const older = result.status === "ready" ? result.messages : [];
        return {
          ...previous,
          messages: [...older, ...previous.messages],
          messageOffset: previous.messageOffset + older.length,
          hasMoreBefore:
            result.status === "ready" &&
            (result.has_more ?? older.length === CHAT_WINDOW_LIMIT),
          loadingOlder: false,
        };
      });
    } catch {
      setViewer((previous) =>
        previous?.session.session_id === current.session.session_id
          ? { ...previous, loadingOlder: false }
          : previous,
      );
    }
  }, [viewer, library.id]);

  // ===== 树数据：正常视图会话 / 归档会话 / 分组 =====

  const projects = history?.status === "ready" ? history.projects : [];
  const currentUserId = history?.current_user_id ?? null;
  const keyword = searchText.trim();

  /** 正常视图会话：按草稿计算的最终显示状态，尚未提交也即时反映。 */
  const liveSessions = useMemo(() => {
    if (!history || history.status !== "ready") return [];
    const archiveIds = new Set(archiveDraft.archiveSessionIds);
    const restoreIds = new Set(archiveDraft.restoreSessionIds);
    return history.sessions.filter(
      (session) =>
        !session.deleted &&
        (restoreIds.has(session.session_id) ||
          (session.hidden_status === null && !archiveIds.has(session.session_id))),
    );
  }, [history, archiveDraft]);

  /** 归档会话集合：包含待提交归档，不包含待提交恢复。 */
  const archivedSessions = useMemo(() => {
    if (!history || history.status !== "ready") return [];
    const archiveIds = new Set(archiveDraft.archiveSessionIds);
    const restoreIds = new Set(archiveDraft.restoreSessionIds);
    return history.sessions.filter(
      (session) =>
        !session.deleted &&
        !restoreIds.has(session.session_id) &&
        (archiveIds.has(session.session_id) || session.hidden_status === "voice_discussion"),
    );
  }, [history, archiveDraft]);

  /** 搜索过滤后的树内会话（输入即过滤）。 */
  const treeSessions = useMemo(
    () => (keyword ? liveSessions.filter((session) => session.title.includes(keyword)) : liveSessions),
    [liveSessions, keyword],
  );

  const namedProjectIds = useMemo(
    () => new Set(projects.filter((project) => project.name.trim()).map((project) => project.project_id)),
    [projects],
  );

  /**
   * 项目树分组：具名项目（仅含有匹配会话的）+「未关联文件夹」末位合并组
   * （G19：空名项目归并一组，会话平铺不做项目二级展示）。
   */
  const treeGroups = useMemo(() => {
    const byProject = new Map<string, MasterSessionEntryDto[]>();
    for (const session of treeSessions) {
      const list = byProject.get(session.project_id) ?? [];
      list.push(session);
      byProject.set(session.project_id, list);
    }
    const named: { id: string; name: string; path: string | null; sessions: MasterSessionEntryDto[] }[] =
      [];
    const unlinked: MasterSessionEntryDto[] = [];
    for (const project of projects) {
      const sessions = byProject.get(project.project_id) ?? [];
      if (!project.name.trim()) {
        unlinked.push(...sessions);
      } else if (sessions.length > 0) {
        named.push({
          id: project.project_id,
          name: project.name,
          path: project.absolute_path,
          sessions,
        });
      }
    }
    return { named, unlinked };
  }, [projects, treeSessions]);

  /**
   * 归档树：模式（work/code/未标注）→ 项目 → 会话。
   * 恢复时 TRAE 侧栏按模式聚合，归档视图保持同构层级方便对位（ADR-0022）。
   */
  const archiveTree = useMemo(() => {
    const tree = new Map<string, Map<string, MasterSessionEntryDto[]>>();
    for (const session of archivedSessions) {
      const mode = session.work_mode?.trim() || "未标注";
      const group = tree.get(mode) ?? new Map<string, MasterSessionEntryDto[]>();
      // 空名项目只是数据库层的多个技术记录，归档视图按用户可理解的
      // 「未关联文件夹」合并，避免一个会话占一个同名文件夹。
      const projectGroupId = namedProjectIds.has(session.project_id)
        ? session.project_id
        : UNLINKED_GROUP_ID;
      const list = group.get(projectGroupId) ?? [];
      list.push(session);
      group.set(projectGroupId, list);
      tree.set(mode, group);
    }
    // 未关联组始终放在各模式末位，和正常视图的排布保持一致。
    for (const groups of tree.values()) {
      const unlinked = groups.get(UNLINKED_GROUP_ID);
      if (unlinked) {
        groups.delete(UNLINKED_GROUP_ID);
        groups.set(UNLINKED_GROUP_ID, unlinked);
      }
    }
    return tree;
  }, [archivedSessions, namedProjectIds]);

  /** 项目显示名：空名项目统一占位「未关联文件夹」。 */
  const projectName = useCallback(
    (projectId: string): string => {
      const found = projects.find((project) => project.project_id === projectId);
      return found?.name?.trim() || "未关联文件夹";
    },
    [projects],
  );

  /** 「未关联文件夹」是多个真实项目的展示合并组，操作时展开为真实项目 ID。 */
  const unlinkedProjectIds = useMemo(
    () => projects.filter((project) => !project.name.trim()).map((project) => project.project_id),
    [projects],
  );

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

  /** 会话接力轨迹（无台账记录 → null）。 */
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

  /**
   * 会话腿集合（着色与接力 tab 数据源）：无台账记录 → 当前账号单一腿
   * （正常会话切号后归当前账号；归档会话不参与切号）。
   */
  const legsOfSession = useCallback(
    (session: MasterSessionEntryDto): readonly RelayLeg[] => {
      const legs = legsForSession(session);
      if (legs) return legs;
      const fallbackName = currentUserId
        ? (nameOfLedger(ledger, currentUserId) ?? "当前账号")
        : "当前账号";
      return [
        {
          userId: currentUserId ?? "",
          accountName: fallbackName,
          fromUnixSeconds: null,
          toUnixSeconds: null,
          messages: session.message_count,
        },
      ];
    },
    [legsForSession, currentUserId, ledger],
  );

  // ===== G22 统一选择模式与批量操作 =====

  /** 统一批量执行骨架：忙态锁 + 成功后强制刷新 + 回执/失败走行内提示。 */
  const runBatch = useCallback(
    async (action: () => Promise<unknown>, onDone: () => void, receipt: string | null) => {
      setBatchBusy(true);
      setBatchError(null);
      try {
        await action();
        onDone();
        // 回执为空时保留 action 内按后端结果生成的提示（合并流）。
        if (receipt) setActionNotice(receipt);
        // 归档/恢复/删除/合并都直接改库文件，强制重读让两栏立即反映新状态。
        await load(true);
      } catch (reason: unknown) {
        setBatchError(safeUiErrorMessage(reason, "操作未完成，请稍后重试。"));
      } finally {
        setBatchBusy(false);
      }
    },
    [load],
  );

  const enterSelectMode = useCallback(() => {
    setActionNotice(null);
    setSelectMode(true);
    setSelectedSessionIds(new Set());
    setSelectedProjectIds(new Set());
  }, []);

  const exitSelectMode = useCallback(() => {
    setSelectMode(false);
    setSelectedSessionIds(new Set());
    setSelectedProjectIds(new Set());
  }, []);

  /** 项目行统一切换选择；展示合并组一次勾选其下全部真实项目。 */
  const toggleProjectSelection = useCallback(
    (projectIds: readonly string[]) => {
      if (projectIds.length === 0) return;
      setSelectedProjectIds((current) => {
        const next = new Set(current);
        const allSelected = projectIds.every((projectId) => next.has(projectId));
        for (const projectId of projectIds) {
          if (allSelected) next.delete(projectId);
          else next.add(projectId);
        }
        return next;
      });
    },
    [],
  );

  /** 勾选切换（项目行/会话行共用）。 */
  const toggleInSet = useCallback(
    (id: string, current: ReadonlySet<string>, setter: (next: ReadonlySet<string>) => void) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      setter(next);
    },
    [],
  );

  const toggleExpand = useCallback((projectId: string) => {
    setExpandedIds((current) => {
      const next = new Set(current);
      if (next.has(projectId)) next.delete(projectId);
      else next.add(projectId);
      return next;
    });
  }, []);

  /** 当前视图中项目/会话选择最终作用到的会话集合。 */
  const selectedVisibleSessionIds = useMemo(() => {
    const visibleSessions = archiveView ? archivedSessions : liveSessions;
    return new Set(
      visibleSessions
        .filter(
          (session) =>
            selectedSessionIds.has(session.session_id) || selectedProjectIds.has(session.project_id),
        )
        .map((session) => session.session_id),
    );
  }, [archiveView, archivedSessions, liveSessions, selectedProjectIds, selectedSessionIds]);

  /** 正常视图删除项目时连同其已归档会话一起纳入；归档视图只删当前归档内容。 */
  const selectedDeleteSessions = useMemo(() => {
    if (!history || history.status !== "ready") return [];
    const candidates = archiveView
      ? archivedSessions
      : history.sessions.filter((session) => !session.deleted);
    return candidates.filter(
      (session) =>
        selectedSessionIds.has(session.session_id) || selectedProjectIds.has(session.project_id),
    );
  }, [archiveView, archivedSessions, history, selectedProjectIds, selectedSessionIds]);

  /** 只有真实具名项目可作为合并目标；未关联展示组仅支持归档/恢复/删除。 */
  const selectedMergeProjects = useMemo(
    () => {
      // 选中未关联展示组时不静默忽略它，避免「合并」只作用于部分所选项目。
      if ([...selectedProjectIds].some((projectId) => !namedProjectIds.has(projectId))) return [];
      return projects.filter(
        (project) => selectedProjectIds.has(project.project_id) && project.name.trim(),
      );
    },
    [namedProjectIds, projects, selectedProjectIds],
  );

  /** 把会话加入待归档集合；再次反向操作可撤销同一条草稿变更。 */
  const stageArchiveChange = useCallback(
    (mode: "archive" | "restore", sessionIds: readonly string[]) => {
      if (sessionIds.length === 0) return;
      setArchiveDraft((current) => {
        const archiveIds = new Set(current.archiveSessionIds);
        const restoreIds = new Set(current.restoreSessionIds);
        for (const sessionId of sessionIds) {
          const addTo = mode === "archive" ? archiveIds : restoreIds;
          const removeFrom = mode === "archive" ? restoreIds : archiveIds;
          if (removeFrom.has(sessionId)) removeFrom.delete(sessionId);
          else addTo.add(sessionId);
        }
        return {
          archiveSessionIds: [...archiveIds],
          restoreSessionIds: [...restoreIds],
        };
      });
      setBatchError(null);
      setActionNotice(
        mode === "archive"
          ? `已加入待归档设置，共 ${sessionIds.length} 个会话。完成选择后点击“应用归档设置”。`
          : `已加入待恢复设置，共 ${sessionIds.length} 个会话。完成选择后点击“应用归档设置”。`,
      );
    },
    [],
  );

  /** 归档只改草稿，不立即写数据库或重启 TRAE。 */
  const archiveSessions = useCallback(
    (sessionIds: readonly string[]) => {
      if (sessionIds.length === 0) return;
      stageArchiveChange("archive", sessionIds);
      // 草稿状态下同步关闭被移出正常列表的查看器，避免右栏与左栏不一致。
      if (viewer && sessionIds.includes(viewer.session.session_id)) setViewer(null);
      exitSelectMode();
    },
    [stageArchiveChange, viewer, exitSelectMode],
  );

  /** 归档视图：恢复所选只进入草稿，待用户统一应用。 */
  const restoreSelected = useCallback(() => {
    const ids = [...selectedVisibleSessionIds];
    if (ids.length === 0) return;
    stageArchiveChange("restore", ids);
    exitSelectMode();
  }, [selectedVisibleSessionIds, stageArchiveChange, exitSelectMode]);

  /** 应用整批归档设置：关闭实例、单事务写入、重启并校验启动结果。 */
  const applyArchiveDraft = useCallback(async () => {
    if (!archiveDraftHasChanges || batchBusy) return;
    setBatchBusy(true);
    setBatchError(null);
    try {
      const result = await invoke<MasterArchiveApplyResultDto>("apply_master_archive_changes", {
        archiveSessionIds: [...archiveDraft.archiveSessionIds],
        restoreSessionIds: [...archiveDraft.restoreSessionIds],
        libraryId: library.id,
      });
      const archivedCount = result.archived_sessions;
      const restoredCount = result.restored_sessions;
      setArchiveDraft(EMPTY_ARCHIVE_DRAFT);
      exitSelectMode();
      let refreshFailed = false;
      try {
        await load(true);
      } catch {
        // 数据库已提交，刷新失败不能把已经完成的批次误报为“未提交”。
        refreshFailed = true;
      }
      const resultNotice =
        result.relaunch_outcome === "failed"
          ? `主库已更新（归档 ${archivedCount} 个、恢复 ${restoredCount} 个），实例尚未启动，请重试启动。`
          : `已应用归档设置：归档 ${archivedCount} 个、恢复 ${restoredCount} 个会话。`;
      setActionNotice(
        refreshFailed ? `${resultNotice} 历史列表刷新失败，请手动刷新。` : resultNotice,
      );
    } catch (reason: unknown) {
      setBatchError(safeUiErrorMessage(reason, "归档设置未提交，草稿仍保留。"));
    } finally {
      setBatchBusy(false);
    }
  }, [archiveDraft, archiveDraftHasChanges, batchBusy, exitSelectMode, library.id, load]);

  /** 放弃未提交的归档设置，不触碰主库。 */
  const discardArchiveDraft = useCallback(() => {
    if (batchBusy) return;
    setArchiveDraft(EMPTY_ARCHIVE_DRAFT);
    setBatchError(null);
    setActionNotice("已放弃未提交的归档设置，主库没有变化。");
  }, [batchBusy]);

  /** 删除所选（先弹确认，确认后走后端先备份再删除）。 */
  const openDeleteConfirm = useCallback(() => {
    const targets = selectedDeleteSessions;
    if (targets.length === 0) return;
    setDeleteConfirm(targets);
  }, [selectedDeleteSessions]);

  /** 确认删除：真实删除（后端自动创建备份；失败不动原数据）。 */
  const confirmDelete = useCallback(() => {
    if (!deleteConfirm) return;
    const ids = deleteConfirm.map((session) => session.session_id);
    setDeleteConfirm(null);
    void runBatch(
      () => invoke("delete_master_sessions", { sessionIds: ids, libraryId: library.id }),
      () => {
        if (viewer && ids.includes(viewer.session.session_id)) setViewer(null);
        exitSelectMode();
      },
      `已删除 ${ids.length} 个会话。`,
    );
  }, [deleteConfirm, runBatch, viewer, library.id, exitSelectMode]);

  /** 打开合并确认弹层（至少选 2 个项目才有合并意义）。 */
  const openMergeConfirm = useCallback(() => {
    if (selectedMergeProjects.length < 2) return;
    const targets = selectedMergeProjects;
    if (targets.length < 2) return;
    setMergeConfirm(targets);
  }, [selectedMergeProjects]);

  /** 确认合并：其余项目的全部会话并入保留项目（后端先备份再事务改挂）。 */
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
            libraryId: library.id,
          });
          // 回执只用数量与动作结果表述（界面表达纪律）。
          setActionNotice(
            `已把 ${result.moved_sessions} 个会话并入「${targetName}」，清理 ${result.removed_projects} 个空分组。`,
          );
        },
        () => exitSelectMode(),
        null, // 回执在 action 内按后端结果生成，此处不覆盖。
      );
    },
    [mergeConfirm, runBatch, library.id, exitSelectMode],
  );

  // ===== 视图切换（正常 ↔ 归档）：切换时清空选择，避免跨视图残留勾选。 =====

  const enterArchiveView = useCallback(() => {
    setActionNotice(null);
    setArchiveView(true);
    exitSelectMode();
  }, [exitSelectMode]);

  const exitArchiveView = useCallback(() => {
    setActionNotice(null);
    setArchiveView(false);
    exitSelectMode();
  }, [exitSelectMode]);

  // Esc 关闭确认弹层 / 退出选择模式（Gmail 式操作习惯）。
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      if (deleteConfirm) setDeleteConfirm(null);
      else if (mergeConfirm) setMergeConfirm(null);
      else if (selectMode) exitSelectMode();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [deleteConfirm, mergeConfirm, selectMode, exitSelectMode]);

  // ===== 渲染 =====

  /** 合并弹层规模数据：项目 → 会话数（含已归档，不含已删除）。 */
  const mergeSessionCounts = useMemo(() => {
    const counts = new Map<string, number>();
    if (!history || history.status !== "ready") return counts;
    for (const session of history.sessions) {
      if (session.deleted) continue;
      counts.set(session.project_id, (counts.get(session.project_id) ?? 0) + 1);
    }
    return counts;
  }, [history]);

  /** 正常视图项目分支（选择模式下项目行可勾选，作为合并源）。 */
  const renderBranch = (group: {
    id: string;
    name: string;
    path: string | null;
    sessions: readonly MasterSessionEntryDto[];
  }) => {
    // 搜索时自动展开（结果立即可见）；平时按本地展开状态。
    const expanded = keyword !== "" || expandedIds.has(group.id);
    const projectChecked = selectedProjectIds.has(group.id);
    const activate = () => {
      if (selectMode) {
        toggleProjectSelection([group.id]);
      } else {
        toggleExpand(group.id);
      }
    };
    return (
      <div className="lib-branch" key={group.id}>
        <div
          className={`lib-project${projectChecked ? " lib-project--checked" : ""}`}
          role="button"
          tabIndex={0}
          aria-pressed={selectMode ? projectChecked : undefined}
          // 项目文件夹路径收进悬浮提示，不占主视野（界面表达纪律）。
          title={group.path?.trim() || undefined}
          onClick={activate}
          onKeyDown={(event) => {
            if (event.key !== "Enter" && event.key !== " ") return;
            event.preventDefault();
            activate();
          }}
          data-testid={`library-project-${group.id}`}
        >
          <span className="lib-project__chevron" aria-hidden="true">
            {expanded ? (
              <ChevronDown size={14} strokeWidth={1.8} />
            ) : (
              <ChevronRight size={14} strokeWidth={1.8} />
            )}
          </span>
          {selectMode && (
            <span className="lib-check" aria-hidden="true">
              {projectChecked ? <CheckSquare size={15} /> : <Square size={15} />}
            </span>
          )}
          <span className="lib-project__icon" aria-hidden="true">
            <FolderClosed size={15} strokeWidth={1.8} />
          </span>
          <span className="lib-project__name">{group.name}</span>
          <span className="lib-project__count">{group.sessions.length}</span>
        </div>
        {expanded && (
          <div className="lib-sessions">
            {group.sessions.map((session) => (
              <TreeSessionRow
                key={session.session_id}
                session={session}
                selectMode={selectMode}
                checked={selectedSessionIds.has(session.session_id)}
                showQuickArchive={!selectMode}
                onToggle={(sessionId) =>
                  toggleInSet(sessionId, selectedSessionIds, setSelectedSessionIds)
                }
                onOpen={(target) => void openSession(target)}
                onQuickArchive={(target) => archiveSessions([target.session_id])}
              />
            ))}
          </div>
        )}
      </div>
    );
  };

  /** 「未关联文件夹」合并组（树末位；选择时展开为其真实项目集合）。 */
  const renderUnlinkedBranch = (sessions: readonly MasterSessionEntryDto[]) => {
    const expanded = keyword !== "" || expandedIds.has(UNLINKED_GROUP_ID);
    const projectChecked =
      unlinkedProjectIds.length > 0 && unlinkedProjectIds.every((projectId) => selectedProjectIds.has(projectId));
    const activate = () => {
      if (selectMode) toggleProjectSelection(unlinkedProjectIds);
      else toggleExpand(UNLINKED_GROUP_ID);
    };
    return (
      <div className="lib-branch" key={UNLINKED_GROUP_ID}>
        <div
          className="lib-project"
          role="button"
          tabIndex={0}
          aria-expanded={expanded}
          aria-pressed={selectMode ? projectChecked : undefined}
          onClick={activate}
          onKeyDown={(event) => {
            if (event.key !== "Enter" && event.key !== " ") return;
            event.preventDefault();
            activate();
          }}
          data-testid={`library-project-${UNLINKED_GROUP_ID}`}
        >
          <span className="lib-project__chevron" aria-hidden="true">
            {expanded ? (
              <ChevronDown size={14} strokeWidth={1.8} />
            ) : (
              <ChevronRight size={14} strokeWidth={1.8} />
            )}
          </span>
          {selectMode && (
            <span className="lib-check" aria-hidden="true">
              {projectChecked ? <CheckSquare size={15} /> : <Square size={15} />}
            </span>
          )}
          <span className="lib-project__icon" aria-hidden="true">
            <FolderClosed size={15} strokeWidth={1.8} />
          </span>
          <span className="lib-project__name">未关联文件夹</span>
          <span className="lib-project__count">{sessions.length}</span>
        </div>
        {expanded && (
          <div className="lib-sessions">
            {sessions.map((session) => (
              <TreeSessionRow
                key={session.session_id}
                session={session}
                selectMode={selectMode}
                checked={selectedSessionIds.has(session.session_id)}
                showQuickArchive={!selectMode}
                onToggle={(sessionId) =>
                  toggleInSet(sessionId, selectedSessionIds, setSelectedSessionIds)
                }
                onOpen={(target) => void openSession(target)}
                onQuickArchive={(target) => archiveSessions([target.session_id])}
              />
            ))}
          </div>
        )}
      </div>
    );
  };

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
      return (
        <div className="history-empty" data-testid="history-loading">
          正在读取库记录…
        </div>
      );
    }
    if (history.status === "no_master_data") {
      return (
        <div className="history-empty" data-testid="history-guide">
          <Layers size={26} strokeWidth={1.6} aria-hidden="true" />
          <p>{libName}还没有对话数据。</p>
          <p className="history-empty__hint">到环境页启动{libName}并在 TRAE 内登录一次，对话记录会自动出现在这里。</p>
          {onNavigate && (
            <button
              className="btn btn--primary"
              type="button"
              onClick={() => onNavigate("environment")}
              data-testid="history-guide-launch"
            >
              去环境页启动
            </button>
          )}
        </div>
      );
    }
    if (history.status === "no_current_account") {
      return (
        <div className="history-empty" data-testid="history-guide">
          <Layers size={26} strokeWidth={1.6} aria-hidden="true" />
          <p>{libName}尚未登记登录账号。</p>
          <p className="history-empty__hint">在环境页确认{libName}已登录账号；切换账号后记录仍会保留。</p>
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
          <p>{libName}记录暂时无法读取，请稍后重试。</p>
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

    // ready：左栏项目树 + 右栏对话查看器。
    return (
      <div className="history-body">
        <aside className="lib-panel" aria-label="对话列表">
          {archiveView ? (
            <div className="lib-back">
              <button
                className="btn btn--quiet"
                type="button"
                onClick={exitArchiveView}
                data-testid="library-archive-back"
              >
                <ArrowLeft size={15} aria-hidden="true" />返回对话列表
              </button>
            </div>
          ) : (
            <div className="lib-search">
              <Search size={14} aria-hidden="true" />
              <input
                type="text"
                placeholder="搜索会话标题…"
                value={searchText}
                onChange={(event) => setSearchText(event.target.value)}
                aria-label="搜索会话标题"
                data-testid="library-search-input"
              />
            </div>
          )}
          {batchError && (
            <div className="lib-notice lib-notice--error" data-testid="batch-error" role="alert">
              {batchError}
            </div>
          )}
          {actionNotice && (
            <div className="lib-notice" data-testid="action-notice" role="status">
              {actionNotice}
            </div>
          )}

          <div
            className={`lib-tree${archiveView ? " lib-tree--archive" : ""}`}
            data-testid="library-tree"
          >
            {archiveView
              ? // 归档视图：模式 → 项目/未关联文件夹 → 会话（低饱和灰显）。
                [...archiveTree.entries()].map(([mode, groups]) => (
                  <div className="lib-arch-mode" key={mode}>
                    <div className="lib-arch-mode__head">{modeLabel(mode)}</div>
                    {[...groups.entries()].map(([projectId, sessions]) => {
                      const selectableProjectIds =
                        projectId === UNLINKED_GROUP_ID ? unlinkedProjectIds : [projectId];
                      const projectChecked =
                        selectableProjectIds.length > 0 &&
                        selectableProjectIds.every((id) => selectedProjectIds.has(id));
                      const activate = () => {
                        if (selectMode) toggleProjectSelection(selectableProjectIds);
                      };
                      return (
                        <div className="lib-branch" key={projectId}>
                          <div
                            className={`lib-project${projectChecked ? " lib-project--checked" : ""}`}
                            role={selectMode ? "button" : undefined}
                            tabIndex={selectMode ? 0 : undefined}
                            aria-pressed={selectMode ? projectChecked : undefined}
                            onClick={selectMode ? activate : undefined}
                            onKeyDown={(event) => {
                              if (!selectMode || (event.key !== "Enter" && event.key !== " ")) return;
                              event.preventDefault();
                              activate();
                            }}
                            data-testid={`library-archive-project-${projectId}`}
                          >
                            <span className="lib-project__icon" aria-hidden="true">
                              <FolderClosed size={15} strokeWidth={1.8} />
                            </span>
                            {selectMode && (
                              <span className="lib-check" aria-hidden="true">
                                {projectChecked ? <CheckSquare size={15} /> : <Square size={15} />}
                              </span>
                            )}
                            <span className="lib-project__name">{projectName(projectId)}</span>
                            <span className="lib-project__count">{sessions.length}</span>
                          </div>
                          <div className="lib-sessions">
                            {sessions.map((session) => (
                              <TreeSessionRow
                                key={session.session_id}
                                session={session}
                                selectMode={selectMode}
                                checked={selectedSessionIds.has(session.session_id)}
                                showQuickArchive={false}
                                onToggle={(sessionId) =>
                                  toggleInSet(sessionId, selectedSessionIds, setSelectedSessionIds)
                                }
                                onOpen={(target) => void openSession(target)}
                                onQuickArchive={() => undefined}
                              />
                            ))}
                          </div>
                        </div>
                      );
                    })}
                  </div>
                ))
              : // 正常视图：项目树（原地展开会话子级）。
                [
                  ...treeGroups.named.map((group) => renderBranch(group)),
                  ...(treeGroups.unlinked.length > 0
                    ? [renderUnlinkedBranch(treeGroups.unlinked)]
                    : []),
                ]}
            {!archiveView && keyword && treeSessions.length === 0 && (
              <div className="lib-tree__empty">没有匹配的会话</div>
            )}
            {!archiveView && !keyword && liveSessions.length === 0 && (
              <div className="lib-tree__empty">还没有对话记录</div>
            )}
            {archiveView && archivedSessions.length === 0 && (
              <div className="lib-tree__empty" data-testid="library-archive-empty">
                没有已归档的会话
              </div>
            )}
          </div>

          {/* 底部固定区：已归档入口（有归档内容才显示）+ 选择/取消 */}
          <div className="lib-foot">
            {!archiveView && archivedSessions.length > 0 && (
              <button
                className="lib-archive-entry"
                type="button"
                onClick={enterArchiveView}
                data-testid="library-archive-entry"
              >
                <Archive size={14} aria-hidden="true" />
                <span>已归档</span>
                <span className="lib-archive-entry__count">{archivedSessions.length}</span>
              </button>
            )}
            <button
              className="btn btn--quiet lib-select-btn"
              type="button"
              onClick={selectMode ? exitSelectMode : enterSelectMode}
              data-testid="library-select-mode"
            >
              {selectMode ? (
                <>
                  <X size={15} aria-hidden="true" />取消
                </>
              ) : (
                <>
                  <ListChecks size={15} aria-hidden="true" />选择
                </>
              )}
            </button>
          </div>
        </aside>

        {/* 右栏：对话查看器（常驻；选择模式下顶部提示只读预览）。 */}
        <section className="viewer-panel" aria-label="对话查看器">
          {selectMode && (
            <div className="viewer-roam" data-testid="library-select-hint">
              选择模式：预览只读
            </div>
          )}
          {viewer ? (
            <SessionViewer
              key={viewer.session.session_id}
              viewer={viewer}
              projectName={projectName(viewer.session.project_id)}
              legs={legsOfSession(viewer.session)}
              tab={viewerTab}
              onTabChange={setViewerTab}
              onClose={closeViewer}
              onLoadOlder={loadOlderMessages}
            />
          ) : (
            <div className="viewer-empty" data-testid="library-viewer-empty">
              <MessageSquare size={30} strokeWidth={1.4} aria-hidden="true" />
              <p>从左侧选择一个会话查看对话</p>
            </div>
          )}
        </section>
      </div>
    );
  };

  return (
    <section
      className={`history-page${embedded ? " history-page--embedded" : ""}`}
      role="region"
      aria-label={embedded ? "对话列表" : "历史"}
    >
      {!embedded && (
        <header className="page-header">
          <div className="page-header__copy">
            <h1 data-page-title="history" tabIndex={-1}>
              历史
            </h1>
          </div>
        </header>
      )}

      {body()}

      {/* G22 Gmail 式浮动操作栏：选择模式浮出（fixed），按钮随视图切换 */}
      {selectMode && history?.status === "ready" && (
        <div
          className="lib-actionbar"
          role="toolbar"
          aria-label="批量操作"
          data-testid="library-action-bar"
        >
          <span className="lib-actionbar__count" data-testid="batch-count">
            已选 {selectedSessionIds.size} 个会话 · {selectedProjectIds.size} 个项目
          </span>
          <div className="lib-actionbar__actions">
            {archiveView ? (
              <>
                <button
                  className="btn btn--primary"
                  type="button"
                  onClick={restoreSelected}
                  disabled={batchBusy || selectedVisibleSessionIds.size === 0}
                  data-testid="batch-restore"
                >
                  <Undo2 size={15} aria-hidden="true" />恢复
                </button>
                <button
                  className="btn btn--danger"
                  type="button"
                  onClick={openDeleteConfirm}
                  disabled={batchBusy || selectedDeleteSessions.length === 0}
                  data-testid="batch-delete"
                >
                  <Trash2 size={15} aria-hidden="true" />删除
                </button>
              </>
            ) : (
              <>
                <button
                  className="btn btn--primary"
                  type="button"
                  onClick={() => archiveSessions([...selectedVisibleSessionIds])}
                  disabled={batchBusy || selectedVisibleSessionIds.size === 0}
                  data-testid="batch-archive"
                >
                  <Archive size={15} aria-hidden="true" />归档
                </button>
                <button
                  className="btn"
                  type="button"
                  onClick={openMergeConfirm}
                  disabled={batchBusy || selectedMergeProjects.length < 2}
                  title={
                    selectedMergeProjects.length < 2 ? "至少选择 2 个有名称的项目才能合并" : undefined
                  }
                  data-testid="batch-merge"
                >
                  <Merge size={15} aria-hidden="true" />合并到…
                </button>
                <button
                  className="btn btn--danger"
                  type="button"
                  onClick={openDeleteConfirm}
                  disabled={batchBusy || selectedDeleteSessions.length === 0}
                  data-testid="batch-delete"
                >
                  <Trash2 size={15} aria-hidden="true" />删除
                </button>
              </>
            )}
          </div>
        </div>
      )}

      {archiveDraftHasChanges && history?.status === "ready" && (
        <div
          className="lib-actionbar lib-actionbar--draft"
          role="toolbar"
          aria-label="待提交的归档设置"
          data-testid="archive-draft-bar"
        >
          <span className="lib-actionbar__count">
            待提交：归档 {archiveDraft.archiveSessionIds.length} 个 · 恢复 {archiveDraft.restoreSessionIds.length} 个
          </span>
          <div className="lib-actionbar__actions">
            <button
              className="btn"
              type="button"
              onClick={discardArchiveDraft}
              disabled={batchBusy}
              data-testid="archive-draft-discard"
            >
              放弃设置
            </button>
            <button
              className="btn btn--primary"
              type="button"
              onClick={() => void applyArchiveDraft()}
              disabled={batchBusy}
              data-testid="archive-draft-apply"
            >
              {batchBusy ? "正在应用…" : "应用归档设置"}
            </button>
          </div>
        </div>
      )}

      {/* 删除二次确认：列明会话数与消息量（ADR-0018 单次确认） */}
      {deleteConfirm && (
        <DeleteConfirmDialog
          sessions={deleteConfirm}
          onCancel={() => setDeleteConfirm(null)}
          onConfirm={confirmDelete}
        />
      )}

      {/* 合并确认弹层：选择保留项目（破坏性批量迁移，单次确认） */}
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
// 树内会话行：选择态显示勾选框，浏览态点击选中进查看器（悬浮快捷归档）
// ============================================================================

function TreeSessionRow({
  session,
  selectMode,
  checked,
  showQuickArchive,
  onToggle,
  onOpen,
  onQuickArchive,
}: {
  session: MasterSessionEntryDto;
  selectMode: boolean;
  checked: boolean;
  /** 浏览态（非选择模式、非归档视图）悬浮快捷归档按钮。 */
  showQuickArchive: boolean;
  onToggle: (sessionId: string) => void;
  onOpen: (session: MasterSessionEntryDto) => void;
  onQuickArchive: (session: MasterSessionEntryDto) => void;
}) {
  const activate = () => (selectMode ? onToggle(session.session_id) : onOpen(session));
  return (
    <div
      className={`lib-session${checked ? " lib-session--checked" : ""}`}
      role="button"
      tabIndex={0}
      aria-pressed={selectMode ? checked : undefined}
      onClick={activate}
      onKeyDown={(event) => {
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        activate();
      }}
      data-testid={`library-session-${session.session_id}`}
    >
      {selectMode && (
        <span className="lib-check" aria-hidden="true">
          {checked ? <CheckSquare size={15} /> : <Square size={15} />}
        </span>
      )}
      <span className="lib-session__title">{session.title?.trim() || "未命名会话"}</span>
      <span className="lib-session__time">{formatRelativeTime(session.updated_at_unix_seconds)}</span>
      {showQuickArchive && (
        // 浏览态悬浮快捷归档：单会话直接归档，不进选择模式。
        <button
          className="lib-session__quick"
          type="button"
          onClick={(event) => {
            event.stopPropagation();
            onQuickArchive(session);
          }}
          title="归档这个会话"
          aria-label={`归档会话：${session.title?.trim() || "未命名会话"}`}
          data-testid={`library-quick-archive-${session.session_id}`}
        >
          <Archive size={13} aria-hidden="true" />
        </button>
      )}
    </div>
  );
}

// ============================================================================
// 右栏对话查看器：头部（标题/项目/规模 + 关闭）+ 消息|接力 tab
// ============================================================================

function SessionViewer({
  viewer,
  projectName,
  legs,
  tab,
  onTabChange,
  onClose,
  onLoadOlder,
}: {
  viewer: ViewerState;
  projectName: string;
  legs: readonly RelayLeg[];
  tab: "messages" | "relay";
  onTabChange: (tab: "messages" | "relay") => void;
  onClose: () => void;
  onLoadOlder: () => Promise<void>;
}) {
  const { session } = viewer;
  // 账号色板索引：按腿顺序首次出现排，蓝→绿→琥珀循环（G21）。
  const colorIndices = accountColorIndices(legs);
  return (
    <div className="viewer" data-testid="library-viewer">
      <div className="viewer-head">
        <div className="viewer-head__main">
          <div className="viewer-head__title" data-testid="library-viewer-title">
            {session.title?.trim() || "未命名会话"}
          </div>
          <div className="viewer-head__meta">
            <span>{projectName}</span>
            <span>
              <MessageSquare size={11} aria-hidden="true" />
              {session.message_count} 条消息
            </span>
            <span>
              <Clock size={11} aria-hidden="true" />
              最后活动 {formatSessionTime(session.updated_at_unix_seconds)}
            </span>
          </div>
        </div>
        <button
          className="btn viewer-close"
          type="button"
          onClick={onClose}
          aria-label="关闭对话"
          data-testid="library-viewer-close"
        >
          <X size={15} aria-hidden="true" />
        </button>
      </div>
      <div className="viewer-tabs" role="tablist" aria-label="对话内容">
        <button
          type="button"
          role="tab"
          aria-selected={tab === "messages"}
          className={`viewer-tab${tab === "messages" ? " viewer-tab--active" : ""}`}
          onClick={() => onTabChange("messages")}
          data-testid="library-tab-messages"
        >
          消息
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === "relay"}
          className={`viewer-tab${tab === "relay" ? " viewer-tab--active" : ""}`}
          onClick={() => onTabChange("relay")}
          data-testid="library-tab-relay"
        >
          接力
        </button>
      </div>
      <div className="viewer-body">
        {tab === "messages" ? (
          <MessagesTab
            viewer={viewer}
            legs={legs}
            colorIndices={colorIndices}
            onLoadOlder={onLoadOlder}
          />
        ) : (
          <RelayTab legs={legs} colorIndices={colorIndices} />
        )}
      </div>
    </div>
  );
}

/** 消息 tab：聊天流（用户右对齐账号色、助手左对齐中性色），默认滚到底部。 */
function MessagesTab({
  viewer,
  legs,
  colorIndices,
  onLoadOlder,
}: {
  viewer: ViewerState;
  legs: readonly RelayLeg[];
  colorIndices: ReadonlyMap<string, number>;
  onLoadOlder: () => Promise<void>;
}) {
  const flowRef = useRef<HTMLDivElement>(null);
  const preserveScrollRef = useRef<{ top: number; height: number } | null>(null);
  // 默认滚动到底部（最新消息在末尾）。
  useEffect(() => {
    const flow = flowRef.current;
    if (!flow) return;
    if (preserveScrollRef.current !== null) {
      const previous = preserveScrollRef.current;
      const heightDelta = flow.scrollHeight - previous.height;
      flow.scrollTop = previous.top + Math.max(0, heightDelta);
      preserveScrollRef.current = null;
    } else {
      flow.scrollTop = flow.scrollHeight;
    }
  }, [viewer.messages]);

  if (viewer.status === "loading") {
    return <div className="viewer-state">正在读取消息…</div>;
  }
  if (viewer.status === "error") {
    return <div className="viewer-state">消息暂时无法读取，请稍后重试。</div>;
  }
  if (viewer.messages.length === 0) {
    return <div className="viewer-state">该会话没有可展示的消息。</div>;
  }
  return (
    <div
      className="chat-flow"
      ref={flowRef}
      onScroll={() => {
        const flow = flowRef.current;
        if (!flow || flow.scrollTop > 24 || !viewer.hasMoreBefore || viewer.loadingOlder) return;
        preserveScrollRef.current = { top: flow.scrollTop, height: flow.scrollHeight };
        void onLoadOlder();
      }}
      data-testid="library-viewer-messages"
    >
      {viewer.loadingOlder && (
        <div className="chat-notice" data-testid="library-chat-loading-older">
          正在加载更早消息…
        </div>
      )}
      {viewer.hasMoreBefore && viewer.messageOffset >= CHAT_WINDOW_LIMIT && (
        // 仍有更早消息时提示当前已加载窗口；滚到顶部会继续按页读取。
        <div className="chat-notice" data-testid="library-chat-notice">
          已加载最近 {viewer.messageOffset} 条消息，还可继续查看更早内容。
        </div>
      )}
      {viewer.messages.map((message) => (
        <ChatMessage key={message.message_id} message={message} legs={legs} colorIndices={colorIndices} />
      ))}
    </div>
  );
}

/**
 * 单条消息气泡：用户消息按归属腿着色（右对齐 + 账号色头像 + 账号名），
 * 助手消息左对齐中性色（通用机器人图标，不着色）。
 */
function ChatMessage({
  message,
  legs,
  colorIndices,
}: {
  message: SessionMessageDto;
  legs: readonly RelayLeg[];
  colorIndices: ReadonlyMap<string, number>;
}) {
  if (message.role === "user") {
    // 用户消息归属：created_at 落在哪腿的 [from, to) 区间就归该腿账号。
    const leg = ownerLegOf(message, legs);
    const name = leg.accountName ?? "已移除账号";
    return (
      <div className="chat-msg chat-msg--user" data-testid={`library-message-${message.message_id}`}>
        <div className="chat-msg__side">
          <AccountAvatar name={name} colorIndex={colorIndices.get(leg.userId) ?? 0} />
          <span className="chat-msg__name">{name}</span>
        </div>
        <div className="chat-msg__bubble">{renderMessageContent(message)}</div>
      </div>
    );
  }
  return (
    <div className="chat-msg chat-msg--assistant" data-testid={`library-message-${message.message_id}`}>
      <span className="chat-msg__bot" aria-hidden="true">
        <Bot size={14} strokeWidth={1.8} />
      </span>
      <div className="chat-msg__bubble">{renderMessageContent(message)}</div>
    </div>
  );
}

/** 消息内容渲染：文本正文 / 任务轨迹摘要（沿用旧预览解析口径）。 */
function renderMessageContent(message: SessionMessageDto) {
  if (message.content.kind === "text") {
    return <div className="chat-msg__text">{message.content.text}</div>;
  }
  return (
    <div className="chat-msg__text chat-msg__text--trace">
      任务轨迹 · {message.content.step_count} 步
      {message.content.thoughts.length > 0 && (
        <ul className="chat-msg__thoughts">
          {message.content.thoughts.map((thought, index) => (
            <li key={index}>{thought}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** 接力 tab：该会话的持有时间线（每腿账号 + 时间段 + 新增消息数；当前腿标「当前」）。 */
function RelayTab({
  legs,
  colorIndices,
}: {
  legs: readonly RelayLeg[];
  colorIndices: ReadonlyMap<string, number>;
}) {
  return (
    <div className="viewer-relay" data-testid="library-relay-tab">
      <p className="viewer-relay__hint">这条会话被以下账号先后使用，按时间顺序排列。</p>
      <div className="leg-timeline">
        {legs.map((leg, index) => {
          // 当前腿 = 最后一段（交接至今，进行中）。
          const isCurrentLeg = index === legs.length - 1;
          return (
            <div className="leg-step" key={`${leg.userId}-${index}`}>
              <div className="leg-step__rail">
                <span className="leg-step__dot">
                  <AccountAvatar
                    name={leg.accountName}
                    colorIndex={colorIndices.get(leg.userId) ?? 0}
                  />
                </span>
                <span className="leg-step__line" />
              </div>
              <div className="leg-step__body">
                <div className="leg-step__name">
                  {leg.accountName ?? "已移除账号"}
                  {isCurrentLeg && <span className="leg-step__badge">当前</span>}
                </div>
                <div className="leg-step__meta">
                  {formatLegSpan(leg)} · {leg.messages} 条消息
                </div>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

// ============================================================================
// 删除确认弹窗：列明规模，确认后执行真实删除（后端先备份）
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
              <span>删除前会自动备份</span>
            </div>
          </div>
          <button className="btn" type="button" onClick={onCancel} data-testid="delete-confirm-cancel">
            <X size={15} aria-hidden="true" />取消
          </button>
        </div>
        <div className="preview__body">
          <p className="confirm-dialog__text">
            删除后这些会话将永久移除，无法在应用内恢复（备份文件保留，可人工恢复）。
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
// 合并确认弹窗：选择保留的项目，其余项目会话全部并入（后端先备份）
// ============================================================================

function MergeConfirmDialog({
  projects,
  sessionCounts,
  onCancel,
  onConfirm,
}: {
  /** 待合并项目集合（其中一个被选为保留目标）。 */
  projects: readonly MasterProjectEntryDto[];
  /** 项目 → 会话数（含已归档，不含已删除；弹层规模展示）。 */
  sessionCounts: ReadonlyMap<string, number>;
  onCancel: () => void;
  onConfirm: (target: MasterProjectEntryDto) => void;
}) {
  // 默认保留列表第一个项目；用户可在弹层内改选。
  const [targetId, setTargetId] = useState(projects[0]?.project_id ?? "");
  const target = projects.find((project) => project.project_id === targetId) ?? projects[0];
  // 将被移动的会话总量 = 非保留项目的会话数之和。
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
        aria-label="确认合并项目"
      >
        <div className="preview__head">
          <div className="preview__head-main">
            <div className="preview__title">合并 {projects.length} 个项目</div>
            <div className="preview__meta">
              <span>{movedSessions} 个会话将移入保留的项目（含已归档）</span>
              <span>合并前会自动备份</span>
            </div>
          </div>
          <button className="btn" type="button" onClick={onCancel} data-testid="merge-confirm-cancel">
            <X size={15} aria-hidden="true" />取消
          </button>
        </div>
        <div className="preview__body">
          <p className="confirm-dialog__text">
            选择要保留的项目：其余项目的全部会话都会移入它，移空的项目会被清理。误合并可用备份人工恢复。
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

// ============================================================================
// 工具：账号着色、消息归属、时间格式化
// ============================================================================

/**
 * 账号色板索引（G21）：按腿顺序首次出现排，蓝 → 绿 → 琥珀 循环。
 * 同一会话内同一账号颜色稳定；LLM 回复不参与着色。
 */
function accountColorIndices(legs: readonly RelayLeg[]): Map<string, number> {
  const indices = new Map<string, number>();
  let next = 0;
  for (const leg of legs) {
    if (!indices.has(leg.userId)) {
      indices.set(leg.userId, next % 3);
      next += 1;
    }
  }
  return indices;
}

/**
 * 用户消息归属腿（G21）：chat_message 不带发送者身份，按消息 created_at
 * 落在哪腿的 [from, to) 时间区间推导；无时间戳（0/null）归属首腿。
 */
function ownerLegOf(message: SessionMessageDto, legs: readonly RelayLeg[]): RelayLeg {
  const timestamp = message.created_at_unix_seconds;
  if (!timestamp || timestamp <= 0) return legs[0];
  for (const leg of legs) {
    const afterFrom = leg.fromUnixSeconds === null || timestamp >= leg.fromUnixSeconds;
    const beforeTo = leg.toUnixSeconds === null || timestamp < leg.toUnixSeconds;
    if (afterFrom && beforeTo) return leg;
  }
  // 区间外兜底（异常时间戳）：归最后一腿（进行中）。
  return legs[legs.length - 1];
}

/** 账号色块头像：圆形 + 账号色底 + 白色首字母（消息流与接力时间线共用）。 */
function AccountAvatar({ name, colorIndex }: { name: string | null; colorIndex: number }) {
  const letter = (name ?? "").trim().charAt(0).toUpperCase() || "?";
  return (
    <span className={`account-avatar account-avatar--c${colorIndex}`} aria-hidden="true">
      {letter}
    </span>
  );
}

/** 从台账反查账号显示名（无台账会话的当前账号名兜底用）。 */
function nameOfLedger(ledger: readonly RelayLedgerEntryDto[], userId: string): string | null {
  for (const entry of ledger) {
    if (entry.from_user_id === userId) return entry.from_account_name;
    if (entry.to_user_id === userId) return entry.to_account_name;
  }
  return null;
}

/** 归档视图模式标签（work_mode 列值 → 展示名）。 */
function modeLabel(mode: string): string {
  if (mode === "work") return "Work 模式";
  if (mode === "code") return "Code 模式";
  if (mode === "未标注") return "未标注模式";
  return mode;
}

/** 树内会话相对时间：刚刚 / N 分钟前 / N 小时前 / N 天前，更早回落具体日期。 */
function formatRelativeTime(unixSeconds: number | null): string {
  if (!unixSeconds) return "时间未知";
  const diffSeconds = Date.now() / 1000 - unixSeconds;
  if (diffSeconds < 60) return "刚刚";
  if (diffSeconds < 3600) return `${Math.floor(diffSeconds / 60)} 分钟前`;
  if (diffSeconds < 86_400) return `${Math.floor(diffSeconds / 3600)} 小时前`;
  if (diffSeconds < 7 * 86_400) return `${Math.floor(diffSeconds / 86_400)} 天前`;
  return formatSessionTime(unixSeconds);
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
