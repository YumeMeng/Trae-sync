import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Copy, Minus, ShieldCheck, Square, Waypoints, X } from "lucide-react";
import type {
  PlatformContextDto,
  CurrentAccountStateDto,
  CapabilityFlagsDto,
} from "../types/workspace";

interface TitleBarProps {
  platform: PlatformContextDto;
  currentAccount: CurrentAccountStateDto;
  capabilities: CapabilityFlagsDto;
}

// 标题栏 38px 单行（2026-08-27 精简）：品牌 + 全局状态徽章 + 自绘窗控。
// 证据信息（当前账号/重新检测）下沉总览页（overview-evidence）。
// 无系统装饰（decorations: false）时代码即标题栏：拖拽/双击最大化/窗控全在此。
export function TitleBar({ platform, currentAccount, capabilities }: TitleBarProps) {
  const modeLabel = renderModeLabel(currentAccount.unavailable_reason, capabilities.scan_enabled);
  const modeTone = renderModeTone(currentAccount.unavailable_reason, capabilities.scan_enabled);
  const [maximized, setMaximized] = useState(false);

  // 最大化状态跟踪：窗口尺寸变化时重查（浏览器 preview 下 API 不可用则静默保持默认态）。
  // is-maximized 类同步到 <html>：CSS 据此收回呼吸带/圆角/投影（贴边铺满）。
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    // jsdom/浏览器环境无 Tauri internals 时 getCurrentWindow() 会同步抛错，安全包装。
    const safeWindow = () => {
      try {
        return getCurrentWindow();
      } catch {
        return null;
      }
    };
    const apply = (value: boolean) => {
      setMaximized(value);
      document.documentElement.classList.toggle("is-maximized", value);
    };
    const query = () => {
      void safeWindow()
        ?.isMaximized()
        .then(apply)
        .catch(() => undefined);
    };
    query();
    void safeWindow()
      ?.onResized(query)
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => undefined);
    return () => {
      unlisten?.();
      document.documentElement.classList.remove("is-maximized");
    };
  }, []);

  return (
    <header className="title-bar" role="banner" data-tauri-drag-region="">
      <div className="title-bar__brand-block" data-tauri-drag-region="">
        <span className="title-bar__mark" aria-hidden="true">
          <Waypoints size={15} strokeWidth={2.1} />
        </span>
        <span className="title-bar__brand">Trae Sync</span>
        <span className="title-bar__subtitle">{platform.display_name}</span>
      </div>
      {/* 全局唯一状态徽章：正常 / 需操作 / 未授权三态，全应用共享同一语义 */}
      <span
        className={`title-bar__mode status-badge--${modeTone}`}
        data-testid="title-bar-mode"
      >
        <ShieldCheck size={13} strokeWidth={2} aria-hidden="true" />
        <span>{modeLabel}</span>
      </span>

      {/* 自绘窗控（无系统边框）：最小化 / 最大化切换 / 关闭。
          失败静默（浏览器 preview 无法执行窗口命令，真机才生效）。 */}
      <div className="title-bar__window-controls" aria-label="窗口控制">
        <button
          type="button"
          className="title-bar__winctl"
          title="最小化"
          aria-label="最小化窗口"
          data-testid="window-minimize"
          onClick={() => void getCurrentWindow().minimize().catch(() => undefined)}
        >
          <Minus size={14} strokeWidth={2} aria-hidden="true" />
        </button>
        <button
          type="button"
          className="title-bar__winctl"
          title={maximized ? "还原" : "最大化"}
          aria-label={maximized ? "还原窗口" : "最大化窗口"}
          data-testid="window-maximize"
          onClick={() => void getCurrentWindow().toggleMaximize().catch(() => undefined)}
        >
          {maximized ? (
            <Copy size={12} strokeWidth={2} aria-hidden="true" />
          ) : (
            <Square size={11} strokeWidth={2} aria-hidden="true" />
          )}
        </button>
        <button
          type="button"
          className="title-bar__winctl title-bar__winctl--close"
          title="关闭"
          aria-label="关闭窗口"
          data-testid="window-close"
          onClick={() => void getCurrentWindow().close().catch(() => undefined)}
        >
          <X size={15} strokeWidth={2} aria-hidden="true" />
        </button>
      </div>
    </header>
  );
}

/// 全局状态徽章文案：三态统一（只读保护中 / 需要重新检测 / 读取未授权）。
function renderModeLabel(reason: string | null, scanEnabled: boolean): string {
  if (!scanEnabled || reason === "authorization_required" || reason === "authorization_mismatch") {
    return "读取未授权";
  }
  return reason ? "需要重新检测" : "只读保护中";
}

/// 状态徽章色调：safe / warning / neutral，与全应用徽章语义一致。
function renderModeTone(reason: string | null, scanEnabled: boolean): string {
  if (!scanEnabled || reason === "authorization_required" || reason === "authorization_mismatch") {
    return "neutral";
  }
  return reason ? "warning" : "safe";
}
