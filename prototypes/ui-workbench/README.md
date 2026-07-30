# Trae Sync UI Workbench Prototype

四个“同步工作台”布局方案，通过 `?variant=A|B|C|D` 切换：

- A：总览工作台
- B：分步任务流
- C：历史库主导
- D：融合工作台（当前推荐）

D 以 C 的历史库为主界面，吸收 A 的账号概况和同步摘要，并在真正应用前进入 B 的分步安全确认流程。

体验审计与生产扩展边界记录在 `../../docs/UI_PROTOTYPE_REVIEW.md`。原型只验证交互，不作为生产组件结构。

原型只使用内存模拟数据，不读取或写入 TRAE 文件。

运行：

```powershell
pnpm --dir prototypes/ui-workbench dev
```

原型结论确认后，记录保留的交互原则并删除本目录。
