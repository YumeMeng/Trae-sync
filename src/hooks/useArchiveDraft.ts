import { useCallback, useEffect, useRef, useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ArchiveDraftDto } from "../types/history";
// 统一通知适配层（P11-1 切片②）：迁移/回退提示经 notify 发出。
import { notify } from "../platform/notify";

/**
 * useArchiveDraft——归档草稿持久化 hook（P12-R2c，自 LibrarySessionsPanel 收敛）。
 *
 * 迁移语义内聚（P11-5）：读后端草稿（storage_root/archive-drafts.json，
 * 损坏 fail-closed 按空草稿处理）→ 后端为空且 localStorage 有旧草稿时一次性
 * 迁移（写入后端成功才删旧键，失败保留旧键并提示，本会话照常使用旧草稿）→
 * 草稿变化时写回后端，写回失败回退 localStorage 并提示，回退也失败才升级
 * 页内横幅（storageError）。draftLoadedForRef 代次守卫阻止加载完成前跨库误写。
 */
/** 归档草稿：待归档与待恢复的会话 id 集合（两集合互斥）。 */
export interface ArchiveDraft {
  readonly archiveSessionIds: readonly string[];
  readonly restoreSessionIds: readonly string[];
}

/** 归档草稿存储键前缀；按库隔离（P11-5 起仅作迁移源，草稿正本在工具管理数据）。 */
const ARCHIVE_DRAFT_STORAGE_PREFIX = "trae-sync:archive-draft:";

/** 空草稿常量（空草稿在后端表现为删除该库键）。 */
export const EMPTY_ARCHIVE_DRAFT: ArchiveDraft = {
  archiveSessionIds: [],
  restoreSessionIds: [],
};

function archiveDraftStorageKey(libraryId: string): string {
  return `${ARCHIVE_DRAFT_STORAGE_PREFIX}${encodeURIComponent(libraryId)}`;
}

/** 草稿是否有未提交内容（空草稿在后端表现为删除该库键）。 */
function draftHasChanges(draft: ArchiveDraft): boolean {
  return draft.archiveSessionIds.length > 0 || draft.restoreSessionIds.length > 0;
}

/**
 * 前端草稿状态 → 后端命令参数。请求方向 Tauri 自动做 camelCase→snake_case
 * 参数转换（与 apply_master_archive_changes 同惯例），因此传 camelCase；
 * 响应方向是 serde 序列化，保持 snake_case（见 dtoToDraft）。
 */
function draftToDto(draft: ArchiveDraft): {
  archiveSessionIds: string[];
  restoreSessionIds: string[];
} {
  return {
    archiveSessionIds: [...draft.archiveSessionIds],
    restoreSessionIds: [...draft.restoreSessionIds],
  };
}

/** 后端草稿返回 → 前端草稿状态。 */
function dtoToDraft(dto: ArchiveDraftDto): ArchiveDraft {
  return {
    archiveSessionIds: [...dto.archive_session_ids],
    restoreSessionIds: [...dto.restore_session_ids],
  };
}

/** 读取 localStorage 旧草稿（P11-5 迁移源）；坏数据按空草稿处理，不阻断列表。 */
function readLegacyDraft(libraryId: string): ArchiveDraft {
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

export interface UseArchiveDraftOptions {
  /** 页面可见时才发起草稿 IPC；隐藏时挂起，active 后补读。 */
  active: boolean;
  /** 只读库不参与草稿机制（P11-6）：不读、不写、不迁移。 */
  readOnly: boolean;
}

export interface UseArchiveDraftResult {
  /** 当前草稿（待后端加载完成前为空草稿）。 */
  draft: ArchiveDraft;
  /** 草稿更新函数（函数式更新可用）；变化后自动写回持久化。 */
  setDraft: Dispatch<SetStateAction<ArchiveDraft>>;
  /** 草稿是否有未提交内容。 */
  hasChanges: boolean;
  /** 草稿持久化全部失败（后端 + localStorage 回退都不可用）时的页内横幅文案。 */
  storageError: string | null;
  /** 放弃草稿：清空为空草稿（写回路径随后清理持久化内容）。 */
  reset: () => void;
}

/**
 * 归档草稿状态机。与任务书签名 `useArchiveDraft(libraryId, readOnly)` 的差异：
 * 增加 `active` 选项（页面不可见时不发起草稿 IPC，是 LibrarySessionsPanel 的
 * 既有行为，必须保留），readOnly 归并入同一 options 对象。
 */
export function useArchiveDraft(
  libraryId: string,
  { active, readOnly }: UseArchiveDraftOptions,
): UseArchiveDraftResult {
  const [draft, setDraft] = useState<ArchiveDraft>(EMPTY_ARCHIVE_DRAFT);
  // 草稿持久化全部失败（后端 + localStorage 回退都不可用）时的显式提示。
  const [storageError, setStorageError] = useState<string | null>(null);
  // 已完成后端草稿加载的库 id：加载未完成前禁止写回，防止空/旧草稿
  // 覆盖其他库或上一会话的持久化内容（跨库写保护）。
  const draftLoadedForRef = useRef<string | null>(null);

  const hasChanges = draftHasChanges(draft);

  // 切换库实例时切换对应草稿：读后端（损坏 fail-closed 返回空草稿）；
  // 后端为空且 localStorage 有旧草稿时迁移（写入成功才清除旧键，失败保留
  // 旧键并提示，本会话仍可用旧草稿）。页面不可见时不发起 IPC，active 后补读。
  useEffect(() => {
    if (!active || readOnly) return;
    let cancelled = false;
    // 加载未完成前先复位：草稿清空 + 关闭写回（ref 校验）。
    draftLoadedForRef.current = null;
    setDraft(EMPTY_ARCHIVE_DRAFT);
    setStorageError(null);
    void (async () => {
      let loaded: ArchiveDraft = EMPTY_ARCHIVE_DRAFT;
      try {
        loaded = dtoToDraft(await invoke<ArchiveDraftDto>("get_archive_draft", { libraryId }));
      } catch {
        // 命令不可用按空草稿处理；后续写入失败仍会回退 localStorage。
      }
      if (cancelled) return;
      if (draftHasChanges(loaded)) {
        draftLoadedForRef.current = libraryId;
        setDraft(loaded);
        return;
      }
      // 后端无草稿：尝试一次性迁移 localStorage 旧草稿（老版本升级路径）。
      const legacy = readLegacyDraft(libraryId);
      if (draftHasChanges(legacy)) {
        try {
          await invoke("set_archive_draft", { libraryId, ...draftToDto(legacy) });
          try {
            window.localStorage.removeItem(archiveDraftStorageKey(libraryId));
          } catch {
            // 清旧键失败无碍：正本已写入后端，localStorage 仅为迁移源。
          }
          if (!cancelled) setDraft(legacy);
        } catch {
          // 迁移失败：草稿保留在 localStorage，本会话照常使用，显式提示。
          if (!cancelled) {
            setDraft(legacy);
            notify.warning("归档草稿迁移到新存储失败，本次暂由本机浏览器存储保存，未丢失。");
          }
        }
      }
      if (!cancelled) draftLoadedForRef.current = libraryId;
    })();
    return () => {
      cancelled = true;
    };
  }, [active, readOnly, libraryId]);

  // 草稿变化时写回工具管理数据；后端写入失败回退 localStorage 并显式提示
  // （不静默丢草稿）。仅对「已完成后端加载」的库生效，防止跨库误写。
  // 只读库不写草稿（P11-6）。
  useEffect(() => {
    if (readOnly || draftLoadedForRef.current !== libraryId) return;
    let cancelled = false;
    void (async () => {
      try {
        await invoke("set_archive_draft", { libraryId, ...draftToDto(draft) });
        // 后端写入成功：localStorage 至多是迁移残留，统一清理。
        try {
          window.localStorage.removeItem(archiveDraftStorageKey(libraryId));
        } catch {
          // 清理失败无碍正本；不提示。
        }
        if (!cancelled) setStorageError(null);
      } catch {
        // 回退 localStorage：空草稿清旧键，有内容则暂存。
        try {
          const key = archiveDraftStorageKey(libraryId);
          if (!hasChanges) window.localStorage.removeItem(key);
          else window.localStorage.setItem(key, JSON.stringify(draft));
          if (!cancelled) {
            setStorageError(null);
            notify.warning(
              hasChanges
                ? "归档草稿暂存到工具数据失败，已改存本机浏览器存储，重启应用后仍可恢复。"
                : "归档草稿清理未能写入工具数据，重启应用后可能再次出现该草稿。",
            );
          }
        } catch {
          // 双重失败才升级为页内横幅：当前状态保留，但不能静默。
          if (!cancelled) {
            setStorageError(
              hasChanges
                ? "归档设置暂存失败，关闭应用可能丢失未提交草稿。请先恢复本地存储后再离开。"
                : "未能清理本地归档草稿，重启后可能再次出现。",
            );
          }
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [draft, hasChanges, readOnly, libraryId]);

  const reset = useCallback(() => setDraft(EMPTY_ARCHIVE_DRAFT), []);

  return { draft, setDraft, hasChanges, storageError, reset };
}
