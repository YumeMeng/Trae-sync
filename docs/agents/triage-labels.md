# Triage Labels

本地 tracker 使用以下默认角色字符串。

| Skill 角色 | Tracker 标签 | 含义 |
| --- | --- | --- |
| `needs-triage` | `needs-triage` | 等待维护者评估 |
| `needs-info` | `needs-info` | 等待报告者补充信息 |
| `ready-for-agent` | `ready-for-agent` | 规格完整，可由 Agent 独立执行 |
| `ready-for-human` | `ready-for-human` | 需要人工执行或决策 |
| `wontfix` | `wontfix` | 明确不处理 |

Skill 提到角色时，使用表中对应字符串。正式 `to-tickets` 输出无需再次 triage，直接使用 `ready-for-agent`。
