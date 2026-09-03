import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  CheckCircle2,
  Combine,
  Database,
  Layers,
  LogIn,
  MonitorPlay,
  Pencil,
  Plus,
  RefreshCw,
  Trash2,
  TriangleAlert,
} from "lucide-react";
import type {
  EnvironmentListItemDto,
  EnvironmentStateDto,
} from "../types/environment";
import type {
  MasterCheckupDto,
  MasterIncorporateProgressEvent,
  MasterIncorporateResultDto,
  MasterLibraryStatsDto,
} from "../types/masterLibrary";
import type { AccountProfileDto, ManagedAccountsViewDto } from "../types/account_switch";
import { safeUiErrorMessage } from "../utils/safeUiError";
import { InstanceSlotBadge } from "./StatusBadges";

interface EnvironmentPageProps {
  /** 页面可见时才读取环境状态，避免后台 IPC；隐藏时停止轮询。 */
  active: boolean;
  /** 打开主库详情页（P5-8a-2：主库卡入口）。 */
  onOpenMasterDetail?: () => void;
}

/**
 * 环境页 V2（P5-2 + P6-4，ADR-0024）：
 * 主库环境卡（默认环境，置顶不可删）+ 辅助环境列表（创建/登录账号/
 * 启动/重命名/删除）。主库卡沿用 P5-2 编排（统计/体检/收编/启动）。
 */
export function EnvironmentPage({ active, onOpenMasterDetail }: EnvironmentPageProps) {
  const [state, setState] = useState<EnvironmentStateDto | null>(null);
  // 主库聚合统计：读取失败（fixture 模式等）时为 null，统计格整体不渲染。
  const [masterStats, setMasterStats] = useState<MasterLibraryStatsDto | null>(null);
  // P5-5 主库体检：同读同刷；失败静默降级为 null（体检区块不渲染）。
  const [checkup, setCheckup] = useState<MasterCheckupDto | null>(null);
  // P6-4 环境列表（含主库置顶项；null = 首次未读）。
  const [envList, setEnvList] = useState<EnvironmentListItemDto[] | null>(null);
  // 收编弹层：null = 关闭；确认 → 运行（四阶段进度）→ 完成回执 / 失败提示。
  const [incorporating, setIncorporating] = useState<IncorporatePhase | null>(null);
  // P6-4 辅助环境生命周期弹层（同一时刻至多一个）。
  const [createDialog, setCreateDialog] = useState<NameDialogState | null>(null);
  const [renameDialog, setRenameDialog] = useState<NameDialogState | null>(null);
  const [loginDialogEnv, setLoginDialogEnv] = useState<EnvironmentListItemDto | null>(null);
  const [deleteDialogEnv, setDeleteDialogEnv] = useState<EnvironmentListItemDto | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [launching, setLaunching] = useState(false);
  const [launchingEnvId, setLaunchingEnvId] = useState<string | null>(null);
  const pollCancelled = useRef(false);

  /** 轮询合并：新列表不带体积时沿用旧值（体积遍历较重，轮询不请求）。 */
  const mergeListSizes = useCallback(
    (previous: EnvironmentListItemDto[] | null, next: EnvironmentListItemDto[]) =>
      previous
        ? next.map((item) => {
            if (item.size_bytes !== undefined) return item;
            const before = previous.find((candidate) => candidate.env_id === item.env_id);
            return before?.size_bytes !== undefined ? { ...item, size_bytes: before.size_bytes } : item;
          })
        : next,
    [],
  );

  const load = useCallback(async () => {
    const next = await invoke<EnvironmentStateDto>("get_environment_state");
    setState(next);
    // 统计与体检同状态同读同刷（聚合 SQL 轻量）：手动刷新/启动后/切号/收编后回页都校准。
    // 读取失败静默降级为 null（不渲染对应区块），不阻塞环境状态展示。
    const [stats, checkupReport, list] = await Promise.all([
      invoke<MasterLibraryStatsDto>("get_master_library_stats").catch(() => null),
      invoke<MasterCheckupDto>("get_master_checkup").catch(() => null),
      // 手动刷新带体积（主库三件套 + 副环境整目录口径）。
      invoke<EnvironmentListItemDto[]>("list_environments", { includeSize: true }).catch(
        () => null,
      ),
    ]);
    setMasterStats(stats);
    setCheckup(checkupReport);
    if (list) setEnvList((previous) => mergeListSizes(previous, list));
  }, [mergeListSizes]);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    void load().catch((reason: unknown) => {
      if (!cancelled) setError(safeUiErrorMessage(reason, "环境状态暂时不可读取，请稍后重试。"));
    });
    return () => { cancelled = true; };
  }, [active, load]);

  // 运行态轮询（与账号页实例徽章同模式）：主库可能被用户在 TRAE 内直接关闭/启动，
  // 页面停留期间每 5 秒校准一次；隐藏即停止。轮询不带体积（沿用最近一次的值）。
  useEffect(() => {
    if (!active) return;
    pollCancelled.current = false;
    const poll = async () => {
      if (pollCancelled.current) return;
      const [next, list] = await Promise.all([
        invoke<EnvironmentStateDto>("get_environment_state").catch(() => null),
        invoke<EnvironmentListItemDto[]>("list_environments", { includeSize: false }).catch(
          () => null,
        ),
      ]);
      if (pollCancelled.current) return;
      if (next) setState(next);
      if (list) setEnvList((previous) => mergeListSizes(previous, list));
    };
    const timer = window.setInterval(() => void poll(), 5000);
    return () => {
      pollCancelled.current = true;
      window.clearInterval(timer);
    };
  }, [active, mergeListSizes]);

  /** 统一的动作后反馈：成功消息 + 列表校准；失败走页面级错误条。 */
  const runEnvAction = useCallback(
    async (action: () => Promise<void>, successMessage: string) => {
      setError(null);
      setMessage(null);
      try {
        await action();
        await load();
        setMessage(successMessage);
      } catch (reason: unknown) {
        setError(safeUiErrorMessage(reason, "环境操作未能完成，请稍后重试。"));
      }
    },
    [load],
  );

  // 启动/聚焦主库实例（Q1：主库单实例承载全部对话记录；登录态由切号流程维护）。
  const handleLaunch = useCallback(() => {
    if (launching) return;
    setLaunching(true);
    return runEnvAction(
      async () => {
        await invoke("launch_master_library");
      },
      "主库已启动（或已聚焦到既有窗口）。",
    ).finally(() => setLaunching(false));
  }, [launching, runEnvAction]);

  // P6-4：启动/聚焦辅助环境实例（--user-data-dir 指向环境目录，环境间并行）。
  const handleLaunchEnv = useCallback(
    (env: EnvironmentListItemDto) => {
      if (launchingEnvId) return;
      setLaunchingEnvId(env.env_id);
      return runEnvAction(
        async () => {
          await invoke("launch_environment", { envId: env.env_id });
        },
        `「${env.name}」已启动（或已聚焦到既有窗口）。`,
      ).finally(() => setLaunchingEnvId(null));
    },
    [launchingEnvId, runEnvAction],
  );

  const currentName = state?.current_account_name ?? null;
  const secondaryEnvs = envList === null ? null : envList.filter((env) => !env.is_master);

  // ===== P5-5 体检区块数据派生 + 一键收编 =====

  // 滞留账号（非当前账号的分布行）——收编目标；无滞留时区块整体不渲染。
  const staleAccounts =
    checkup?.status === "ready" ? checkup.accounts.filter((account) => !account.current) : [];
  const staleSessionTotal = staleAccounts.reduce((sum, a) => sum + a.session_count, 0);
  const staleProjectTotal = staleAccounts.reduce((sum, a) => sum + a.project_count, 0);

  /** 确认收编 → 后端四步编排（关实例 → 备份 → 归属改写 → 重启）。 */
  const startIncorporate = useCallback(async () => {
    setIncorporating({ kind: "running", stage: "closing" });
    let unlisten: (() => void) | null = null;
    try {
      // 进度事件（done 只推满进度条；回执以 invoke 返回为准）。
      unlisten = await listen<MasterIncorporateProgressEvent>(
        "master-incorporate-progress",
        (event) => {
          setIncorporating((current) =>
            current?.kind === "running" ? { kind: "running", stage: event.payload.stage } : current,
          );
        },
      );
      const receipt = await invoke<MasterIncorporateResultDto>("incorporate_master_records");
      setIncorporating({ kind: "done", receipt });
      // 收编完成即刷新体检与统计（滞留行清零，区块自然消失）。
      await load();
    } catch (reason: unknown) {
      setIncorporating({
        kind: "failed",
        message: safeUiErrorMessage(reason, "归入未完成，主库数据保持操作前状态；请稍后重试。"),
      });
    } finally {
      unlisten?.();
    }
  }, [load]);

  /** 收编弹层关闭（运行中不允许——后端事务已开）。 */
  const closeIncorporate = useCallback(() => {
    setIncorporating((current) => (current?.kind === "running" ? current : null));
  }, []);

  return (
    <section className="environment-page" role="region" aria-label="环境">
      <header className="page-header">
        <div className="page-header__copy">
          <h1 data-page-title="environment" tabIndex={-1}>环境</h1>
        </div>
        <div className="page-header__actions">
          <button
            className="btn"
            type="button"
            onClick={() => void load().catch(() => undefined)}
            data-testid="environment-refresh"
            title="重新读取环境状态与体积"
          >
            <RefreshCw size={15} aria-hidden="true" />刷新
          </button>
        </div>
      </header>

      {error && <p className="workbench__error" role="alert">{error}</p>}
      {message && <p className="environment-page__message" role="status">{message}</p>}

      <div className="environment-page__body">
        {/* 主库：默认环境（不可删除/不可修改类型） */}
        <div className="env-card" data-testid="env-master-card">
          <div className="env-card__head">
            <span className="env-card__icon" aria-hidden="true"><Database size={22} strokeWidth={1.8} /></span>
            <div className="env-card__title">
              <div className="env-card__name-line">
                <span className="env-card__name">主库</span>
                <span className="env-card__badge">默认环境</span>
              </div>
              <div className="env-card__status" data-testid="env-master-state">
                <InstanceSlotBadge running={state?.running === true} loginState={state?.login_state} />
              </div>
            </div>
            {currentName && (
              <div className="env-card__current">
                <span className="env-card__current-avatar" aria-hidden="true">{currentName.slice(0, 1)}</span>
                <div>
                  <div className="env-card__current-name" data-testid="env-current-name">{currentName}</div>
                  <div className="env-card__current-meta">当前登录</div>
                </div>
              </div>
            )}
          </div>

          {/* P5-4 主库统计格（原型 env-stats 契约）：会话/项目/参与账号。
              仅 stats ready 时渲染；库体量/接力次数等留待 P5-8 详情页。 */}
          {masterStats?.status === "ready" && (
            <div className="env-stats" data-testid="env-master-stats">
              <div className="env-stat" title="主库内全部对话会话数">
                <div className="env-stat__value">{masterStats.session_count}</div>
                <div className="env-stat__label">会话</div>
              </div>
              <div className="env-stat" title="主库内全部项目数">
                <div className="env-stat__value">{masterStats.project_count}</div>
                <div className="env-stat__label">项目</div>
              </div>
              <div className="env-stat" title="主库内出现过的账号数（含历史归属）">
                <div className="env-stat__value">{masterStats.participating_account_count}</div>
                <div className="env-stat__label">参与账号</div>
              </div>
            </div>
          )}

          <p className="env-card__hint">
            {state?.current_profile_id
              ? "全部对话记录都在主库内共享；在「账号」页切换账号，全部记录都会保留。"
              : "主库尚未登录账号：首次启动后请在 TRAE 窗口内登录一次，此后切换账号全自动完成。"}
          </p>

          {/* P5-5 主库体检：有滞留记录才出现（无差异静默，不打扰）。
              账号行用账号名表述；user_id 收进悬浮提示（界面表达纪律）。 */}
          {checkup?.status === "ready" && staleAccounts.length > 0 && (
            <div className="env-checkup" data-testid="env-checkup">
              <div className="env-checkup__head">
                <TriangleAlert size={16} aria-hidden="true" />
                <span>
                  有 {staleAccounts.length} 个账号的 {staleSessionTotal} 个会话在主库中，未随当前账号展示
                </span>
              </div>
              <ul className="env-checkup__list">
                {staleAccounts.map((account) => (
                  <li key={account.user_id} title={`账号标识：${account.user_id}`}>
                    <span className="env-checkup__name">
                      {account.account_name ?? "未登记账号"}
                    </span>
                    <span className="env-checkup__meta">
                      {account.session_count} 个会话 · {account.project_count} 个项目
                    </span>
                  </li>
                ))}
              </ul>
              {checkup.orphan_project_count > 0 && (
                <p className="env-checkup__note">
                  另有 {checkup.orphan_project_count} 条无归属记录不会被归入。
                </p>
              )}
              <button
                className="btn btn--primary env-checkup__action"
                type="button"
                onClick={() => setIncorporating({ kind: "confirm" })}
                data-testid="env-incorporate-entry"
              >
                <Combine size={15} aria-hidden="true" />一键归入当前账号
              </button>
            </div>
          )}

          <div className="env-card__foot">
            {/* 数据目录完整路径收进悬浮提示，不占主视野（界面表达纪律）。 */}
            <span
              className="env-card__path"
              title={state?.data_dir ? `主库数据目录：${state.data_dir}` : undefined}
              data-testid="env-data-dir"
            >
              {state?.data_dir ? "数据目录" : "—"}
            </span>
            <div className="env-card__foot-actions">
              <button
                className="btn"
                type="button"
                onClick={onOpenMasterDetail}
                data-testid="env-master-detail"
                title="查看主库详情（对话记录、统计与备份）"
              >
                <Database size={15} aria-hidden="true" />详情
              </button>
              <button
                className="btn btn--primary"
                type="button"
                onClick={() => void handleLaunch()}
                disabled={launching}
                data-testid="env-master-launch"
                title={state?.running ? "把主库 TRAE 窗口带到前台" : "启动主库 TRAE 实例"}
              >
                <MonitorPlay size={15} aria-hidden="true" />
                {launching ? "处理中…" : state?.running ? "聚焦主库" : "启动主库"}
              </button>
            </div>
          </div>
        </div>

        {/* ===== P6-4 辅助环境区（列表 + 创建入口）===== */}
        <div className="env-section" data-testid="env-secondary-section">
          <div className="env-section__head">
            <h2 className="env-section__title">辅助环境</h2>
            <button
              className="btn"
              type="button"
              onClick={() => setCreateDialog({ name: "", error: null, busy: false })}
              data-testid="env-create-entry"
              title="创建一个新的辅助环境（独立数据目录）"
            >
              <Plus size={15} aria-hidden="true" />新建环境
            </button>
          </div>

          {secondaryEnvs !== null && secondaryEnvs.length === 0 && (
            <div className="env-placeholder" data-testid="env-placeholder">
              <span className="env-placeholder__icon" aria-hidden="true"><Layers size={20} strokeWidth={1.8} /></span>
              <div>
                <div className="env-placeholder__title">还没有辅助环境</div>
                <p className="env-placeholder__text">辅助环境是独立的数据目录，可与主库并行使用；各环境的对话记录互相独立。</p>
              </div>
            </div>
          )}

          {secondaryEnvs !== null && secondaryEnvs.length > 0 && (
            <div className="env-list">
              {secondaryEnvs.map((env) => (
                <SecondaryEnvCard
                  key={env.env_id}
                  env={env}
                  launching={launchingEnvId === env.env_id}
                  onLaunch={() => void handleLaunchEnv(env)}
                  onLogin={() => {
                    setError(null);
                    setLoginDialogEnv(env);
                  }}
                  onRename={() => {
                    setError(null);
                    setRenameDialog({ env, name: env.name, error: null, busy: false });
                  }}
                  onDelete={() => {
                    setError(null);
                    setDeleteDialogEnv(env);
                  }}
                />
              ))}
            </div>
          )}
        </div>
      </div>

      {/* P5-5 收编弹层：确认 → 四阶段进度 → 完成回执 / 失败提示 */}
      {incorporating && (
        <IncorporateDialog
          phase={incorporating}
          currentName={currentName}
          staleAccounts={staleAccounts}
          staleSessionTotal={staleSessionTotal}
          staleProjectTotal={staleProjectTotal}
          orphanProjectCount={checkup?.orphan_project_count ?? 0}
          onConfirm={() => void startIncorporate()}
          onClose={closeIncorporate}
        />
      )}

      {/* P6-4 创建/重命名弹层（同构的名称输入弹层） */}
      {createDialog && (
        <EnvironmentNameDialog
          title="新建环境"
          confirmLabel="创建"
          value={createDialog}
          onChange={setCreateDialog}
          onClose={() => setCreateDialog(null)}
          onSubmit={async (name) => {
            await invoke("create_environment", { name });
            await load();
            setMessage(`已创建环境「${name.trim()}」。`);
          }}
        />
      )}
      {renameDialog && (
        <EnvironmentNameDialog
          title="重命名环境"
          confirmLabel="保存"
          value={renameDialog}
          onChange={setRenameDialog}
          onClose={() => setRenameDialog(null)}
          onSubmit={async (name) => {
            const env = renameDialog.env;
            if (!env) return;
            await invoke("rename_environment", { envId: env.env_id, name });
            await load();
            setMessage(`已重命名为「${name.trim()}」。`);
          }}
        />
      )}

      {/* P6-4 环境登录账号弹层 */}
      {loginDialogEnv && (
        <EnvironmentLoginDialog
          env={loginDialogEnv}
          onClose={() => setLoginDialogEnv(null)}
          onDone={async () => {
            setLoginDialogEnv(null);
            await load();
          }}
        />
      )}

      {/* P6-4 环境删除确认弹层 */}
      {deleteDialogEnv && (
        <EnvironmentDeleteDialog
          env={deleteDialogEnv}
          onClose={() => setDeleteDialogEnv(null)}
          onDone={async () => {
            setDeleteDialogEnv(null);
            await load();
            setMessage(`已删除环境「${deleteDialogEnv.name}」。`);
          }}
        />
      )}
    </section>
  );
}

/** P6-4 辅助环境卡：名称 + 当前账号 + 运行态 + 体积 + 四项操作。 */
function SecondaryEnvCard({
  env,
  launching,
  onLaunch,
  onLogin,
  onRename,
  onDelete,
}: {
  env: EnvironmentListItemDto;
  launching: boolean;
  onLaunch: () => void;
  onLogin: () => void;
  onRename: () => void;
  onDelete: () => void;
}) {
  return (
    <div className="env-card" data-testid={`env-card-${env.env_id}`}>
      <div className="env-card__head">
        <span className="env-card__icon" aria-hidden="true"><Layers size={22} strokeWidth={1.8} /></span>
        <div className="env-card__title">
          <div className="env-card__name-line">
            <span className="env-card__name">{env.name}</span>
            <span className="env-card__badge">辅助环境</span>
          </div>
          <div className="env-card__status">
            <InstanceSlotBadge running={env.running} loginState={env.login_state} />
          </div>
        </div>
        {env.current_account_name && (
          <div className="env-card__current">
            <span className="env-card__current-avatar" aria-hidden="true">
              {env.current_account_name.slice(0, 1)}
            </span>
            <div>
              <div className="env-card__current-name">{env.current_account_name}</div>
              <div className="env-card__current-meta">当前登录</div>
            </div>
          </div>
        )}
      </div>

      <div className="env-card__foot">
        {/* 体积与目录路径收进次信息；路径完整值进悬浮提示（界面表达纪律）。 */}
        <span
          className="env-card__path"
          title={`数据目录：${env.data_dir}`}
          data-testid={`env-path-${env.env_id}`}
        >
          {env.size_bytes !== undefined ? `${formatBytes(env.size_bytes)} · 数据目录` : "数据目录"}
        </span>
        <div className="env-card__foot-actions">
          <button
            className="btn"
            type="button"
            onClick={onLogin}
            data-testid={`env-login-${env.env_id}`}
            title="选择一个账号登录该环境"
          >
            <LogIn size={15} aria-hidden="true" />登录账号
          </button>
          <button
            className="btn"
            type="button"
            onClick={onRename}
            data-testid={`env-rename-${env.env_id}`}
            title="重命名该环境"
          >
            <Pencil size={15} aria-hidden="true" />重命名
          </button>
          <button
            className="btn btn--danger"
            type="button"
            onClick={onDelete}
            data-testid={`env-delete-${env.env_id}`}
            title="删除该环境（删除前会列出数据规模）"
          >
            <Trash2 size={15} aria-hidden="true" />删除
          </button>
          <button
            className="btn btn--primary"
            type="button"
            onClick={onLaunch}
            disabled={launching}
            data-testid={`env-launch-${env.env_id}`}
            title={env.running ? "把该环境的 TRAE 窗口带到前台" : "启动该环境的 TRAE 实例"}
          >
            <MonitorPlay size={15} aria-hidden="true" />
            {launching ? "处理中…" : env.running ? "聚焦" : "启动"}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 名称输入弹层状态（创建/重命名共用）。 */
interface NameDialogState {
  /** 重命名时的目标环境（创建时为空）。 */
  env?: EnvironmentListItemDto;
  name: string;
  error: string | null;
  busy: boolean;
}

/** P6-4 创建/重命名弹层：名称输入 → 提交 → 失败内联提示（错误码映射）。 */
function EnvironmentNameDialog({
  title,
  confirmLabel,
  value,
  onChange,
  onClose,
  onSubmit,
}: {
  title: string;
  confirmLabel: string;
  value: NameDialogState;
  onChange: (next: NameDialogState) => void;
  onClose: () => void;
  /** 提交动作（成功才关闭弹层；失败抛错由本组件转为内联提示）。 */
  onSubmit: (name: string) => Promise<void>;
}) {
  const submit = async () => {
    const name = value.name.trim();
    if (!name || value.busy) return;
    onChange({ ...value, busy: true, error: null });
    try {
      await onSubmit(name);
      onClose();
    } catch (reason: unknown) {
      onChange({ ...value, busy: false, error: safeUiErrorMessage(reason, "操作未完成，请稍后重试。") });
    }
  };

  return (
    <div className="switch-veil" role="presentation" onClick={(event) => {
      if (event.target === event.currentTarget && !value.busy) onClose();
    }}>
      <div
        className="switch-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        data-testid="env-name-dialog"
      >
        <div className="switch-dialog__title">{title}</div>
        <input
          className="env-name-input"
          type="text"
          value={value.name}
          maxLength={64}
          placeholder="环境名称（1-64 个字符）"
          autoFocus
          onChange={(event) => onChange({ ...value, name: event.target.value })}
          onKeyDown={(event) => {
            if (event.key === "Enter") void submit();
          }}
          data-testid="env-name-input"
        />
        {value.error && (
          <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
            <TriangleAlert size={18} aria-hidden="true" />
            <p>{value.error}</p>
          </div>
        )}
        <div className="switch-dialog__actions">
          <button className="btn" type="button" onClick={onClose} disabled={value.busy} data-testid="env-name-cancel">
            取消
          </button>
          <button
            className="btn btn--primary"
            type="button"
            onClick={() => void submit()}
            disabled={value.busy || value.name.trim().length === 0}
            data-testid="env-name-confirm"
          >
            {value.busy ? "处理中…" : confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

/** P6-4 环境登录账号弹层：账号选择 → 互换登录 → 回执（数量表述）。 */
function EnvironmentLoginDialog({
  env,
  onClose,
  onDone,
}: {
  env: EnvironmentListItemDto;
  onClose: () => void;
  onDone: () => Promise<void>;
}) {
  const [accounts, setAccounts] = useState<AccountProfileDto[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [receipt, setReceipt] = useState<{ accountName: string; seeded: boolean; transferredProjects: number; switchedSessions: number } | null>(null);

  useEffect(() => {
    let cancelled = false;
    invoke<ManagedAccountsViewDto>("get_managed_account_state")
      .then((view) => {
        // 后端 DTO 为 readonly 数组，拷贝为可变数组再入 state
        if (!cancelled) setAccounts([...view.saved_accounts]);
      })
      .catch((reason: unknown) => {
        if (!cancelled) {
          setAccounts([]);
          setError(safeUiErrorMessage(reason, "账号列表暂时不可读取，请稍后重试。"));
        }
      });
    return () => { cancelled = true; };
  }, []);

  const confirm = async () => {
    if (!selected || busy) return;
    const account = accounts?.find((candidate) => candidate.profile_id === selected);
    const accountName = account?.display_name ?? "所选账号";
    setBusy(true);
    setError(null);
    try {
      const result = await invoke<{
        seeded: boolean;
        transferred_projects: number;
        switched_sessions: number;
      }>("login_environment", { envId: env.env_id, profileId: selected });
      setReceipt({
        accountName,
        seeded: result.seeded,
        transferredProjects: result.transferred_projects,
        switchedSessions: result.switched_sessions,
      });
    } catch (reason: unknown) {
      setError(safeUiErrorMessage(reason, "环境账号切换未完成，请稍后重试。"));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="switch-veil" role="presentation" onClick={(event) => {
      if (event.target === event.currentTarget && !busy) onClose();
    }}>
      <div
        className="switch-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="登录账号到环境"
        data-testid="env-login-dialog"
      >
        <div className="switch-dialog__title">登录账号到「{env.name}」</div>

        {receipt ? (
          <>
            <div className="switch-dialog__done" role="status" data-testid="env-login-receipt">
              <CheckCircle2 size={18} aria-hidden="true" />
              <span>
                已登录账号 {receipt.accountName}
                {receipt.seeded
                  ? "；环境首次登录完成，可直接启动使用。"
                  : `；随行项目 ${receipt.transferredProjects} 个 · 会话 ${receipt.switchedSessions} 个`}
              </span>
            </div>
            <div className="switch-dialog__actions">
              <button
                className="btn btn--primary"
                type="button"
                onClick={() => void onDone()}
                data-testid="env-login-done"
              >
                完成
              </button>
            </div>
          </>
        ) : (
          <>
            <p className="switch-dialog__prepare">
              选择要登录到该环境的账号；环境内的对话记录会随登录归到所选账号名下。
            </p>
            {accounts === null ? (
              <p className="switch-dialog__prepare">正在读取账号列表…</p>
            ) : accounts.length === 0 ? (
              <div className="switch-dialog__notice">
                <TriangleAlert size={18} aria-hidden="true" />
                <p>还没有已保存的账号；请先在「账号」页添加账号。</p>
              </div>
            ) : (
              <ul className="env-login-list" data-testid="env-login-account-list">
                {accounts.map((account) => {
                  const isSelected = selected === account.profile_id;
                  return (
                    <li key={account.profile_id}>
                      <button
                        type="button"
                        className={`env-login-account${isSelected ? " env-login-account--selected" : ""}`}
                        onClick={() => setSelected(account.profile_id)}
                        role="radio"
                        aria-checked={isSelected}
                        data-testid={`env-login-account-${account.profile_id}`}
                      >
                        <span className="env-card__current-avatar" aria-hidden="true">
                          {account.display_name.slice(0, 1)}
                        </span>
                        <span className="env-login-account__name">{account.display_name}</span>
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}
            {error && (
              <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
                <TriangleAlert size={18} aria-hidden="true" />
                <p>{error}</p>
              </div>
            )}
            <div className="switch-dialog__actions">
              <button className="btn" type="button" onClick={onClose} disabled={busy} data-testid="env-login-cancel">
                取消
              </button>
              <button
                className="btn btn--primary"
                type="button"
                onClick={() => void confirm()}
                disabled={busy || selected === null}
                data-testid="env-login-confirm"
              >
                {busy ? "处理中…" : "登录"}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

/** P6-4 环境删除弹层：规模预览（ADR-0018 单次确认）→ 删除 → 关闭刷新。 */
function EnvironmentDeleteDialog({
  env,
  onClose,
  onDone,
}: {
  env: EnvironmentListItemDto;
  onClose: () => void;
  onDone: () => Promise<void>;
}) {
  const [preview, setPreview] = useState<{ status: string; project_count: number; session_count: number; size_bytes: number } | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    invoke<{ status: string; project_count: number; session_count: number; size_bytes: number }>(
      "get_environment_delete_preview",
      { envId: env.env_id },
    )
      .then((result) => {
        if (!cancelled) setPreview(result);
      })
      .catch((reason: unknown) => {
        if (!cancelled) setLoadError(safeUiErrorMessage(reason, "环境信息暂时不可读取，请稍后重试。"));
      });
    return () => { cancelled = true; };
  }, [env.env_id]);

  const confirm = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await invoke("delete_environment", { envId: env.env_id });
      await onDone();
    } catch (reason: unknown) {
      setError(safeUiErrorMessage(reason, "环境删除未完成，请稍后重试。"));
    } finally {
      setBusy(false);
    }
  };

  // 规模文案：有记录列明数量与体积；无记录轻描淡写；读不到规模时如实告知。
  const scaleText = (() => {
    if (!preview) return null;
    const size = formatBytes(preview.size_bytes);
    if (preview.status === "no_data") return `该环境从未启动，没有对话数据；将删除环境目录（约 ${size}）。`;
    if (preview.status === "read_failed") {
      return `环境数据规模暂时无法读取；删除将移除整个环境目录（约 ${size}），请确认其中没有需要保留的内容。`;
    }
    if (preview.session_count === 0) return `该环境没有对话记录；将删除环境目录（约 ${size}）。`;
    return `该环境内有 ${preview.project_count} 个项目、${preview.session_count} 个会话（约 ${size}），删除后无法恢复。`;
  })();

  return (
    <div className="switch-veil" role="presentation" onClick={(event) => {
      if (event.target === event.currentTarget && !busy) onClose();
    }}>
      <div
        className="switch-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="删除环境"
        data-testid="env-delete-dialog"
      >
        <div className="switch-dialog__title">删除环境「{env.name}」</div>

        {scaleText !== null ? (
          <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
            <TriangleAlert size={18} aria-hidden="true" />
            <div>
              <strong>删除后无法恢复</strong>
              <p>{scaleText}</p>
            </div>
          </div>
        ) : loadError ? (
          <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
            <TriangleAlert size={18} aria-hidden="true" />
            <p>{loadError}</p>
          </div>
        ) : (
          <p className="switch-dialog__prepare">正在读取环境数据规模…</p>
        )}

        {error && (
          <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
            <TriangleAlert size={18} aria-hidden="true" />
            <p>{error}</p>
          </div>
        )}

        <div className="switch-dialog__actions">
          <button className="btn" type="button" onClick={onClose} disabled={busy} data-testid="env-delete-cancel">
            取消
          </button>
          <button
            className="btn btn--danger"
            type="button"
            onClick={() => void confirm()}
            disabled={busy || preview === null}
            data-testid="env-delete-confirm"
          >
            {busy ? "删除中…" : "删除环境"}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 体积格式化（KB/MB/GB；与主库详情页同口径）。 */
function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0 MB";
  const units = ["KB", "MB", "GB", "TB"];
  let size = value / 1024;
  let unitIndex = 0;
  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex += 1;
  }
  return `${size >= 10 || unitIndex === 0 ? Math.round(size) : size.toFixed(1)} ${units[unitIndex]}`;
}

/** 收编弹层阶段（确认 → 运行 → 完成/失败）。 */
type IncorporatePhase =
  | { kind: "confirm" }
  | { kind: "running"; stage: string }
  | { kind: "done"; receipt: MasterIncorporateResultDto }
  | { kind: "failed"; message: string };

/** 收编四阶段进度（与后端 master-incorporate-progress 事件一一对应）。 */
const INCORPORATE_STAGES: readonly { stage: string; label: string }[] = [
  { stage: "closing", label: "关闭主库" },
  { stage: "backing_up", label: "创建备份" },
  { stage: "incorporating", label: "归入记录" },
  { stage: "restarting", label: "重启主库" },
];

/**
 * P5-5 一键收编弹层：确认（列明规模与备份提示，ADR-0018 单次确认）→
 * 四阶段进度（复用切号弹层步骤条样式）→ 完成回执（数量表述）。
 */
function IncorporateDialog({
  phase,
  currentName,
  staleAccounts,
  staleSessionTotal,
  staleProjectTotal,
  orphanProjectCount,
  onConfirm,
  onClose,
}: {
  phase: IncorporatePhase;
  currentName: string | null;
  staleAccounts: { user_id: string; account_name: string | null }[];
  staleSessionTotal: number;
  staleProjectTotal: number;
  orphanProjectCount: number;
  onConfirm: () => void;
  onClose: () => void;
}) {
  const allDone = phase.kind === "done" || (phase.kind === "running" && phase.stage === "done");
  const stageIndex = allDone
    ? INCORPORATE_STAGES.length
    : phase.kind === "running"
      ? Math.max(0, INCORPORATE_STAGES.findIndex((item) => item.stage === phase.stage))
      : 0;
  const progressPercent = allDone
    ? 100
    : Math.round(((stageIndex + 1) / INCORPORATE_STAGES.length) * 100);
  const targetName = currentName ?? "当前账号";

  return (
    <div className="switch-veil" role="presentation" onClick={(event) => {
      // 运行中不允许点遮罩中断（后端事务已开）；确认/完成/失败时可关闭。
      if (event.target === event.currentTarget && phase.kind !== "running") onClose();
    }}>
      <div
        className="switch-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="归入当前账号"
        data-testid="incorporate-dialog"
      >
        <div className="switch-dialog__title">归入当前账号</div>
        <div className="switch-dialog__target">
          <span className="switch-dialog__avatar" aria-hidden="true">{targetName.slice(0, 1)}</span>
          <div>
            <div className="switch-dialog__target-name" data-testid="incorporate-target">{targetName}</div>
            <div className="switch-dialog__target-meta">滞留记录全部归入 · 自动完成转移</div>
          </div>
        </div>

        {phase.kind === "confirm" ? (
          <>
            <div className="switch-dialog__notice">
              <TriangleAlert size={18} aria-hidden="true" />
              <div>
                <strong>将归入 {staleAccounts.length} 个账号的记录</strong>
                <p>
                  共 {staleSessionTotal} 个会话、{staleProjectTotal} 个项目将归入{targetName}；
                  主库会自动关闭并重启，归入前会创建数据备份。
                  {orphanProjectCount > 0 ? `另有 ${orphanProjectCount} 条无归属记录不会被归入。` : ""}
                </p>
              </div>
            </div>
            <ul className="switch-dialog__diff" data-testid="incorporate-account-list">
              {staleAccounts.map((account) => (
                <li key={account.user_id} className="switch-dialog__diff-item" title={`账号标识：${account.user_id}`}>
                  {account.account_name ?? "未登记账号"}
                </li>
              ))}
            </ul>
            <div className="switch-dialog__actions">
              <button className="btn" type="button" onClick={onClose} data-testid="incorporate-cancel">
                取消
              </button>
              <button className="btn btn--primary" type="button" onClick={onConfirm} data-testid="incorporate-confirm">
                <Combine size={15} aria-hidden="true" />确认归入
              </button>
            </div>
          </>
        ) : phase.kind === "failed" ? (
          <>
            <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
              <TriangleAlert size={18} aria-hidden="true" />
              <p>{phase.message}</p>
            </div>
            <div className="switch-dialog__actions">
              <button className="btn" type="button" onClick={onClose} data-testid="incorporate-failed-close">
                关闭
              </button>
            </div>
          </>
        ) : (
          <>
            {phase.kind === "done" && (
              <div className="switch-dialog__done" role="status">
                <CheckCircle2 size={18} aria-hidden="true" />
                <span>
                  已归入 {phase.receipt.merged_accounts} 个账号：保留项目 {phase.receipt.transferred_projects} 个 · 转移会话 {phase.receipt.switched_sessions} 个
                  {phase.receipt.relay_ledger_written ? "" : "（接力轨迹记录暂缺，不影响数据）"}
                </span>
              </div>
            )}
            <div className="switch-dialog__bar" aria-hidden="true">
              <div className="switch-dialog__bar-fill" style={{ width: `${progressPercent}%` }} />
            </div>
            <ol className="switch-dialog__steps">
              {INCORPORATE_STAGES.map((item, index) => {
                const state = phase.kind === "done" || index < stageIndex ? "done"
                  : phase.kind === "running" && index === stageIndex ? "run" : "idle";
                return (
                  <li key={item.stage} className={`switch-step switch-step--${state}`} data-stage={item.stage}>
                    <span className="switch-step__icon" aria-hidden="true">
                      {state === "done" ? <CheckCircle2 size={16} /> : <span className="switch-step__ring" />}
                    </span>
                    <span className="switch-step__label">{item.label}</span>
                  </li>
                );
              })}
            </ol>
            <div className="switch-dialog__actions">
              {phase.kind === "done" && (
                <button className="btn btn--primary" type="button" onClick={onClose} data-testid="incorporate-done">
                  完成
                </button>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
