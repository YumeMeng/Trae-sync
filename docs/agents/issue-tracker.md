# Issue Tracker：Local Markdown

本仓库使用本地 Markdown 管理规格、实施任务和依赖关系，不依赖远程平台。

## 实施任务

- `to-tickets` 生成的正式实施计划保存为仓库根目录 `tickets.new.md`（现行唯一计划文件；旧版 `tickets.md` 已于 2026-09-02 删除）。
- 每个 ticket 必须包含交付行为、验收条件和 `Blocked by`。
- 按依赖前沿工作：只有阻塞项全部完成的 ticket 才能开始。
- ticket 完成状态直接在 `tickets.new.md` 中更新，不另建重复任务。

## 调查与临时工作

- 每个主题使用 `.scratch/<feature-slug>/`。
- PRD 保存为 `.scratch/<feature-slug>/PRD.md`。
- 独立调查任务保存为 `.scratch/<feature-slug>/issues/<NN>-<slug>.md`，从 `01` 编号。
- 文件顶部使用 `Status:` 和 `Blocked by:` 记录状态与依赖。
- 评论和补充信息追加在 `## Comments` 下，不覆盖原始结论。

## Skill 发布规则

- 当 skill 要求“发布到 issue tracker”时，按任务类型写入 `tickets.new.md` 或对应 `.scratch/<feature-slug>/`。
- 当 skill 要求“读取 ticket”时，读取用户指定的文件或 `tickets.new.md` 中对应标题。
- 正式实施 ticket 使用 `ready-for-agent` 状态；原始外部请求才进入 triage。

## Wayfinder 约定

- Map：`.scratch/<effort>/map.md`。
- 子任务：`.scratch/<effort>/issues/<NN>-<slug>.md`。
- 子任务使用 `Type:`、`Status:` 和 `Blocked by:`。
- `claimed` 表示已领取，`resolved` 表示已形成可复用决策。
