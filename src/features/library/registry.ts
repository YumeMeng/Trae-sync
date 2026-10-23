/**
 * 库页 builder 登记表（P4e-4a / P4e-4b）：产品 id → builder 的查表分发，零 if/else 产品链。
 * 未登记产品 fail-closed 到空视图（不猜能力、不套用任一产品的行语义）。
 * 新增产品只在 BUILDERS 加一行 + 在 LibrarySnapshot 联合加一支，不写 === 分支。
 */
import { LIBRARY_TEXTS, LIBRARY_VIEW_TEXTS, type LibraryView } from "./config";
import { buildTraeLibraryView, type TraeLibrarySnapshot } from "./traeLibrary";
import { buildWorkBuddyLibraryView, type WorkBuddyLibrarySnapshot } from "./workbuddyLibrary";

/** 页面读取层产出的产品快照（判别字段 productId 与载荷字段同名同值）。 */
export type LibrarySnapshot = { productId: "trae_cn"; trae: TraeLibrarySnapshot } | { productId: "workbuddy"; workbuddy: WorkBuddyLibrarySnapshot };
/** 按载荷键取快照里的产品数据（表内键与载荷键一一对应，故直接断言）。 */
const payloadOf = <T,>(snapshot: LibrarySnapshot, key: string): T => (snapshot as unknown as Record<string, unknown>)[key] as T;
/** 查表分发：键 = productId，值 = 该产品的 builder（新增产品只在此加一行）。 */
const BUILDERS: Record<string, (snapshot: LibrarySnapshot) => LibraryView> = {
  trae_cn: (snapshot) => buildTraeLibraryView(payloadOf<TraeLibrarySnapshot>(snapshot, "trae")),
  workbuddy: (snapshot) => buildWorkBuddyLibraryView(payloadOf<WorkBuddyLibrarySnapshot>(snapshot, "workbuddy")),
};

/** fail-closed 空视图：无树、无查看器、无动作、无 tab；文案不冒充任一产品（同 accounts/registry 口径）。 */
const EMPTY_VIEW: LibraryView = {
  productId: "", libraryId: "",
  header: { title: "库信息暂不可用", regionLabel: LIBRARY_TEXTS.detailRegionMaster, pageTitleAttr: "master-library",
    readonlyBadge: null, badgeProductId: null, back: null, refresh: null },
  tabs: [], loadingNote: null, error: null,
  empty: { title: "库信息暂不可用，请稍后重试。", hint: "", actionLabel: null },
  stats: null, filter: null, includeDeleted: null, search: null,
  tree: [], treeEmpty: "库信息暂不可用，请稍后重试。", viewer: null,
  batchBar: null, selectionEntry: null, draftBar: null, archive: null, checkup: null, plugins: null, info: null,
  notices: [], texts: LIBRARY_VIEW_TEXTS.trae_cn,
};

/** 快照 → 视图模型：未知产品一律空视图（不抛错，页面按空态渲染）。 */
export function libraryViewFor(snapshot: LibrarySnapshot): LibraryView {
  return BUILDERS[snapshot.productId]?.(snapshot) ?? EMPTY_VIEW;
}
