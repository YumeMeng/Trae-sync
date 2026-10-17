import { invoke } from "@tauri-apps/api/core";
import type { LibraryRef } from "../components/LibrarySessionsPanel";
import type { CurrentProductDto, ProductEntryDto } from "../types/product";

/**
 * 当前产品展示名（单产品语境）：TRAE 账号域文案统一用此名称。
 * P17-T5 并列骨架恢复产品切换后，该常量只服务 TRAE 产品的文案拼装
 * （如「TRAE：今日自动签到完成」通知前缀）；产品名随切换变化处一律
 * 用 CurrentProductDto.display_name。
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

/** 查询某区块的能力三态（TRAE 语境；跨产品语境用 sectionStatesFor）。 */
export function sectionStateOf(section: SectionKey): CapabilityState {
  return PRODUCT_SECTION_STATES[section];
}

/**
 * WorkBuddy 的区块三态（P17-T5 并列骨架一期）：账号（健康/捕获/账号池）
 * 与签到（直连协议）已就绪；总览统计、环境、主库随二期对话库适配推进，
 * 先以 pending 说明卡诚实呈现。settings 是应用级设置（备份保留策略等），
 * 与产品无关，保持 ready。
 */
const WORKBUDDY_SECTION_STATES: Record<SectionKey, CapabilityState> = {
  overview: "pending",
  accounts: "ready",
  checkin: "ready",
  environment: "pending",
  "master-library": "pending",
  settings: "ready",
};

/** 按产品查询区块三态（未注册产品 fail-closed 回退 pending）。 */
export function sectionStatesFor(productId: string): Record<SectionKey, CapabilityState> {
  switch (productId) {
    case "workbuddy":
      return WORKBUDDY_SECTION_STATES;
    case "trae_cn":
      return PRODUCT_SECTION_STATES;
    default:
      return {
        overview: "pending",
        accounts: "pending",
        checkin: "pending",
        environment: "pending",
        "master-library": "pending",
        settings: "ready",
      };
  }
}

// ===== 产品上下文（P17-T5 并列骨架）：切换器与产品分支渲染的数据源 =====

/** 产品注册表清单（后端 descriptor 静态声明）。 */
export function loadProductEntries(): Promise<readonly ProductEntryDto[]> {
  return invoke<readonly ProductEntryDto[]>("list_products");
}

/** 当前产品指针（后端 product-state 文件；未初始化缺省 trae_cn）。 */
export function loadCurrentProduct(): Promise<CurrentProductDto> {
  return invoke<CurrentProductDto>("get_current_product");
}

/** 切换当前产品（只写指针，不动任何凭据/账号数据；注册表校验 fail-closed）。 */
export async function setCurrentProduct(productId: string): Promise<void> {
  await invoke("set_current_product", { productId });
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
