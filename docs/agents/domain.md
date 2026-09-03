# Domain Docs

本仓库使用 single-context 领域文档布局。

## 读取顺序

开始分析、设计或实现前读取：

1. 根目录 `CONTEXT.md`。
2. `docs/DECISIONS-20260822-GRILL.md`（项目方向记录；**注意头部取代标注**——部分决策已被后续 ADR 取代）。
3. `docs/adr/` 现行决策（0018-0024，见 `docs/adr/README.md` 索引；新 ADR 由任务驱动创建）。
4. `tickets.new.md` 中对应 Phase 的 ticket（现行唯一计划文件）。
5. 相关技术证据文档（`docs/TECHNICAL_BASELINE.md`、`.scratch/checkin-http/`）。

文件尚不存在时继续工作，不为填充目录而创建无实际决策的文档。

## 布局

```text
/
├── CONTEXT.md
├── tickets.new.md
├── docs/
│   ├── adr/
│   ├── agents/
│   └── TECHNICAL_BASELINE.md
└── src/
```

## 术语规则

- 输出、ticket、测试和代码使用 `CONTEXT.md` 定义的规范术语。
- 不用近义词替换已经定义的领域概念。
- 新概念无法用现有术语准确表达时，先记录术语缺口，再决定是否运行 `domain-modeling`。

## ADR 规则

- 实现若与现有 ADR 冲突，必须明确指出冲突，不得静默覆盖。
- 难以撤销、影响多个模块或改变用户承诺的决定应写入 ADR。
- 仅记录真实决定，不为推测性扩展预建 ADR。
