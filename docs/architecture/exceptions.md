# 架构例外登记（UI-CHARTER §8）

> 本文件是 `docs/architecture/UI-CHARTER.md` §8 的破例白名单。未在此登记的破例
> 视为违规；`scripts/ui-guard.mjs` 读取本文件作为白名单（见宪章 §7/§8）。
> 每条按宪章 §8 的五个字段登记：文件/规则、原因、范围、退出条件、登记人/日期。

## 现行例外

（暂无）

## 已失效例外

### E-01 页面槽位过渡接缝（P2a → 2026-10-24 撤销，P5-4b）

- 撤销记录：P5 六页（overview / accounts / checkin / environment / library / settings）
  已全部直连合并页，`app/transition/sectionSlots.tsx`（唯一产品整页选择点）与
  `components/SettingsPanel.tsx` 的槽位间接层均已删除——前者整份删除、后者改为
  直渲染 `features/settings/SettingsPage`（productId 透传）。产品整页分支在 `app/`、
  `features/` 内归零，例外失去存在基础，按退出条件从白名单移除。
- 原登记人/日期：P2a（2026-10-22）。
