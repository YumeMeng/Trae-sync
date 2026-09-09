import { CalendarCheck, Layers, Settings2, UserRound } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { Home } from "lucide-react";

// master-library 为主库详情页（P5-8a-2）：不在导航栏（从环境页主库卡进入，
// 页内「返回」回环境页），故 navigationItems 不含它。
export type AppPage =
  | "overview"
  | "accounts"
  | "checkin"
  | "environment"
  | "master-library"
  | "settings";

interface NavigationRailProps {
  activePage: AppPage;
  onPageChange: (page: AppPage) => void;
}

// 导航只负责切换页面；状态徽章统一收敛到标题栏，不再重复展示能力状态。
const navigationItems: Array<{
  page: AppPage;
  label: string;
  icon: LucideIcon;
}> = [
  { page: "overview", label: "总览", icon: Home },
  { page: "accounts", label: "账号", icon: UserRound },
  { page: "checkin", label: "签到", icon: CalendarCheck },
  { page: "environment", label: "环境", icon: Layers },
  { page: "settings", label: "设置", icon: Settings2 },
];

// 持久导航只切换产品工作区，不承载任何数据写入动作。
// 52px 图标栏：文字不占栏内空间，hover 由 CSS 玻璃 tooltip（data-label）呈现。
export function NavigationRail({ activePage, onPageChange }: NavigationRailProps) {
  return (
    <aside className="navigation-rail" aria-label="主导航">
      <nav className="navigation-rail__nav">
        {navigationItems.map((item) => {
          const active = activePage === item.page;
          return (
            <button
              key={item.page}
              type="button"
              className={`navigation-rail__item${active ? " navigation-rail__item--active" : ""}`}
              aria-current={active ? "page" : undefined}
              data-label={item.label}
              data-testid={`navigation-${item.page}`}
              onClick={() => onPageChange(item.page)}
            >
              <span className="navigation-rail__icon" aria-hidden="true">
                <item.icon size={20} strokeWidth={1.8} />
              </span>
              {/* 可访问名来源：视觉隐藏但读屏可读（不用 aria-label，避免与页面 region 同名撞 selector） */}
              <span className="navigation-rail__label">{item.label}</span>
            </button>
          );
        })}
      </nav>
    </aside>
  );
}
