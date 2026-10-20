# ADR-0037：跨产品概念映射表与扩展强制流程

状态：2026-10-20 采纳（用户拍板，P19 定稿）

## 背景

P17 扩展 WorkBuddy 时发生四次"同概念误判为不同"：①对话库→臆断"云端跟号"（实为本地 SQLite 索引+jsonl 正文，与 Trae 主库同构）②环境→以为 WB 无多实例就不做 ③额度→没逆向就说没有（checkin-status 有 total_credits 全套字段）④添加账号→以为登录只能在客户端（实际 WB=网页 OAuth 授权码流与 Trae 同构）。根因：GLOSSARY 定义绑死 Trae 实现，无跨产品概念映射表，扩展时未经逆向验证就下"不存在"结论。

## 决策

1. **概念映射表**（GLOSSARY.md 新增章节，五列结构）：

   | 锚定概念 | TRAE 实现 | WB 实现 | 扩展判定问题清单 |
   |----------|-----------|---------|------------------|
   | 对话库 | 主库 database.db（SQLCipher）+ 归属随行 | workbuddy.db（SQLite WAL，sessions.user_id 隔离）+ projects jsonl 正文 | 会话存储在哪？本地还是云？账号隔离字段？正文与索引是否分离？ |
   | 环境 | --user-data-dir 隔离目录多实例，对话库跟随目录 | userData（~/.workbuddy/app）与数据根分离，多实例隔离性待实验 | 客户端支持多实例参数吗？数据目录跟随哪个参数？ |
   | 额度 | 模型积分（checkin 概览字段） | total_credits（周期累计积分）+ daily_credit + checkin_dates | 签到/积分接口有哪些字段？逐字段核对过吗？ |
   | 添加账号 | 隔离浏览器 OAuth 编排 | 网页 OAuth 授权码流（CodeBuddy 参数）+ 捕获入池 | 登录是网页流还是仅客户端？协议参数在哪？ |
   | 签到登录态 | begin/complete/cancel_login 会话编排 | 直连（池凭据）无登录会话 | 该平台签到是否需要独立登录会话？ |
   | 主库/副库 | 原生库 vs --user-data-dir 副环境 | 单库（workbuddy.db），多实例实验后再定 | 存在几个库？如何区分原生与创建？ |
   | 收编/过户 | incorporate_master_records | transfer sessions.user_id（B1） | 客户端游离记录如何纳入管理？归属字段是什么？ |
   | 归并 | merge_master_projects（主库内项目合并） | cwd 项目归并（sessions 批量 UPDATE + projects 目录迁移） | 项目维度是什么？如何合并？ |

2. **新产品接入五步强制流程**（AGENTS.md 铁律）：
   ①过映射表：逐概念问判定问题清单，禁止凭印象回答 → ②逆向验证：每个"该平台没有"的结论必须有探针/asar/协议证据，无证据的 absent 结论禁止成立 → ③映射矩阵：Trae 全命令逐项标注（复用/移植/重定义/豁免-待探查）→ ④骨架复用：UI 走 shared/ 组件 + 能力开关（ADR-0036），禁止骨架外产品分支 → ⑤登记：新平台的语义差异回填映射表。

## 后果

- 扩展成本前置（过表+逆向），换取错误成本归零。
- "absent 结论必须有证据"写入验收标准——验收者有权要求探针证据。
