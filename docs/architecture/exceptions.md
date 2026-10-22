# 架构例外登记（UI-CHARTER §8）

> 本文件是 `docs/architecture/UI-CHARTER.md` §8 的破例白名单。未在此登记的破例
> 视为违规；`scripts/ui-guard.mjs` 读取本文件作为白名单（见宪章 §7/§8）。
> 每条按宪章 §8 的五个字段登记：文件/规则、原因、范围、退出条件、登记人/日期。

## 现行例外

### E-01 页面槽位过渡接缝（P2a）

- 文件/规则：`src/app/transition/sectionSlots.tsx`、`src/components/SettingsPanel.tsx`、
  `src/components/settings/TraeSettingsVariant.tsx`、`src/components/settings/WorkBuddySettingsVariant.tsx`
  ——违反宪章 §4.1「`app/`、`features/` 中出现 `isWorkBuddy ?`、`productId === 'workbuddy'`、
  `WORKBUDDY_PRODUCT_ID`」（产品整页分支）；以及 §3「组件文件 300 行」在过渡期对
  `TraeSettingsVariant` 的暂时不适用。
- 原因：strangler 接缝。P4 六页迁移尚未完成，六区块此刻仍各有 trae / workbuddy 两套
  独立实现；若不先把"选哪个实现"收敛到一处，P4 每迁一页都要再改一遍 `App.tsx` 的
  内联三目，迁移期会反复产生同样的产品整页分支。本例外把这类判断集中到
  `resolveSection(slot, productId)` 唯一入口，使 `App.tsx` 与 `SettingsPanel` 里
  **零产品整页分支**（目标态的真正要求），产品差异只留在各自页面内部。
- 范围（多久/多大）：仅限上述四个文件；`sectionSlots.tsx` 是唯一允许出现
  `WORKBUDDY_PRODUCT_ID` 与产品二选一的地方（文件头已标
  `// TRANSITION: 仅过渡用，P4 页面迁移完成后删除`）。不新增页面、不改页面内部实现、
  不改样式与测试。例外不覆盖"同一页面两份实现"本身——那是 P4 要消除的对象。
- 退出条件：P4 六页（overview → accounts → checkin → environment → library → settings）
  迁移全部完成。届时 `sectionSlots.tsx` 与其两个设置变体文件整份删除，
  `SettingsPanel.tsx` 收敛为单一实现，本例外随之失效并从本文件移除。
- 登记人/日期：P2a（2026-10-22）。

## 已失效例外

（暂无）
