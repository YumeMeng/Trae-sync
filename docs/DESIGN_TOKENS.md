# Trae Sync Design Tokens（毛玻璃设计系统契约）

> 2026-08-26 确立。本文档是 UI 三批改造的唯一视觉与交互契约：实施时的全部色值、字号、间距、圆角、玻璃配方、动效参数以本文为准；每批完成用真实截图对照样张（方案 02「亮白通用玻璃」）验收，偏差控制在 token 级。
> 已确认的设计决策快照：玻璃强度 1A（blur 22px/62% 白）、雾斑 2C（单色靛蓝）、accent 3A（#4F46E5）、圆角 4B（全线 +3~4px）、动效 5（雾斑漂移 + hover 微浮起 + 条件 stagger）、颗粒 6A（3.5% 保留）、导航 7B（52px 图标栏）、密度 8C（舒展档 +15%）。
> 2026-08-27 修订（毛玻璃收敛批）：雾斑改对角贯穿漂移、浓度提至约 20%（决策 #1）；玻璃配方补 `--glass-rail/--glass-dense` 两档；总览快捷操作定为纯导航（决策 #8，见「总览页」章节）。
> 2026-08-28 修订（U-5 token 单轨批）：旧桥接层（--ink/--muted*/--line*/--surface-*/--warning* 等）并入或删除，全 App token 唯一语义层 = `src/styles/tokens.css`；本文档总表与实现同步（--line→--line-hair、--line-strong→--line-control、--warn-soft→--warn-soft-bg，补 --accent-hover/--accent-shadow/--muted-placeholder/--shadow-*/--radius-* 等实际保留项）。
> 2026-08-27 grill 决策（U-4 批，规划见 `.scratch/plan-20260827-checkin-close-ui-base.md`）：历史工作台整页 `--glass-dense`（结构重构另议）；全文件字面色值清剿——辅助 token 入「玻璃配方」段（`--glass-hover/--glass-active/--glass-solid/--fill-soft/--fill/--fill-strong/--text-on-accent/--window-close`），历史页青绿系全部映射中性/靛蓝/琥珀语义，禁再出现字面色值。

## 设计哲学

1. **异常才亮色**：正常状态全部中性安静；看到颜色 = 有事要处理（琥珀 = 需要动手，红 = 必须处理）。无绿色成功 token——正常态用中性/靛蓝表达。
2. **亮白通用玻璃**：亮白画布 + 白玻璃面板 + 极淡单色雾斑；玻璃质感由「面板可糊到底层内容」产生，而非装饰性描边。
3. **主题化架构**：组件层只消费语义 token，禁止出现字面色值；换主题 = 仅重定义 `:root` 变量组，组件零改动（见「主题适配机制」）。

## Token 总表

### 色彩（语义层，组件唯一合法引用）

```css
:root {
  /* 文字三级 */
  --text-1: #1a2233;          /* 主文字：标题、数值、名字 */
  --text-2: #5b6478;          /* 次文字：说明、按钮次要态 */
  --text-3: #9aa3b5;          /* 弱文字：meta、标签、时间戳 */
  --muted-placeholder: #5f6d76;  /* 输入占位专用色（独立保留保 WCAG AA，U-5 归层） */

  /* 强调（唯一品牌色，全 App 性格来源） */
  --accent: #4f46e5;
  --accent-soft: rgba(79, 70, 229, .09);    /* 选中底、软按钮底 */
  --accent-border: rgba(79, 70, 229, .38);
  --accent-shadow: 0 4px 14px rgba(79, 70, 229, .28);  /* 主操作按钮品牌投影 */
  --accent-hover: #4338ca;                            /* 强调色 hover 深一档（U-5 归层保留） */

  /* 异常琥珀（全画面唯一暖色；= 用户需要动手） */
  --warn: #b45309;
  --warn-soft-bg: rgba(217, 119, 6, .09);
  --warn-line: rgba(217, 119, 6, .35);
  --warn-dot: #d97706;

  /* 危险红（删除/不可恢复操作） */
  --danger: #b4233c;
  --danger-soft: rgba(180, 35, 60, .08);
  --danger-line: rgba(180, 35, 60, .32);

  /* 画布与雾斑 */
  --app-bg: #f4f6fa;
  --mist: rgba(99, 102, 241, .2);           /* 单色靛蓝雾斑（与 accent 同族；2026-08-27 决策 #1：浓度约 20%） */

  /* 线条 */
  --line-hair: rgba(26, 34, 51, .08);            /* 玻璃面板描边 */
  --line-control: rgba(26, 34, 51, .14);     /* 控件描边 */
}
```

### 玻璃配方（可整体参数化，深浅主题可异质感）

```css
:root {
  --glass: rgba(255, 255, 255, .62);        /* 标准面板 */
  --glass-strong: rgba(255, 255, 255, .78); /* 标题栏 */
  --glass-rail: rgba(255, 255, 255, .5);    /* 导航栏（轻一档，与标题栏成层级差） */
  --glass-dense: rgba(255, 255, 255, .75);  /* 密集文本区（历史工作台整页，更实保可读） */
  --glass-blur: blur(22px) saturate(135%);
  --glass-blur-strong: blur(18px) saturate(130%);

  /* —— 辅助档（U-4 字面色值清剿批新增；历史页 rgba 近值按归一规则并档） —— */
  --glass-hover: rgba(255, 255, 255, .65);  /* 次操作按钮底 / hover 白玻璃 */
  --glass-active: rgba(255, 255, 255, .85); /* 导航激活浮起底 */
  --glass-solid: rgba(255, 255, 255, .97);  /* 移动端底部栏（高对比底） */
  --fill-soft: rgba(26, 34, 51, .045);      /* 中性极淡底 */
  --fill: rgba(26, 34, 51, .08);            /* 中性淡填充（进度槽等） */
  --fill-strong: rgba(26, 34, 51, .12);     /* 中性 hover 加深 */
  --text-on-accent: #ffffff;                /* 主操作/窗控白字 */
  --window-close: #e81123;                  /* Windows 窗控关闭红（平台约定原值保留） */
  --glass-edge: inset 0 1px 0 rgba(255, 255, 255, .85);   /* 内高光 */
  --glass-shadow: 0 6px 24px rgba(26, 34, 51, .07), 0 1px 3px rgba(26, 34, 51, .05);
}
```

### 投影与尺寸（实现层辅助 token，U-5 自桥接层归档）

```css
:root {
  --shadow-float: 0 12px 32px rgba(20, 32, 42, 0.14);   /* 浮层投影 */
  --shadow-panel: 0 1px 2px rgba(20, 32, 42, 0.06);    /* 面板静态投影 */
  --radius-control: 12px;   /* 控件圆角（2026-08-27 收敛） */
  --radius-panel: 15px;     /* 面板圆角（U-4 对齐舒展档） */
  --control-height: 40px;
  --rail-width: 52px;       /* 图标栏宽度 */
  --space-panel: 20px;      /* 面板主间距（暂无消费者，保留档位） */
}
```

玻璃面板标准组合：`background: var(--glass); backdrop-filter: var(--glass-blur); border: 1px solid var(--line-hair); box-shadow: var(--glass-shadow), var(--glass-edge);`
密集文本区（历史工作台整页）标准组合：同上，`--glass` 替换为 `--glass-dense`（U-4 决策 #1）。

### 字体

```text
字体栈：Segoe UI Variable Text → Segoe UI → Microsoft YaHei（Windows 原生 Fluent 栈）
页面标题   15px / 650 / -0.01em 字距
区块标题   13px / 600
行名字     13px / 600
导航项     12.5px / 500
按钮       11.5px（行内）/ 12.5px（主操作区）/ 500-550
徽章、meta 11px / 450
统计数值   16px / 650 / tabular-nums
统计标签   10.5px
导航分组号 10px / 700 / 0.14em 大写字距
```

### 间距

```text
4 / 8 / 10 / 12 / 14 / 16 / 18 / 20 / 24
行内元素间隙 7px；名字与徽章间隙 7px；行内左右内边距 16px
```

### 圆角（舒展档，全线较样张 +3~4px）

```text
应用窗口 16px · 面板/统计卡 15px · 账号行/卡片 16px
操作按钮 14px · 常规与行内按钮 12px · 分段控件 13px（内项 10px）
徽章 10px · 导航项 12px · 头像正圆
```

### 噪点颗粒

```css
/* 3.5% multiply 颗粒：防白玻璃塑料感；实现为内联 SVG feTurbulence data-URI */
.grain { opacity: .035; mix-blend-mode: multiply; }
```

## 布局骨架

### 应用窗口

- 尺寸基准：内容区最小宽 720px；窗口圆角 16px、外阴影 `0 24px 60px rgba(26,34,51,.16)`。
- 标题栏 38px：`--glass-strong` 玻璃 + 底部 1px `--line-hair`；左端品牌（20px 圆角方块 logo `--accent` 底 + "Trae Sync"），右端窗控。
- **导航栏 52px 图标栏**（7B）：`--glass-rail` 玻璃 + 右侧 1px `--line-hair`；图标 19px 居中，激活项白底浮起（`var(--glass-active)` + 微阴影 + 图标着色 `--accent` + 右缘 5px `--accent` 指示点）；hover 出现右侧玻璃 tooltip（淡入 100ms、延迟 300ms、`--glass-strong` 底）；分组标题（工作台/系统）在图标栏模式下隐藏，靠 tooltip 提供文字。
- 主内容区内边距 20px 24px（舒展档）。
- 底层：单团靛蓝雾斑（`--mist`，直径 1300px、blur(90px)，对角贯穿漂移：右上 → 左下，36s alternate）+ 3.5% 颗粒层。位置须覆盖导航与内容主体，保证玻璃全程有内容可糊。

### 账号行（列表视图，舒展档密度）

```text
行内边距 13px 16px · 行间隙 10px · 行圆角 16px · 玻璃面板标准组合
结构（左→右）：头像 32px 圆（渐变底 + 2px 白描边）→ 主列（名字行 + meta 行）→ 右列（积分块 + 按钮组）
名字行：名字（13/600/--text-1）+ 槽位1徽章 + 槽位2徽章，间隙 7px，永不换行省略
meta 行：11px --text-3 ·「脱敏手机号 · 令牌 N 天 · 设备 …XXXX」
积分块：数值 13/600 tabular + 标签 10px，右对齐，最小宽 62px
按钮组：行内按钮 12px 圆角
```

卡片视图：同结构纵向堆叠，信息三级层级（身份头部 / 主数据行 / meta 底行 + 操作），宽度为网格自适应。

## 徽章系统（两槽位统一，全局同义同色）

**槽位 1 · 签到**（固定出现，布局锚点）：

| 状态 | 形态 | 文案 |
|---|---|---|
| 已签 | 中性底 + 靛蓝点（55% 透明度） | `已签` |
| 未签 | 中性底 + 空心点 | `未签` |
| 未知 | 中性底 + 灰点 | `状态未知` |

**槽位 2 · 实例**（复合态）：

| 状态 | 形态 | 文案 |
|---|---|---|
| 未启动 | 中性底 + 空心点 | `未启动` |
| 未启动·登录失效 | **琥珀加强**（持续性异常：下次启动必需重新登录） | `未启动 · 登录失效` |
| 运行中 | 中性底 + 靛蓝实点 | `运行中` |
| 运行中·已登录 | 中性底 + 靛蓝实点 | `运行中 · 已登录` |
| 运行中·待登录 | **琥珀**（--warn 系）+ 实点 | `运行中 · 待登录` |
| 运行中·登录失效 | **琥珀加强**（--warn 全饱和边框） | `运行中 · 登录失效` |

**降级为文字**（不再是徽章）：令牌健康（`令牌 N 天`，<7 天整段 meta 文字变琥珀）、设备尾号。正常态徽章一律中性；颜色即异常。

徽章基础样式：11px / 内边距 2px 9px / 圆角 10px / 中性底 `rgba(26,34,51,.045)` + `--line-control` 边 / 圆点 5px。

## 组件规格

### 按钮

| 变体 | 规格 |
|---|---|
| 主操作（一键全签等） | 实色 `--accent` 底白字 / 圆角 14px / 投影 `0 4px 14px rgba(79,70,229,.28)` + 内高光 |
| 次操作 | 玻璃底 `rgba(255,255,255,.65)` + `--line-control` 边 / `--text-1` 字 |
| 软强调（行内「启动」） | `--accent-soft` 底 + `--accent-border` 边 + `--accent` 字 |
| 危险 | `--danger` 系，仅出现在独立危险区 |
| 禁用 | opacity .35，无投影 |

### 统计卡

玻璃面板标准组合 / 圆角 15px / 内边距 13px 16px / 数值 16px 650 tabular（单位小字 `--text-3`）/ 标签 10.5px `--text-3` / 卡间隙 12px。

### 分段控件（列表/卡片切换）

容器：玻璃底 + `--line-hair` 边 / 圆角 13px / 内项 10px 圆角；激活项白底 + 微投影；选择记忆 localStorage（`accounts.view`）。

### 签到页（四动作直达）

- 顶部动作区：`一键全签（N）` 主操作 + `一键补签（M）` 次操作常驻；勾选任意账号后追加出现 `签到所选（K）`。
- 单列表演进行：同一行随流程变状态（勾选态 → 排队 → 执行中高亮 → 结果态），顶部总进度条（X/Y + 玻璃轨道），行内单账号签到按钮仅未签可点。
- 结果态文案沿用业务码透传规则（9074/9095/20324/remint_failed 独立文案）。

### 总览页（摘要行 + 快捷操作）

四统计卡（账号数 / 今日签到 X·Y / 运行中实例 / 模型积分合计）+ 快捷操作行（**纯导航**：去签到 / 管理账号，只跳转不直接执行——2026-08-27 决策 #8，主动偏离早期"一键全签/添加账号"直接执行设想）+ 下方保留现有工作台状态区；区块化可插拔（后续主库模式状态卡等）。

### 账号详情

基础信息 / 登录健康度 / 自动签到分区不变；操作区改两组：常规组（刷新额度、立即签到、重新登录、重铸设备）+ 危险组（删除账号，`--danger` 系，独立分隔线下沉页面底部）。备注名行内编辑（输入框 + 保存）。

### 输入框

玻璃底 + `--line-control` 边 / 圆角 12px / 高 36px / 聚焦 `--accent-border` 边 + `--accent-soft` 外晕 2px。

## 动效规范

```css
/* hover 微浮起：行、卡片、按钮通用 */
.lift:hover:not(:disabled) { transform: translateY(-1px); transition: 120ms ease; }
/* 列表 stagger：仅首次挂载，40ms 间隔，最多前 10 项；刷新不重放 */
.stagger-in { animation: rise 220ms ease both; }  /* translateY(4px)→0 + 淡入 */
/* 雾斑漂移 36s alternate；全部动效包 @media (prefers-reduced-motion: no-preference) */
```

## 主题适配机制

组件层**只允许**引用上文语义 token（含玻璃配方 token）。换主题仅新增一组覆盖：

```css
/* 示例：暗色变体（未实施，仅示范机制） */
:root[data-theme="dark"] {
  --app-bg: #070b14;
  --text-1: #e8ecf4;  --text-2: #8b93a7;  --text-3: #5a6275;
  --glass: rgba(15, 21, 38, .52);  --glass-edge: inset 0 1px 0 rgba(255,255,255,.07);
  --line-hair: rgba(255, 255, 255, .09);  --line-control: rgba(255, 255, 255, .14);
  --accent-soft: rgba(129, 140, 248, .24);
  /* --accent/--warn/--danger 色相不变，按需调亮度 */
}
```

## 实施批次与验收

| 批次 | 范围 | 验收 |
|---|---|---|
| ① 数据层 | 注册表备注名/脱敏手机号字段、登录采集、无感补采、总览 DTO、备注名命令与详情编辑 | cargo test 全绿 + typecheck + 前端测试 + 真实环境刷新额度后手机号自动补全 |
| ② 账号页 | 双视图切换、两槽位徽章系统、行/卡片新布局、详情危险分区、排序 | 截图对照样张（token 级偏差检查）+ E2E |
| ③ 签到页 + 总览 | 单列表演进式 + 四动作、总览摘要行改版、动效（hover/stagger/雾斑） | 截图对照 + E2E + 手动体验走查 |

验收基线样张：方案 02「亮白通用玻璃」整窗渲染（2026-08-26 会话内确认稿）。
