# ADR-0036：跨产品 UI 一致性架构（骨架模板 × 产品数据）

状态：2026-10-20 采纳（用户拍板，P19 定稿）

## 背景

P17 扩展 WorkBuddy 时前端"另起炉灶"（WorkBuddyPanel 独立实现 601 行），截图走查证实与 TRAE 同页面差距巨大（总览/库为占位页、账号/签到缺批量与逐行形态）。用户判定：多平台管理工具（目标十几个平台）的前端 UI 必须架构级一致，任何不一致乘以平台数都是学习成本。

## 决策

三原则（铁律）：
1. **信息架构统一**：导航、页面构成、卡片/列表/详情组织 = 全产品同一套模板（六页：总览/账号/签到/库/环境/设置）。区块规格见 docs/specs/p19-information-architecture-draft.md §1。
2. **组件共享**：账号行、徽章、统计卡、空态、过滤器、视图切换全部是产品无关组件（src/components/shared/），产品层只喂数据。骨架已存在一半（AccountCenter 的 SharedAccountEntry L1332），补齐页编排骨架层。
3. **能力开关**：产品差异只体现在"有没有这个数据/功能"（前端静态 productUiRegistry 与后端 productRegistry 的 sectionStatesFor 并表），绝不体现在布局与交互。所有区块走 ADR-0032 三态渲染，禁止 productId 整页分支。

## 迁移路径（五步，避免大爆炸）

①纯移动（现有共享组件入 shared/）→ ②控件抽取 → ③页编排骨架（AccountPageSkeleton）→ ④签到骨架 → ⑤WB 消费迁移。目标结构 src/components/shared/{account,checkin}/ + product/{trae,workbuddy}/。UI 能力声明 = 前端静态 productUiRegistry。

## 后果

- 新平台扩展 = 填数据 + 声明能力开关，不写布局。
- PR 门禁：禁止骨架外的产品专属分支渲染（AGENTS.md 执行）。
- localStorage 偏好分产品 key（接受一次性重置）。
