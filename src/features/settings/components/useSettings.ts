import { useCallback, useLayoutEffect, useRef, useState } from "react";
import type { TraeSettingsSnapshot } from "../traeSettings";
import type { WorkBuddySettingsSnapshot } from "../workbuddySettings";
import type { ConfirmDialogState, LocationDialogState, RestoreDialogState, SettingsActions } from "./settingsTypes";

/**
 * 设置页读取槽与页面数据形状（P4f-5 共用件）：两个产品 hook（useTraeSettings / useWorkBuddySettings）
 * 产出同一个 SettingsPageData，页面据此零产品判断地装配。读取槽取旧 TraeSettingsVariant L65-112 的
 * 代次 + 可见性守卫：晚到的旧快照不覆盖新值，页面失活不发 IPC。
 */
export function useSlot<T>(active: boolean) {
  const [value, setValue] = useState<T | null>(null);
  const version = useRef(0);
  const live = useRef(active);
  useLayoutEffect(() => { live.current = active; }, [active]);
  const load = useCallback((read: () => Promise<T>): Promise<T | null> => {
    if (!live.current) return Promise.resolve(null); // 页面失活：不发起读取（避免后台 IPC）
    const ticket = ++version.current;
    return read().then((snapshot) => {
      if (version.current === ticket && live.current) setValue(snapshot);
      return snapshot;
    });
  }, []);
  return [value, load] as const;
}

/** 页面可渲染的设置数据（两产品同形：快照 + 动作 + 弹层状态 + 忙碌动作 id）。 */
export interface SettingsPageData {
  readonly trae: TraeSettingsSnapshot | null;
  readonly workbuddy: WorkBuddySettingsSnapshot | null;
  readonly actions: SettingsActions;
  readonly location: LocationDialogState | null;
  readonly restore: RestoreDialogState | null;
  readonly confirm: ConfirmDialogState | null;
  readonly busyIds: readonly string[];
}
