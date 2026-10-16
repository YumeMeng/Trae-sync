import type { LibraryRef } from "../components/LibrarySessionsPanel";

/**
 * 当前产品展示名（P15-10 单产品塌缩，ADR-0035）：融合版 TRAE 是唯一产品目标，
 * 界面、通知、问候语统一用此名称，零「TRAE Work CN」表述。
 */
export const PRODUCT_DISPLAY_NAME = "TRAE";

/**
 * 工作区区块键（与导航页面一一对应）。master-library 是详情位：
 * 不生成导航项，从环境页（主库卡）或总览统计入口进入。
 */
export type SectionKey =
  | "overview"
  | "accounts"
  | "checkin"
  | "environment"
  | "master-library"
  | "settings";

/**
 * 区块能力三态（ADR-0032 决策 2）：ready = 已适配，渲染真实数据与操作；
 * pending = 适配在途；absent = 协议事实不存在。单产品塌缩后六区块全部 ready，
 * 三态类型与 CapabilitySection 保留给后续区块按事实降级使用。
 */
export type CapabilityState = "ready" | "pending" | "absent";

/** 单产品区块三态：融合版数据与操作均已就绪（P15-6 主库绑定迁移后全 ready）。 */
export const PRODUCT_SECTION_STATES: Record<SectionKey, CapabilityState> = {
  overview: "ready",
  accounts: "ready",
  checkin: "ready",
  environment: "ready",
  "master-library": "ready",
  settings: "ready",
};

/** 查询某区块的能力三态（单产品塌缩后简化为直查）。 */
export function sectionStateOf(section: SectionKey): CapabilityState {
  return PRODUCT_SECTION_STATES[section];
}

/** 只读库实例 id（与后端写守卫同名的库引用；ADR-0032 决策 5；库实例概念保留）。 */
export const TRAE_CN_LIBRARY_ID = "trae_cn";

/** 是否只读库实例：只读库的写操作控件一律隐藏（不是禁用）。 */
export function isReadOnlyLibrary(libraryId: string): boolean {
  return libraryId === TRAE_CN_LIBRARY_ID;
}

/**
 * 主库详情页的库实例（ADR-0032 决策 5）：环境页入口注入的库引用优先，
 * 缺省主库。单产品塌缩后不再按产品分支（融合版主库即官方数据目录）。
 */
export function detailLibraryRefFor(injected: LibraryRef | null): LibraryRef {
  return injected ?? { id: "master" };
}

/** 只读库详情页标题（界面表达纪律：描述对象对用户是什么就叫什么）。 */
export const TRAE_CN_LIBRARY_TITLE = `${PRODUCT_DISPLAY_NAME} 数据根`;
