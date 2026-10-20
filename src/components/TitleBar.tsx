import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Copy, Minus, Square, Waypoints, X } from "lucide-react";
import type { PlatformContextDto } from "../types/workspace";
import type { CurrentProductDto, ProductEntryDto } from "../types/product";
import { ProductBadge } from "./shared/ProductBadge";

interface TitleBarProps {
  platform: PlatformContextDto;
  /** 产品注册表清单（切换器选项；空清单不渲染切换器）。 */
  products: readonly ProductEntryDto[];
  /** 当前产品指针（切换器选中值）。 */
  currentProduct: CurrentProductDto | null;
  /** 切换产品回调（App 负责写指针 + 刷新分支渲染）。 */
  onSwitchProduct: (productId: string) => void;
}

// 标题栏 38px 单行（2026-08-27 精简，2026-09-03 移除三态徽章）：品牌 + 自绘窗控。
// 证据信息（当前账号/重新检测）下沉总览页（overview-evidence）。
// P17-T5：恢复产品切换器（并列骨架一期）——产品是空间级操作，原生 select
// 紧贴品牌；子标题仍展示当前产品平台名。无系统装饰（decorations: false）
// 时代码即标题栏：拖拽/双击最大化/窗控全在此。
export function TitleBar({ platform, products, currentProduct, onSwitchProduct }: TitleBarProps) {
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
      {/* 产品切换器（P17-T5 并列骨架）：brand-block 之外的 header 直接子元素
          （brand-block 子元素 pointer-events:none 拖拽穿透，切换器需可交互）。
          原生 select 保证键盘/读屏可用；空清单（注册表读取失败）不渲染。 */}
      {products.length > 0 && currentProduct && (
        <div className="title-bar__product-switcher">
          {/* P17-T5 二期/A6：当前产品徽标（形态优化主标识）；原生 select
              保留键盘/读屏可用性。 */}
          <ProductBadge productId={currentProduct.product_id}>
            {currentProduct.display_name}
          </ProductBadge>
          <select
            aria-label="当前产品"
            data-testid="product-switcher"
            value={currentProduct.product_id}
            onChange={(event) => onSwitchProduct(event.target.value)}
          >
            {products.map((product) => (
              <option key={product.product_id} value={product.product_id}>
                {product.display_name}
              </option>
            ))}
          </select>
        </div>
      )}
      {/* 自绘窗控（无系统边框）：最小化 / 最大化切换 / 关闭。
          三个按钮直接平铺在标题栏 flex 行内（无包裹容器）：首钮 margin-left:auto 推到右缘，
          末钮贴壳层圆角；按钮 align-self:stretch 拉满标题栏 38px 高（Windows 窗控惯例）。
          失败静默（浏览器 preview 无法执行窗口命令，真机才生效）。 */}
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
    </header>
  );
}
