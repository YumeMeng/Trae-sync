import {
  AlertTriangle,
  Archive,
  ArrowLeft,
  ArrowRight,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Clock3,
  Database,
  FolderGit2,
  HardDrive,
  History,
  LayoutDashboard,
  Library,
  ListFilter,
  Loader2,
  LockKeyhole,
  MessageSquare,
  MoreHorizontal,
  Play,
  RefreshCw,
  ScanSearch,
  Search,
  Settings,
  ShieldCheck,
  SlidersHorizontal,
  UserRound,
  X,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";

type VariantKey = "A" | "B" | "C" | "D";
type ViewKey = "sync" | "history" | "operations" | "settings";
type ScopeMode = "all" | "custom";
type Tone = "success" | "warning" | "danger" | "neutral" | "info";

type Session = {
  id: string;
  title: string;
  updated: string;
  messages: number;
};

type Project = {
  id: string;
  title: string;
  source: string;
  targetState: "same-project" | "missing-project" | "already-current";
  sessions: Session[];
};

type PlanRow = {
  id: string;
  title: string;
  detail: string;
  count: number;
  tone: Tone;
  kind: "attach" | "follow" | "current" | "excluded";
};

type VariantProps = {
  view: ViewKey;
  setView: (view: ViewKey) => void;
  scopeMode: ScopeMode;
  setScopeMode: (mode: ScopeMode) => void;
  selectedIds: Set<string>;
  toggleSession: (id: string) => void;
  selectWholeDreamBot: () => void;
  planRows: PlanRow[];
  selectedCount: number;
  partialDreamBot: boolean;
  scanning: boolean;
  scanProgress: number;
  historyRevision: number;
  onScan: () => void;
  onApply: () => void;
  autoScan: boolean;
  setAutoScan: (value: boolean) => void;
  reopenTrae: boolean;
  setReopenTrae: (value: boolean) => void;
  lastOperationDone: boolean;
};

const CURRENT_ACCOUNT = "1804778984702451";
const SOURCE_ACCOUNT = "2578820706078841";

const projects: Project[] = [
  {
    id: "sub2api",
    title: "Sub2API",
    source: SOURCE_ACCOUNT,
    targetState: "same-project",
    sessions: [
      { id: "sub-1", title: "上游版本接入与回滚验证", updated: "今天 20:18", messages: 86 },
      { id: "sub-2", title: "单飞请求并发问题分析", updated: "昨天 23:42", messages: 41 },
      { id: "sub-3", title: "前端版本标识设计", updated: "7 月 28 日", messages: 29 },
    ],
  },
  {
    id: "dreambot",
    title: "DreamBot",
    source: SOURCE_ACCOUNT,
    targetState: "missing-project",
    sessions: [
      { id: "dream-1", title: "Bridge 回调链路验证", updated: "今天 18:06", messages: 54 },
      { id: "dream-2", title: "QQ 群消息去重策略", updated: "7 月 29 日", messages: 37 },
      { id: "dream-3", title: "模型 Gate 失败记录", updated: "7 月 28 日", messages: 23 },
      { id: "dream-4", title: "MCP 工具边界讨论", updated: "7 月 27 日", messages: 62 },
    ],
  },
  {
    id: "agentall",
    title: "AgentAll",
    source: CURRENT_ACCOUNT,
    targetState: "already-current",
    sessions: [
      { id: "agent-1", title: "Foundation Gate 0", updated: "7 月 26 日", messages: 31 },
      { id: "agent-2", title: "ADR 0026 复审", updated: "7 月 25 日", messages: 18 },
    ],
  },
];

const variants: Array<{ key: VariantKey; name: string }> = [
  { key: "A", name: "总览工作台" },
  { key: "B", name: "分步任务流" },
  { key: "C", name: "历史库主导" },
  { key: "D", name: "融合工作台" },
];

const operationStages = ["正在准备", "正在备份", "正在写入", "正在验证", "已完成"];

const navItems: Array<{ key: ViewKey; label: string; icon: LucideIcon }> = [
  { key: "sync", label: "同步", icon: LayoutDashboard },
  { key: "history", label: "历史库", icon: Library },
  { key: "operations", label: "操作与备份", icon: History },
  { key: "settings", label: "设置", icon: Settings },
];

const dNavItems: Array<{ key: ViewKey; label: string; icon: LucideIcon }> = [
  { key: "sync", label: "历史库", icon: Library },
  { key: "operations", label: "操作与备份", icon: History },
  { key: "settings", label: "设置", icon: Settings },
];

function App() {
  const [variant, setVariant] = useVariant();
  const [view, setView] = useState<ViewKey>("sync");
  const [scopeMode, setScopeMode] = useState<ScopeMode>("custom");
  const [selectedIds, setSelectedIds] = useState(
    new Set<string>(["sub-1", "sub-2", "dream-1"]),
  );
  const [scanning, setScanning] = useState(false);
  const [scanProgress, setScanProgress] = useState(0);
  const [historyRevision, setHistoryRevision] = useState(0);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [workflowOpen, setWorkflowOpen] = useState(false);
  const [operationStage, setOperationStage] = useState(-1);
  const [lastOperationDone, setLastOperationDone] = useState(false);
  const [autoScan, setAutoScan] = useState(false);
  const [reopenTrae, setReopenTrae] = useState(true);

  useEffect(() => {
    if (!scanning) return;

    const timer = window.setInterval(() => {
      setScanProgress((value) => {
        if (value >= 92) {
          window.clearInterval(timer);
          setScanning(false);
          setHistoryRevision((revision) => revision + 1);
          return 100;
        }
        return value + 8;
      });
    }, 140);

    return () => window.clearInterval(timer);
  }, [scanning]);

  useEffect(() => {
    if (operationStage < 0 || operationStage >= operationStages.length - 1) return;

    const timer = window.setTimeout(() => {
      setOperationStage((stage) => stage + 1);
    }, operationStage === 1 ? 1100 : 760);

    return () => window.clearTimeout(timer);
  }, [operationStage]);

  useEffect(() => {
    if (operationStage === operationStages.length - 1) setLastOperationDone(true);
  }, [operationStage]);

  const toggleSession = (id: string) => {
    setSelectedIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const selectWholeDreamBot = () => {
    setSelectedIds((current) => {
      const next = new Set(current);
      projects[1].sessions.forEach((session) => next.add(session.id));
      return next;
    });
  };

  const plan = useMemo(
    () => buildPlan(scopeMode, selectedIds),
    [scopeMode, selectedIds],
  );

  const beginScan = () => {
    if (scanning) return;
    setScanProgress(0);
    setScanning(true);
  };

  const beginApply = () => {
    setConfirmOpen(false);
    setOperationStage(0);
    setLastOperationDone(false);
  };

  const props: VariantProps = {
    view,
    setView,
    scopeMode,
    setScopeMode,
    selectedIds,
    toggleSession,
    selectWholeDreamBot,
    planRows: plan.rows,
    selectedCount: plan.selectedCount,
    partialDreamBot: plan.partialDreamBot,
    scanning,
    scanProgress,
    historyRevision,
    onScan: beginScan,
    onApply: () => setConfirmOpen(true),
    autoScan,
    setAutoScan,
    reopenTrae,
    setReopenTrae,
    lastOperationDone,
  };

  return (
    <div className="prototype-root">
      {variant === "A" && <VariantA {...props} />}
      {variant === "B" && <VariantB {...props} />}
      {variant === "C" && <VariantC {...props} />}
      {variant === "D" && <VariantD {...props} onOpenWorkflow={() => setWorkflowOpen(true)} />}

      {confirmOpen && (
        <ConfirmDialog
          planRows={plan.rows}
          reopenTrae={reopenTrae}
          setReopenTrae={setReopenTrae}
          onCancel={() => setConfirmOpen(false)}
          onConfirm={beginApply}
        />
      )}

      {workflowOpen && (
        <WorkflowDialog
          planRows={plan.rows}
          reopenTrae={reopenTrae}
          setReopenTrae={setReopenTrae}
          onCancel={() => setWorkflowOpen(false)}
          onConfirm={() => {
            setWorkflowOpen(false);
            beginApply();
          }}
        />
      )}

      {operationStage >= 0 && (
        <OperationDialog
          stage={operationStage}
          onClose={() => setOperationStage(-1)}
        />
      )}

      <PrototypeSwitcher variant={variant} setVariant={setVariant} />
    </div>
  );
}

function buildPlan(scopeMode: ScopeMode, selectedIds: Set<string>) {
  const countSelected = (project: Project) =>
    scopeMode === "all"
      ? project.sessions.length
      : project.sessions.filter((session) => selectedIds.has(session.id)).length;

  const subCount = countSelected(projects[0]);
  const dreamCount = countSelected(projects[1]);
  const agentCount = countSelected(projects[2]);
  const partialDreamBot = dreamCount > 0 && dreamCount < projects[1].sessions.length;
  const rows: PlanRow[] = [];

  if (subCount > 0) {
    rows.push({
      id: "attach-sub2api",
      title: "Sub2API",
      detail: `加入当前账号已有项目 · ${subCount} 条对话`,
      count: subCount,
      tone: "info",
      kind: "attach",
    });
  }

  if (dreamCount === projects[1].sessions.length) {
    rows.push({
      id: "follow-dreambot",
      title: "DreamBot",
      detail: `完整项目跟随当前账号 · ${dreamCount} 条对话`,
      count: dreamCount,
      tone: "success",
      kind: "follow",
    });
  } else if (partialDreamBot) {
    rows.push({
      id: "exclude-dreambot",
      title: "DreamBot",
      detail: "目标账号没有该项目，不能只转入部分对话",
      count: dreamCount,
      tone: "warning",
      kind: "excluded",
    });
  }

  if (agentCount > 0) {
    rows.push({
      id: "current-agentall",
      title: "AgentAll",
      detail: `已在当前账号 · ${agentCount} 条对话`,
      count: agentCount,
      tone: "neutral",
      kind: "current",
    });
  }

  if (scopeMode === "all") {
    rows.push({
      id: "unknown-project",
      title: "未识别工作区",
      detail: "项目身份不足，本次排除",
      count: 1,
      tone: "danger",
      kind: "excluded",
    });
  }

  return {
    rows,
    partialDreamBot,
    selectedCount: subCount + dreamCount + agentCount,
  };
}

function useVariant(): [VariantKey, (variant: VariantKey) => void] {
  const readVariant = () => {
    const value = new URLSearchParams(window.location.search).get("variant");
    return variants.some((variant) => variant.key === value) ? (value as VariantKey) : "D";
  };

  const [variant, setVariantState] = useState<VariantKey>(readVariant);

  const setVariant = (next: VariantKey) => {
    const url = new URL(window.location.href);
    url.searchParams.set("variant", next);
    window.history.replaceState({}, "", url);
    setVariantState(next);
  };

  return [variant, setVariant];
}

function VariantA(props: VariantProps) {
  return (
    <div className="variant-a">
      <aside className="a-sidebar">
        <Brand compact={false} />
        <nav className="side-nav" aria-label="主导航">
          {navItems.map((item) => (
            <NavButton key={item.key} item={item} active={props.view === item.key} onClick={() => props.setView(item.key)} />
          ))}
        </nav>
        <div className="sidebar-storage">
          <div className="storage-title"><HardDrive size={15} /> 历史库存储</div>
          <div className="storage-meter"><span style={{ width: "58%" }} /></div>
          <div className="storage-copy"><span>2.9 GB</span><span>5 GB 警戒线</span></div>
        </div>
      </aside>

      <main className="a-main">
        <ContextHeader scanning={props.scanning} onScan={props.onScan} />
        {props.view === "sync" ? (
          <>
            <header className="page-heading">
              <div>
                <p className="eyebrow">同步工作台</p>
                <h1>让历史跟随当前账号</h1>
              </div>
              <PrimaryAction onClick={props.onApply} disabled={props.scanning} />
            </header>

            <section className="metric-band" aria-label="历史概况">
              <Metric label="已见账号" value="3" note="2 个可识别" />
              <Metric label="项目" value="15" note="13 个可同步" />
              <Metric label="完整对话" value={String(34 + props.historyRevision)} note="刚刚检查" />
              <Metric label="当前目标" value="1804…2451" note="证据一致" tone="success" />
            </section>

            <div className="a-work-grid">
              <section className="section-panel scope-panel">
                <SectionTitle icon={ListFilter} title="同步范围" action={<ScopeToggle mode={props.scopeMode} setMode={props.setScopeMode} />} />
                <ProjectPicker {...props} />
              </section>

              <section className="section-panel plan-panel">
                <SectionTitle icon={ShieldCheck} title="变化预览" action={<span className="quiet-count">{props.planRows.length} 项</span>} />
                <PlanList rows={props.planRows} onSelectWhole={props.selectWholeDreamBot} />
                <PlanFooter selectedCount={props.selectedCount} />
              </section>
            </div>
          </>
        ) : (
          <UtilityView {...props} />
        )}
      </main>
    </div>
  );
}

function VariantB(props: VariantProps) {
  const completedStep = props.lastOperationDone ? 4 : props.scanning ? 0 : 2;

  return (
    <div className="variant-b">
      <header className="b-topbar">
        <Brand compact />
        <nav className="top-nav" aria-label="主导航">
          {navItems.map((item) => (
            <button key={item.key} className={props.view === item.key ? "active" : ""} onClick={() => props.setView(item.key)}>
              {item.label}
            </button>
          ))}
        </nav>
        <AccountChip />
      </header>

      <ContextRibbon scanning={props.scanning} onScan={props.onScan} />

      {props.view === "sync" ? (
        <main className="b-body">
          <aside className="step-rail" aria-label="同步阶段">
            {["查找历史", "选择范围", "确认变化", "安全应用", "完成"].map((label, index) => (
              <div className={`step-item ${index <= completedStep ? "done" : ""} ${index === completedStep ? "current" : ""}`} key={label}>
                <span className="step-dot">{index < completedStep ? <Check size={14} /> : index + 1}</span>
                <div><strong>{label}</strong><small>{stepDescription(index)}</small></div>
              </div>
            ))}
          </aside>

          <section className="b-focus">
            <div className="focus-title-row">
              <div>
                <p className="eyebrow">第 2 步</p>
                <h1>选择要继续使用的历史</h1>
              </div>
              <ScopeToggle mode={props.scopeMode} setMode={props.setScopeMode} />
            </div>

            <div className="focus-selection">
              <ProjectPicker {...props} compact />
            </div>

            <div className="focus-action-row">
              <span>{props.selectedCount} 条对话进入预览</span>
              <button className="primary-button" onClick={props.onApply}><Play size={16} />查看并应用</button>
            </div>
          </section>

          <aside className="b-review">
            <div className="review-header">
              <div><ShieldCheck size={18} /><strong>应用摘要</strong></div>
              <span>{props.planRows.length}</span>
            </div>
            <PlanList rows={props.planRows} onSelectWhole={props.selectWholeDreamBot} condensed />
            <div className="safety-facts">
              <div><LockKeyhole size={15} /><span>写前双备份</span></div>
              <div><Database size={15} /><span>完整检查后才完成</span></div>
              <div><Archive size={15} /><span>不会自动删除历史</span></div>
            </div>
          </aside>
        </main>
      ) : (
        <main className="b-utility"><UtilityView {...props} /></main>
      )}
    </div>
  );
}

function VariantC(props: VariantProps) {
  const [activeProject, setActiveProject] = useState("sub2api");
  const project = projects.find((item) => item.id === activeProject) ?? projects[0];

  return (
    <div className="variant-c">
      <aside className="c-icon-rail">
        <Brand compact iconOnly />
        <nav aria-label="主导航">
          {navItems.map((item) => {
            const Icon = item.icon;
            return (
              <button key={item.key} className={props.view === item.key ? "active" : ""} onClick={() => props.setView(item.key)} title={item.label} aria-label={item.label}>
                <Icon size={20} />
              </button>
            );
          })}
        </nav>
      </aside>

      <main className="c-main">
        <header className="c-header">
          <div className="c-title"><strong>历史库</strong><span>3 个账号 · 15 个项目 · {34 + props.historyRevision} 条对话</span></div>
          <div className="c-context"><span>TRAE Work CN</span><ChevronRight size={14} /><span className="account-online">1804…2451</span></div>
          <ScanButton scanning={props.scanning} progress={props.scanProgress} onClick={props.onScan} iconOnly />
        </header>

        {props.view === "sync" ? (
          <div className="c-explorer">
            <aside className="project-tree">
              <div className="tree-toolbar"><Search size={15} /><input aria-label="查找项目" placeholder="查找项目或对话" /></div>
              <div className="account-group">
                <div className="account-group-title"><ChevronDown size={15} /><UserRound size={15} /><span>2578…8841</span><b>2</b></div>
                {projects.slice(0, 2).map((item) => (
                  <button key={item.id} className={activeProject === item.id ? "active" : ""} onClick={() => setActiveProject(item.id)}>
                    <FolderGit2 size={16} /><span>{item.title}</span><b>{item.sessions.length}</b>
                  </button>
                ))}
              </div>
              <div className="account-group">
                <div className="account-group-title"><ChevronDown size={15} /><UserRound size={15} /><span>1804…2451</span><b>1</b></div>
                <button className={activeProject === "agentall" ? "active" : ""} onClick={() => setActiveProject("agentall")}>
                  <FolderGit2 size={16} /><span>AgentAll</span><b>2</b>
                </button>
              </div>
            </aside>

            <section className="conversation-pane">
              <div className="conversation-toolbar">
                <div><h1>{project.title}</h1><span>{project.source === CURRENT_ACCOUNT ? "当前账号" : `首次发现于 ${project.source.slice(0, 4)}…`}</span></div>
                <button className="icon-button" title="筛选" aria-label="筛选"><SlidersHorizontal size={17} /></button>
              </div>
              <div className="conversation-list">
                {project.sessions.map((session) => (
                  <label className={`conversation-row ${props.selectedIds.has(session.id) || props.scopeMode === "all" ? "selected" : ""}`} key={session.id}>
                    <input
                      type="checkbox"
                      checked={props.scopeMode === "all" || props.selectedIds.has(session.id)}
                      disabled={props.scopeMode === "all"}
                      onChange={() => props.toggleSession(session.id)}
                    />
                    <span className="fake-check"><Check size={13} /></span>
                    <MessageSquare size={17} />
                    <span className="conversation-copy"><strong>{session.title}</strong><small>{session.updated} · {session.messages} 条消息</small></span>
                    <ChevronRight size={16} />
                  </label>
                ))}
              </div>
            </section>

            <aside className="sync-tray">
              <div className="tray-heading"><div><ScanSearch size={18} /><strong>同步清单</strong></div><ScopeToggle mode={props.scopeMode} setMode={props.setScopeMode} small /></div>
              <PlanList rows={props.planRows} onSelectWhole={props.selectWholeDreamBot} condensed />
              <div className="tray-spacer" />
              <PlanFooter selectedCount={props.selectedCount} />
              <PrimaryAction onClick={props.onApply} disabled={props.scanning} full />
            </aside>
          </div>
        ) : (
          <div className="c-utility"><UtilityView {...props} /></div>
        )}
      </main>
    </div>
  );
}

function VariantD(props: VariantProps & { onOpenWorkflow: () => void }) {
  const [activeProject, setActiveProject] = useState("sub2api");
  const [preview, setPreview] = useState<{ project: Project; session: Session } | null>(null);
  const project = projects.find((item) => item.id === activeProject) ?? projects[0];
  const syncableCount = props.planRows
    .filter((row) => row.kind === "attach" || row.kind === "follow")
    .reduce((total, row) => total + row.count, 0);

  return (
    <div className="variant-c variant-d">
      <aside className="c-icon-rail">
        <Brand compact iconOnly />
        <nav aria-label="主导航">
          {dNavItems.map((item) => {
            const Icon = item.icon;
            return (
              <button key={item.key} className={props.view === item.key ? "active" : ""} onClick={() => props.setView(item.key)} title={item.label} aria-label={item.label}>
                <Icon size={20} />
              </button>
            );
          })}
        </nav>
      </aside>

      <main className="c-main">
        <header className="c-header d-header">
          <div className="d-brand"><strong>Trae Sync</strong><span>Work CN</span></div>
          <div className="c-title"><strong>历史库</strong><span>所有账号的对话统一查看和选择</span></div>
          <div className="d-header-actions"><AccountChip /><ScanButton scanning={props.scanning} progress={props.scanProgress} onClick={props.onScan} iconOnly /></div>
        </header>

        {props.view === "sync" ? (
          <>
            <section className="metric-band d-metric-band" aria-label="历史与目标概况">
              <Metric label="已收录账号" value="2" note="身份均已识别" />
              <Metric label="项目" value="3" note="全部可处理" />
              <Metric label="完整对话" value={String(9 + props.historyRevision)} note="刚刚检查" />
              <Metric label="当前目标" value="1804…2451" note="证据一致" tone="success" />
            </section>

            <div className="c-explorer d-explorer">
              <aside className="project-tree">
                <div className="tree-toolbar"><Search size={15} /><input aria-label="查找项目或对话" placeholder="查找项目或对话" /></div>
                <div className="account-group">
                  <div className="account-group-title"><ChevronDown size={15} /><UserRound size={15} /><span>2578…8841</span><b>2 个项目</b></div>
                  {projects.slice(0, 2).map((item) => (
                    <button key={item.id} className={activeProject === item.id ? "active" : ""} onClick={() => setActiveProject(item.id)}>
                      <FolderGit2 size={16} /><span>{item.title}</span><b>{item.sessions.length}</b>
                    </button>
                  ))}
                </div>
                <div className="account-group">
                  <div className="account-group-title"><ChevronDown size={15} /><UserRound size={15} /><span>1804…2451</span><b>当前</b></div>
                  <button className={activeProject === "agentall" ? "active" : ""} onClick={() => setActiveProject("agentall")}>
                    <FolderGit2 size={16} /><span>AgentAll</span><b>2</b>
                  </button>
                </div>
              </aside>

              <section className="conversation-pane">
                <div className="conversation-toolbar">
                  <div><h1>{project.title}</h1><span>{project.source === CURRENT_ACCOUNT ? "当前账号中的项目" : `来源账号 ${project.source.slice(0, 4)}…${project.source.slice(-4)}`}</span></div>
                  <button className="icon-button" title="筛选" aria-label="筛选"><SlidersHorizontal size={17} /></button>
                </div>
                <div className="conversation-list">
                  {project.sessions.map((session) => (
                    <div className={`conversation-row d-conversation-row ${props.selectedIds.has(session.id) || props.scopeMode === "all" ? "selected" : ""}`} key={session.id}>
                      <label className="conversation-check" title="选择此对话">
                        <input
                          type="checkbox"
                          checked={props.scopeMode === "all" || props.selectedIds.has(session.id)}
                          disabled={props.scopeMode === "all"}
                          onChange={() => props.toggleSession(session.id)}
                        />
                        <span className="fake-check"><Check size={13} /></span>
                      </label>
                      <MessageSquare size={17} />
                      <button className="conversation-open" onClick={() => setPreview({ project, session })}>
                        <span className="conversation-copy"><strong>{session.title}</strong><small>{session.updated} · {session.messages} 条消息</small></span>
                        <ChevronRight size={16} />
                      </button>
                    </div>
                  ))}
                </div>
              </section>

              <aside className="sync-tray d-sync-tray">
                <div className="tray-heading"><div><ShieldCheck size={18} /><strong>同步计划</strong></div><ScopeToggle mode={props.scopeMode} setMode={props.setScopeMode} small /></div>
                <div className="d-target-card">
                  <span>应用目标</span>
                  <strong><UserRound size={15} />1804…2451</strong>
                  <small>TRAE Work CN · 当前账号证据一致</small>
                </div>
                <PlanList rows={props.planRows} onSelectWhole={props.selectWholeDreamBot} condensed />
                <div className="tray-spacer" />
                <div className="safety-facts d-safety-facts">
                  <div><LockKeyhole size={15} /><span>应用前强制创建双备份</span></div>
                  <div><Archive size={15} /><span>不会自动删除任何历史</span></div>
                </div>
                <div className="plan-footer d-plan-footer">
                  <div><span>已选择</span><strong>{props.selectedCount} 条对话</strong></div>
                  <div><span>本次可同步</span><strong>{syncableCount} 条对话</strong></div>
                </div>
                <button className="primary-button full" onClick={props.onOpenWorkflow} disabled={props.scanning || syncableCount === 0}><Play size={16} />检查并安全同步</button>
              </aside>
            </div>
          </>
        ) : (
          <div className="c-utility"><UtilityView {...props} /></div>
        )}
      </main>

      {preview && <ConversationPreviewDialog project={preview.project} session={preview.session} onClose={() => setPreview(null)} />}
    </div>
  );
}

function Brand({ compact, iconOnly = false }: { compact: boolean; iconOnly?: boolean }) {
  return (
    <div className={`brand ${compact ? "compact" : ""} ${iconOnly ? "icon-only" : ""}`}>
      <span className="brand-mark"><History size={19} /></span>
      {!iconOnly && <div><strong>Trae Sync</strong><span>Work CN</span></div>}
    </div>
  );
}

function NavButton({ item, active, onClick }: { item: (typeof navItems)[number]; active: boolean; onClick: () => void }) {
  const Icon = item.icon;
  return <button className={active ? "active" : ""} onClick={onClick}><Icon size={18} /><span>{item.label}</span></button>;
}

function ContextHeader({ scanning, onScan }: { scanning: boolean; onScan: () => void }) {
  return (
    <header className="context-header">
      <div className="context-selects">
        <label><span>平台</span><select aria-label="平台"><option>TRAE Work CN</option></select></label>
        <label><span>数据位置</span><select aria-label="数据位置"><option>默认位置 · TRAE SOLO CN</option></select></label>
      </div>
      <div className="context-actions"><AccountChip /><ScanButton scanning={scanning} progress={0} onClick={onScan} /></div>
    </header>
  );
}

function ContextRibbon({ scanning, onScan }: { scanning: boolean; onScan: () => void }) {
  return (
    <div className="context-ribbon">
      <div><span>平台</span><strong>TRAE Work CN</strong></div>
      <div><span>数据位置</span><strong>默认位置</strong></div>
      <div><span>当前账号</span><strong className="account-online">1804…2451</strong></div>
      <ScanButton scanning={scanning} progress={0} onClick={onScan} />
    </div>
  );
}

function AccountChip() {
  return <div className="account-chip"><span /><UserRound size={15} /><div><small>当前账号</small><strong>1804…2451</strong></div></div>;
}

function ScanButton({ scanning, progress, onClick, iconOnly = false }: { scanning: boolean; progress: number; onClick: () => void; iconOnly?: boolean }) {
  return (
    <button className={`scan-button ${iconOnly ? "icon-only" : ""}`} onClick={onClick} disabled={scanning} title="查找新历史" aria-label="查找新历史">
      {scanning ? <Loader2 className="spin" size={16} /> : <RefreshCw size={16} />}
      {!iconOnly && <span>{scanning ? `正在查找${progress ? ` ${progress}%` : ""}` : "查找新历史"}</span>}
    </button>
  );
}

function PrimaryAction({ onClick, disabled, full = false }: { onClick: () => void; disabled: boolean; full?: boolean }) {
  return <button className={`primary-button ${full ? "full" : ""}`} onClick={onClick} disabled={disabled}><Play size={16} />应用到当前账号</button>;
}

function Metric({ label, value, note, tone = "neutral" }: { label: string; value: string; note: string; tone?: Tone }) {
  return <div className={`metric ${tone}`}><span>{label}</span><strong>{value}</strong><small>{note}</small></div>;
}

function SectionTitle({ icon: Icon, title, action }: { icon: LucideIcon; title: string; action: React.ReactNode }) {
  return <div className="section-title"><div><Icon size={18} /><h2>{title}</h2></div>{action}</div>;
}

function ScopeToggle({ mode, setMode, small = false }: { mode: ScopeMode; setMode: (mode: ScopeMode) => void; small?: boolean }) {
  return (
    <div className={`segmented ${small ? "small" : ""}`} role="group" aria-label="同步范围">
      <button className={mode === "all" ? "active" : ""} onClick={() => setMode("all")}>全部</button>
      <button className={mode === "custom" ? "active" : ""} onClick={() => setMode("custom")}>自定义</button>
    </div>
  );
}

function ProjectPicker(props: VariantProps & { compact?: boolean }) {
  const { scopeMode, selectedIds, toggleSession, compact = false } = props;

  return (
    <div className={`project-picker ${compact ? "compact" : ""}`}>
      {projects.map((project) => {
        const selected = project.sessions.filter((session) => selectedIds.has(session.id)).length;
        return (
          <div className="project-block" key={project.id}>
            <div className="project-row">
              <ChevronDown size={16} />
              <FolderGit2 size={17} />
              <span><strong>{project.title}</strong><small>{project.sessions.length} 条对话 · {ownerLabel(project)}</small></span>
              <b>{scopeMode === "all" ? project.sessions.length : selected}/{project.sessions.length}</b>
            </div>
            {scopeMode === "custom" && (
              <div className="session-list">
                {project.sessions.map((session) => (
                  <label key={session.id}>
                    <input type="checkbox" checked={selectedIds.has(session.id)} onChange={() => toggleSession(session.id)} />
                    <span className="fake-check"><Check size={12} /></span>
                    <span><strong>{session.title}</strong><small>{session.updated}</small></span>
                  </label>
                ))}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

function ownerLabel(project: Project) {
  if (project.targetState === "already-current") return "已在当前账号";
  if (project.targetState === "same-project") return "当前账号已有同一项目";
  return "目标账号尚无该项目";
}

function PlanList({ rows, onSelectWhole, condensed = false }: { rows: PlanRow[]; onSelectWhole: () => void; condensed?: boolean }) {
  return (
    <div className={`plan-list ${condensed ? "condensed" : ""}`}>
      {rows.length === 0 && <div className="empty-state"><ListFilter size={20} /><span>尚未选择对话</span></div>}
      {rows.map((row) => (
        <div className={`plan-row ${row.tone}`} key={row.id}>
          <StatusIcon tone={row.tone} />
          <span><strong>{row.title}</strong><small>{row.detail}</small></span>
          {row.kind === "excluded" && row.id === "exclude-dreambot" ? (
            <button className="link-button" onClick={onSelectWhole}>选择整个项目</button>
          ) : (
            <b>{row.count}</b>
          )}
        </div>
      ))}
    </div>
  );
}

function StatusIcon({ tone }: { tone: Tone }) {
  if (tone === "success") return <CheckCircle2 size={18} />;
  if (tone === "warning" || tone === "danger") return <AlertTriangle size={18} />;
  if (tone === "info") return <MessageSquare size={18} />;
  return <Check size={18} />;
}

function PlanFooter({ selectedCount }: { selectedCount: number }) {
  return (
    <div className="plan-footer">
      <div><span>本次范围</span><strong>{selectedCount} 条对话</strong></div>
      <div><span>目标原历史</span><strong>保持不变</strong></div>
    </div>
  );
}

function UtilityView(props: VariantProps) {
  const [query, setQuery] = useState("");
  const visibleSessions = projects.flatMap((project) => project.sessions.map((session) => ({ ...session, project: project.title })))
    .filter((session) => session.title.toLowerCase().includes(query.toLowerCase()));

  if (props.view === "history") {
    return (
      <section className="utility-view">
        <div className="utility-heading"><div><p className="eyebrow">历史库</p><h1>全部对话</h1></div><label className="search-field"><Search size={16} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索标题" /></label></div>
        <div className="history-table">
          {visibleSessions.map((session) => <div key={session.id}><MessageSquare size={17} /><span><strong>{session.title}</strong><small>{session.project} · {session.updated}</small></span><b>{session.messages}</b></div>)}
        </div>
      </section>
    );
  }

  if (props.view === "operations") {
    return (
      <section className="utility-view">
        <div className="utility-heading"><div><p className="eyebrow">操作与备份</p><h1>最近操作</h1></div></div>
        <div className="operations-list">
          {props.lastOperationDone && <OperationRow status="已完成" tone="success" time="刚刚" title="应用到 1804…2451" detail="双备份与完整性检查通过" />}
          <OperationRow status="已验证" tone="success" time="7 月 30 日 13:42" title="历史应用到账号" detail="3 个项目 · 7 条对话" />
          <OperationRow status="需要处理" tone="warning" time="7 月 29 日 21:18" title="扫描历史" detail="1 个项目身份暂时无法确认" />
        </div>
      </section>
    );
  }

  return (
    <section className="utility-view settings-view">
      <div className="utility-heading"><div><p className="eyebrow">设置</p><h1>常用设置</h1></div></div>
      <div className="settings-list">
        <ToggleRow title="自动查找新历史" detail="默认关闭；仅在已授权且 TRAE 已停止时读取" checked={props.autoScan} setChecked={props.setAutoScan} />
        <ToggleRow title="同步成功后重新打开 TRAE" detail="失败或恢复状态下不会启动" checked={props.reopenTrae} setChecked={props.setReopenTrae} />
        <div className="setting-row"><div><strong>历史库存储</strong><small>D:\Trae Sync History · 2.9 GB</small></div><button className="secondary-button">管理</button></div>
      </div>
    </section>
  );
}

function OperationRow({ status, tone, time, title, detail }: { status: string; tone: Tone; time: string; title: string; detail: string }) {
  return <div className="operation-row"><StatusIcon tone={tone} /><span><strong>{title}</strong><small>{detail}</small></span><time>{time}</time><span className={`status-label ${tone}`}>{status}</span><button className="icon-button" title="更多" aria-label="更多"><MoreHorizontal size={17} /></button></div>;
}

function ToggleRow({ title, detail, checked, setChecked }: { title: string; detail: string; checked: boolean; setChecked: (value: boolean) => void }) {
  return <label className="setting-row"><div><strong>{title}</strong><small>{detail}</small></div><input className="toggle-input" type="checkbox" checked={checked} onChange={(event) => setChecked(event.target.checked)} /><span className="toggle"><span /></span></label>;
}

function stepDescription(index: number) {
  return ["扫描本地数据", "决定同步范围", "检查排除项", "备份并验证", "查看结果"][index];
}

function ConfirmDialog({ planRows, reopenTrae, setReopenTrae, onCancel, onConfirm }: { planRows: PlanRow[]; reopenTrae: boolean; setReopenTrae: (value: boolean) => void; onCancel: () => void; onConfirm: () => void }) {
  const activeRows = planRows.filter((row) => row.kind === "attach" || row.kind === "follow");

  return (
    <div className="dialog-backdrop" role="presentation">
      <section className="dialog" role="dialog" aria-modal="true" aria-labelledby="confirm-title">
        <div className="dialog-header"><div><span className="dialog-icon"><ShieldCheck size={20} /></span><div><h2 id="confirm-title">确认应用到当前账号</h2><p>TRAE Work CN · 1804…2451</p></div></div><button className="icon-button" onClick={onCancel} title="关闭" aria-label="关闭"><X size={18} /></button></div>
        <div className="dialog-body">
          <div className="confirm-summary"><div><span>将处理</span><strong>{activeRows.reduce((total, row) => total + row.count, 0)} 条对话</strong></div><div><span>写入方式</span><strong>单事务</strong></div><div><span>恢复副本</span><strong>强制创建</strong></div></div>
          <div className="ownership-note"><AlertTriangle size={18} /><span>转入后，来源账号不再显示这些项目或对话；目标账号原有历史保持不变。</span></div>
          <div className="confirm-list">{activeRows.map((row) => <div key={row.id}><FolderGit2 size={16} /><span><strong>{row.title}</strong><small>{row.detail}</small></span><b>{row.count}</b></div>)}</div>
          <ToggleRow title="完成后重新打开 TRAE" detail="验证全部通过后执行" checked={reopenTrae} setChecked={setReopenTrae} />
        </div>
        <div className="dialog-actions"><button className="secondary-button" onClick={onCancel}>取消</button><button className="primary-button" onClick={onConfirm}><Play size={16} />开始安全应用</button></div>
      </section>
    </div>
  );
}

function WorkflowDialog({ planRows, reopenTrae, setReopenTrae, onCancel, onConfirm }: { planRows: PlanRow[]; reopenTrae: boolean; setReopenTrae: (value: boolean) => void; onCancel: () => void; onConfirm: () => void }) {
  const activeRows = planRows.filter((row) => row.kind === "attach" || row.kind === "follow");
  const total = activeRows.reduce((sum, row) => sum + row.count, 0);

  return (
    <div className="dialog-backdrop" role="presentation">
      <section className="dialog workflow-dialog" role="dialog" aria-modal="true" aria-labelledby="workflow-title">
        <div className="dialog-header">
          <div><span className="dialog-icon"><ShieldCheck size={20} /></span><div><h2 id="workflow-title">确认同步计划</h2><p>第 3 步，共 5 步 · 尚未写入数据</p></div></div>
          <button className="icon-button" onClick={onCancel} title="关闭" aria-label="关闭"><X size={18} /></button>
        </div>
        <div className="workflow-step-track" aria-label="同步阶段">
          {["查找历史", "选择范围", "确认变化", "安全应用", "完成"].map((label, index) => (
            <div className={index < 2 ? "done" : index === 2 ? "active" : ""} key={label}>
              <span>{index < 2 ? <Check size={13} /> : index + 1}</span>
              <small>{label}</small>
            </div>
          ))}
        </div>
        <div className="dialog-body">
          <div className="confirm-summary"><div><span>将同步</span><strong>{total} 条对话</strong></div><div><span>目标账号</span><strong>1804…2451</strong></div><div><span>恢复保障</span><strong>强制双备份</strong></div></div>
          <div className="ownership-note"><AlertTriangle size={18} /><span>目标账号原有历史保持不变；转入后，来源账号将不再显示这些项目或对话，但 Trae Sync 历史库和写前备份均会保留。</span></div>
          <div className="confirm-list">{activeRows.map((row) => <div key={row.id}><FolderGit2 size={16} /><span><strong>{row.title}</strong><small>{row.detail}</small></span><b>{row.count}</b></div>)}</div>
          <div className="workflow-safety-grid">
            <div><LockKeyhole size={17} /><span><strong>写入前</strong><small>关闭 TRAE 并校验双备份</small></span></div>
            <div><Database size={17} /><span><strong>写入后</strong><small>检查数据库和对话数量</small></span></div>
          </div>
          <ToggleRow title="完成后重新打开 TRAE" detail="仅在所有验证通过后执行" checked={reopenTrae} setChecked={setReopenTrae} />
        </div>
        <div className="dialog-actions"><button className="secondary-button" onClick={onCancel}>返回调整</button><button className="primary-button" onClick={onConfirm}><Play size={16} />创建备份并应用</button></div>
      </section>
    </div>
  );
}

function ConversationPreviewDialog({ project, session, onClose }: { project: Project; session: Session; onClose: () => void }) {
  return (
    <div className="dialog-backdrop" role="presentation">
      <section className="dialog conversation-preview-dialog" role="dialog" aria-modal="true" aria-labelledby="preview-title">
        <div className="dialog-header">
          <div><span className="dialog-icon"><MessageSquare size={20} /></span><div><h2 id="preview-title">{session.title}</h2><p>{project.title} · {session.updated} · {session.messages} 条消息</p></div></div>
          <button className="icon-button" onClick={onClose} title="关闭" aria-label="关闭"><X size={18} /></button>
        </div>
        <div className="preview-messages">
          <article><span className="preview-avatar"><UserRound size={15} /></span><div><strong>用户</strong><p>继续检查当前方案，并确认这一部分的实际行为是否符合预期。</p></div></article>
          <article><span className="preview-avatar assistant"><History size={15} /></span><div><strong>TRAE</strong><p>已读取相关上下文。下面将按现有项目结构核对数据，并保留可验证的结果。</p></div></article>
          <article><span className="preview-avatar"><UserRound size={15} /></span><div><strong>用户</strong><p>确认后继续，原有记录不要删除。</p></div></article>
        </div>
        <div className="preview-footer"><span>预览内容来自 Trae Sync 历史库</span><button className="secondary-button" onClick={onClose}>关闭</button></div>
      </section>
    </div>
  );
}

function OperationDialog({ stage, onClose }: { stage: number; onClose: () => void }) {
  const done = stage === operationStages.length - 1;

  return (
    <div className="dialog-backdrop" role="presentation">
      <section className="dialog operation-dialog" role="dialog" aria-modal="true" aria-labelledby="operation-title">
        <div className="dialog-header"><div><span className={`dialog-icon ${done ? "done" : ""}`}>{done ? <CheckCircle2 size={20} /> : <Loader2 className="spin" size={20} />}</span><div><h2 id="operation-title">{done ? "历史已跟随当前账号" : operationStages[stage]}</h2><p>{done ? "所有验证均已通过" : "请保持 TRAE 关闭"}</p></div></div></div>
        <div className="stage-track">
          {operationStages.map((label, index) => <div className={index < stage ? "done" : index === stage ? "active" : ""} key={label}><span>{index < stage || done ? <Check size={13} /> : index + 1}</span><small>{label}</small></div>)}
        </div>
        <div className="operation-detail">
          {done ? <><ShieldCheck size={20} /><span><strong>双备份与完整性检查通过</strong><small>目标账号原有历史保持不变，操作记录已保存。</small></span></> : <><Clock3 size={20} /><span><strong>{operationStages[stage]}</strong><small>{stage === 2 ? "写入开始后不可取消" : "正在保存操作证据"}</small></span></>}
        </div>
        <div className="dialog-actions"><button className="primary-button" disabled={!done} onClick={onClose}>{done ? "完成" : "处理中"}</button></div>
      </section>
    </div>
  );
}

function PrototypeSwitcher({ variant, setVariant }: { variant: VariantKey; setVariant: (variant: VariantKey) => void }) {
  const currentIndex = variants.findIndex((item) => item.key === variant);

  const cycle = (direction: number) => {
    const nextIndex = (currentIndex + direction + variants.length) % variants.length;
    setVariant(variants[nextIndex].key);
  };

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target?.matches("input, textarea, select, [contenteditable='true']")) return;
      if (event.key === "ArrowLeft") cycle(-1);
      if (event.key === "ArrowRight") cycle(1);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  });

  if (!import.meta.env.DEV) return null;

  return (
    <div className="prototype-switcher">
      <button onClick={() => cycle(-1)} title="上一个方案" aria-label="上一个方案"><ArrowLeft size={17} /></button>
      <span><b>{variant}</b>{variants[currentIndex].name}</span>
      <button onClick={() => cycle(1)} title="下一个方案" aria-label="下一个方案"><ArrowRight size={17} /></button>
    </div>
  );
}

export default App;
