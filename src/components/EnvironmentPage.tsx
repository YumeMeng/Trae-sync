import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { UserPlus,
  CheckCircle2,
  Combine,
  Database,
  HeartPulse,
  Layers,
  LogIn,
  MonitorPlay,
  Pencil,
  Plus,
  RefreshCw,
  Square,
  Trash2,
  TriangleAlert,
} from "lucide-react";
import type {
  EnvironmentProcessState,
  EnvironmentListItemDto,
  EnvironmentLoginReceiptDto,
  EnvironmentStateDto,
} from "../types/environment";
import type {
  MasterCheckupDto,
  MasterEmptyProjectCleanupDto,
  MasterIncorporateProgressEvent,
  MasterIncorporateResultDto,
  MasterSelfCheckDto,
} from "../types/masterLibrary";
import type { AccountProfileDto, ManagedAccountsViewDto } from "../types/account_switch";
import { safeUiErrorMessage } from "../utils/safeUiError";
// 统一通知适配层（P11-1 切片②）：操作结果类提示一律经 notify 发出。
import { notify } from "../platform/notify";
import { InstanceSlotBadge } from "./StatusBadges";
import { sectionStateOf } from "../platform/productRegistry";
import { ConfirmDialog } from "./ConfirmDialog";

interface EnvironmentPageProps {
  /** 页面可见时才读取环境状态，避免后台 IPC；隐藏时停止轮询。 */
  active: boolean;
  /** 打开主库详情页（P5-8a-2：主库卡入口）。 */
  onOpenMasterDetail?: () => void;
  /** 打开副库详情页（P11-5 收官：副环境卡「查看详情」入口，携带环境引用）。 */
  onOpenLibraryDetail?: (library: { id: string; displayName: string }) => void;
}

function environmentProcessState(
  environment: { readonly process_state?: EnvironmentProcessState; readonly running: boolean } | null,
): EnvironmentProcessState {
  if (!environment) return "unknown";
  return environment.process_state ?? (environment.running ? "running" : "not_running");
}

/**
 * 环境页 V2（P5-2 + P6-4，ADR-0024）：
 * 主库环境卡（默认环境，置顶不可删）+ 辅助环境列表（创建/登录账号/
 * 启动/关闭/重命名/删除）。主库卡承载生命周期与整合后的「主库体检」
 * 区块（P11-5：登录健康 + 库内容两组）。
 * P11-5 收官：副环境卡挂体检区块（有差异才显示；登录健康分组只显示
 * 注册表账号与待登录态）+「归入当前账号」+「查看详情」入口；收编弹层
 * 复用同一结构，进度事件按 env_id 过滤。
 */
export function EnvironmentPage({ active, onOpenMasterDetail, onOpenLibraryDetail }: EnvironmentPageProps) {
  // 产品事实源：区块三态来自注册表直查（P15-10 单产品塌缩，环境区块恒 ready）。
  const sectionState = sectionStateOf("environment");
  const [state, setState] = useState<EnvironmentStateDto | null>(null);
  const masterProcessState = environmentProcessState(state);
  // P5-5 主库体检：同读同刷；失败静默降级为 null（库内容分组显示降级提示）。
  const [checkup, setCheckup] = useState<MasterCheckupDto | null>(null);
  // P11-5 收官 副环境体检：按 env_id 存报告（仅手动刷新/进页时读取，
  // 5 秒轮询不触发——性能红线：轮询不新增数据库聚合调用）。
  const [secondaryCheckups, setSecondaryCheckups] = useState<
    Record<string, MasterCheckupDto | null>
  >({});
  // P8-5 G18 主库自检报告（P11-5 并入「主库体检」区块的登录健康分组；
  // active 时随体检自动读取，不再依赖手动触发）。
  const [selfCheck, setSelfCheck] = useState<MasterSelfCheckDto | null>(null);
  const [repairing, setRepairing] = useState(false);
  // P6-4 环境列表（含主库置顶项；null = 首次未读）。
  const [envList, setEnvList] = useState<EnvironmentListItemDto[] | null>(null);
  // 收编弹层：null = 关闭；确认 → 运行（四阶段进度）→ 完成回执 / 失败提示。
  // P11-5 收官：带目标库引用（主库/副环境共用弹层与编排），进度事件按
  // env_id 过滤，多库并行收编时只推进自己目标的事件。
  const [incorporating, setIncorporating] = useState<IncorporateSession | null>(null);
  // 清理空项目：弹层开关 + 执行中（后端编排：关实例 → 备份 → 删除 → 重启）。
  const [cleanupDialogOpen, setCleanupDialogOpen] = useState(false);
  const [cleaningUp, setCleaningUp] = useState(false);
  // P16-T4 账号捕获：从 Trae 客户端登录态捕获新账号入池（捕获中/结果提示）。
  const [capturingAccount, setCapturingAccount] = useState(false);
  const [captureNotice, setCaptureNotice] = useState<{ kind: "ok" | "error"; text: string } | null>(null);
  // P6-4 辅助环境生命周期弹层（同一时刻至多一个）。
  const [createDialog, setCreateDialog] = useState<NameDialogState | null>(null);
  const [renameDialog, setRenameDialog] = useState<NameDialogState | null>(null);
  const [loginDialogEnv, setLoginDialogEnv] = useState<EnvironmentListItemDto | null>(null);
  const [deleteDialogEnv, setDeleteDialogEnv] = useState<EnvironmentListItemDto | null>(null);
  // P11-5 关闭实例：确认弹层目标（主库用固定名）+ 执行中环境。
  const [closeDialogEnv, setCloseDialogEnv] = useState<{ envId: string; name: string } | null>(
    null,
  );
  const [closingEnvId, setClosingEnvId] = useState<string | null>(null);
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
    // P11-5 体检整合：体检与环境列表同读同刷，自检报告（登录健康）一并
    // 自动读取；两组各自失败降级为 null，互不阻塞。主库统计仍归详情页。
    const [checkupReport, selfCheckReport, list] = await Promise.all([
      invoke<MasterCheckupDto>("get_master_checkup").catch(() => null),
      invoke<MasterSelfCheckDto>("get_master_self_check").catch(() => null),
      // 手动刷新带体积（主库三件套 + 副环境整目录口径）。
      invoke<EnvironmentListItemDto[]>("list_environments", { includeSize: true }).catch(
        () => null,
      ),
    ]);
    setCheckup(checkupReport);
    setSelfCheck(selfCheckReport);
    if (list) setEnvList((previous) => mergeListSizes(previous, list));
    // P11-5 收官：副环境体检随列表并行拉取（每环境只读聚合；与主库体检
    // 同时机——进页/手动刷新/动作后，5 秒轮询不进入本函数）。
    if (list) {
      const secondaries = list.filter((item) => !item.is_master);
      const reports = await Promise.all(
        secondaries.map(async (item) => [
          item.env_id,
          await invoke<MasterCheckupDto>("get_master_checkup", {
            libraryId: item.env_id,
          }).catch(() => null),
        ] as const),
      );
      setSecondaryCheckups(Object.fromEntries(reports));
    }
  }, [mergeListSizes]);

  /** G18：只在用户明确点击后纠正工具缓存，随后整组重读（含登录健康报告）。 */
  const handleRepair = useCallback(async () => {
    setRepairing(true);
    try {
      await invoke("repair_master_current_account");
      await load();
      notify.success("已按实际登录账号纠正主库记录。");
    } catch (reason: unknown) {
      notify.error(safeUiErrorMessage(reason, "主库记录未能纠正，请稍后重试。"));
    } finally {
      setRepairing(false);
    }
  }, [load]);

  useEffect(() => {
    if (!active || sectionState !== "ready") return;
    let cancelled = false;
    void load().catch((reason: unknown) => {
      if (!cancelled) notify.error(safeUiErrorMessage(reason, "环境状态暂时不可读取，请稍后重试。"));
    });
    return () => { cancelled = true; };
  }, [active, sectionState, load]);

  // 运行态轮询（与账号页实例徽章同模式）：主库可能被用户在 TRAE 内直接关闭/启动，
  // 页面停留期间每 5 秒校准一次；隐藏即停止。轮询不带体积（沿用最近一次的值）。
  useEffect(() => {
    if (!active || sectionState !== "ready") return;
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
  }, [active, sectionState, mergeListSizes]);

  /** 统一的动作后反馈：成功消息走全局通知；失败原因走全局错误通知。 */
  const runEnvAction = useCallback(
    async (action: () => Promise<void>, successMessage: string) => {
      try {
        await action();
        await load();
        notify.success(successMessage);
      } catch (reason: unknown) {
        notify.error(safeUiErrorMessage(reason, "环境操作未能完成，请稍后重试。"));
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

  // P11-5：关闭环境实例（主库/副环境通用）。成功消息按关闭回执定制：
  // 优雅关闭 / 强制兜底 / 本就未运行（幂等，info 提示）各自不同表述；
  // 完成后立即刷新一次运行态（5 秒轮询会继续校准）。
  const handleCloseEnv = useCallback(
    async (target: { envId: string; name: string }) => {
      if (closingEnvId) return;
      setClosingEnvId(target.envId);
      try {
        const receipt = await invoke<{ outcome: string }>("close_environment_instance", {
          envId: target.envId,
        });
        await load();
        if (receipt.outcome === "not_running") {
          notify.info(`「${target.name}」当前没有运行中的实例。`);
        } else if (receipt.outcome === "force_closed") {
          notify.success(`「${target.name}」未能自动退出，已强制结束进程。`);
        } else {
          notify.success(`已关闭「${target.name}」的 TRAE 实例。`);
        }
      } catch (reason: unknown) {
        notify.error(safeUiErrorMessage(reason, "环境操作未能完成，请稍后重试。"));
      } finally {
        setClosingEnvId(null);
      }
    },
    [closingEnvId, load],
  );

  const currentName = state?.current_account_name ?? null;
  const secondaryEnvs = envList === null ? null : envList.filter((env) => !env.is_master);

  // ===== P5-5 体检区块数据派生 + 一键收编 =====

  // 主库体检派生（滞留账号 / 空项目诊断 / 收编目标规模）。
  const masterView = deriveCheckupView(checkup);
  const staleAccounts = masterView.staleAccounts;
  const emptyShellAccounts = masterView.emptyShellAccounts;
  const staleSessionTotal = masterView.staleSessionTotal;
  const staleProjectTotal = masterView.staleProjectTotal;
  const emptyProjectTotal = masterView.emptyProjectTotal;
  // 只要全库存在空项目（哪怕全在当前账号名下），体检区块与清理入口就要出现。
  const hasCheckupNotice = masterView.hasNotice;
  // 登录数据损坏（G18 读校验失败且摘要提示损坏）：需要显式引导重新登录。
  const readCheck = selfCheck?.checks.find((check) => check.key === "read");
  const loginCorrupted = readCheck?.status === "failed" && readCheck.summary.includes("损坏");

  /** 确认收编 → 后端四步编排（关实例 → 备份 → 归属改写 → 重启）。 */
  const startIncorporate = useCallback(async (target: IncorporateTarget) => {
    setIncorporating({ target, phase: { kind: "running", stage: "closing" } });
    let unlisten: (() => void) | null = null;
    try {
      // 进度事件（done 只推满进度条；回执以 invoke 返回为准）。
      // P11-5 收官：载荷带 env_id，弹层只推进自己目标的事件。
      unlisten = await listen<MasterIncorporateProgressEvent>(
        "master-incorporate-progress",
        (event) => {
          if (event.payload.env_id !== target.envId) return;
          setIncorporating((current) =>
            current?.target.envId === target.envId && current.phase.kind === "running"
              ? { target, phase: { kind: "running", stage: event.payload.stage } }
              : current,
          );
        },
      );
      const receipt = await invoke<MasterIncorporateResultDto>(
        "incorporate_master_records",
        target.isMaster ? undefined : { libraryId: target.envId },
      );
      // 收编完成即刷新体检与统计（滞留行清零，区块自然消失）。
      let refreshFailed = false;
      try {
        await load();
      } catch {
        // 收编回执已确认写入，状态刷新失败不能覆盖已完成结果。
        refreshFailed = true;
      }
      setIncorporating({ target, phase: { kind: "done", receipt, refreshFailed } });
    } catch (reason: unknown) {
      setIncorporating({
        target,
        phase: {
          kind: "failed",
          message: safeUiErrorMessage(
            reason,
            target.isMaster
              ? "归入未完成，主库数据保持操作前状态；请稍后重试。"
              : "归入未完成，环境数据保持操作前状态；请稍后重试。",
          ),
        },
      });
    } finally {
      unlisten?.();
    }
  }, [load]);

  /** 收编弹层关闭（运行中不允许——后端事务已开）。 */
  const closeIncorporate = useCallback(() => {
    setIncorporating((current) => (current?.phase.kind === "running" ? current : null));
  }, []);

  /** P16-T4：从客户端捕获账号（读登录态 → 在线复核 → 铸造入池）。 */
  const handleCaptureAccount = useCallback(async () => {
    if (capturingAccount) return;
    setCapturingAccount(true);
    setCaptureNotice(null);
    try {
      const receipt = await invoke<{ profile_id: string; account_id: string; screen_name: string }>(
        "capture_trae_cn_account",
      );
      setCaptureNotice({
        kind: "ok",
        text: `已捕获账号「${receipt.screen_name}」入池（${receipt.profile_id}），切号/签到/历史能力即刻可用。`,
      });
      try {
        await load();
      } catch {
        // 捕获已成功，刷新失败不影响结果。
      }
    } catch (reason: unknown) {
      const raw = typeof reason === "string" ? reason : String(reason);
      setCaptureNotice({
        kind: "error",
        text: raw.startsWith("capture_account_already_registered")
          ? "该账号已在账号池中，无需重复捕获。"
          : raw.startsWith("capture_blob_missing")
            ? "未在 Trae 客户端检测到新的登录态：请先在客户端登录要捕获的账号，再点击捕获。"
            : raw.startsWith("capture_identity_review_failed")
              ? "在线复核身份未通过，未做任何改动；请稍后重试。"
              : `捕获未完成：${raw}（数据保持操作前状态）。`,
      });
    } finally {
      setCapturingAccount(false);
    }
  }, [capturingAccount, load]);

  /** 确认清理空项目 → 后端编排（关实例 → 备份 → 删除 → 重启）。 */
  const handleCleanupEmptyProjects = useCallback(async () => {
    if (cleaningUp) return;
    setCleaningUp(true);
    try {
      const receipt = await invoke<MasterEmptyProjectCleanupDto>("cleanup_master_empty_projects");
      setCleanupDialogOpen(false);
      // 清理完成即刷新体检（空项目清零，诊断行自然消失）。
      let refreshFailed = false;
      try {
        await load();
      } catch {
        // 删除回执已确认提交，不能把后续读取失败改报为清理失败。
        refreshFailed = true;
      }
      // 重启失败不回滚删除：删除已提交且已备份，据实提示而非"清理未完成"。
      if (receipt.deleted_projects > 0) {
        notify.success(
          `${receipt.relaunch_outcome === "failed"
            ? `已删除 ${receipt.deleted_projects} 个空项目记录（已自动备份）；主库 TRAE 未能自动启动，可从环境页重新打开。`
            : `已删除 ${receipt.deleted_projects} 个空项目记录（已自动备份）。`}${refreshFailed ? "环境状态刷新失败，请手动刷新。" : ""}`,
        );
      } else {
        notify.info(
          `没有可清理的空项目记录。${refreshFailed ? "环境状态刷新失败，请手动刷新。" : ""}`,
        );
      }
    } catch (reason: unknown) {
      // 失败关闭弹层，错误走全局通知（ConfirmDialog 无内嵌错误插槽）。
      setCleanupDialogOpen(false);
      notify.error(safeUiErrorMessage(reason, "清理未完成，主库数据保持操作前状态；请稍后重试。"));
    } finally {
      setCleaningUp(false);
    }
  }, [cleaningUp, load]);

  // 区块能力三态（P11-6，ADR-0032 决策 2）：单产品塌缩后环境区块恒 ready，
  // 非 ready 说明卡随 PRODUCT_SECTION_STATES 塌缩移除（P15-10）。
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
                <InstanceSlotBadge
                  running={masterProcessState === "running"}
                  processState={masterProcessState}
                  loginState={state?.login_state}
                />
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

          <p className="env-card__hint">
            {state?.current_profile_id
              ? "全部对话记录都在主库内共享；在「账号」页切换账号，全部记录都会保留。"
              : "主库尚未登录账号：首次启动后请在 TRAE 窗口内登录一次，此后切换账号全自动完成。"}
          </p>

          {/* P11-5 主库体检整合区块（原 G18 自检 + P5-5 体检两个入口合并）：
              登录健康分组（读校验/一致性/可切换/深度四项）+ 库内容分组
              （滞留/空项目/孤儿行诊断与动作）。active 时自动读取，两组
              各自失败降级为提示，互不阻塞；两组都不可读时区块整体静默。 */}
          {(selfCheck || checkup) && (
            <div className="env-checkup" data-testid="env-checkup">
              <div className="env-checkup__head">
                <HeartPulse size={16} aria-hidden="true" />
                <div>主库体检</div>
              </div>

              {/* 登录健康分组：现 G18 自检四项（user_id 等技术细节不进主视野）。 */}
              <div className="env-checkup__group" data-testid="env-checkup-login">
                <div className="env-checkup__group-title">
                  登录健康{selfCheck ? `：${selfCheckLevelLabel(selfCheck.level)}` : ""}
                </div>
                {selfCheck ? (
                  <>
                    {selfCheck.current_account_name !== selfCheck.observed_account_name &&
                      selfCheck.observed_account_name !== null && (
                        <p className="env-self-check__note">
                          实际登录：{selfCheck.observed_account_name}；工具记录：
                          {selfCheck.current_account_name ?? "暂无"}
                        </p>
                      )}
                    {loginCorrupted && (
                      <p className="env-self-check__note">登录数据已损坏，需重新登录。</p>
                    )}
                    <ul className="env-self-check__list">
                      {selfCheck.checks.map((check) => (
                        <li
                          key={check.key}
                          className={`env-self-check__item env-self-check__item--${check.status}`}
                        >
                          <span className="env-self-check__item-name">{selfCheckKeyLabel(check.key)}</span>
                          <span className="env-self-check__item-status">{selfCheckStatusLabel(check.status)}</span>
                          <span className="env-self-check__item-summary">{check.summary}</span>
                        </li>
                      ))}
                    </ul>
                    <details className="env-self-check__details" data-testid="env-self-check-deep">
                      <summary>查看深度检查说明</summary>
                      <p>
                        {selfCheck.checks.find((check) => check.key === "deep")?.summary ??
                          "暂无深度检查结果。"}
                      </p>
                      <p>需要核对具体记录时，请在主库详情的库信息中展开备份对比。</p>
                    </details>
                    {selfCheck.can_repair && (
                      <button
                        className="btn btn--primary env-self-check__repair"
                        type="button"
                        onClick={() => void handleRepair()}
                        disabled={repairing}
                        data-testid="env-self-check-repair"
                      >
                        {repairing ? "纠正中…" : "纠正为实际登录账号"}
                      </button>
                    )}
                  </>
                ) : (
                  <p className="env-checkup__note">登录健康暂时无法读取。</p>
                )}
              </div>

              {/* 库内容分组：现 P5-5 体检主体（滞留/空项目诊断 + 收编/清理动作）。 */}
              <div className="env-checkup__group" data-testid="env-checkup-content">
                <div className="env-checkup__group-title">库内容</div>
                {checkup?.status === "ready" ? (
                  hasCheckupNotice ? (
                    <>
                      <div className="env-checkup__notice">
                        {staleAccounts.length > 0 && (
                          <div>
                            有 {staleAccounts.length} 个账号的 {staleSessionTotal} 个会话在主库中，未随当前账号展示
                          </div>
                        )}
                        {emptyProjectTotal > 0 && (
                          <div>另有 {emptyProjectTotal} 个空项目记录未进入归入范围</div>
                        )}
                      </div>
                      {staleAccounts.length > 0 && <ul className="env-checkup__list">
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
                      </ul>}
                      {emptyShellAccounts.length > 0 && (
                        <ul className="env-checkup__list" data-testid="env-empty-shell-list">
                          {emptyShellAccounts.map((account) => (
                            <li key={`empty-${account.user_id}`} title={`账号标识：${account.user_id}`}>
                              <span className="env-checkup__name">
                                {account.account_name ?? "未登记账号"}
                              </span>
                              <span className="env-checkup__meta">
                                {account.empty_project_count} 个空项目
                              </span>
                            </li>
                          ))}
                        </ul>
                      )}
                      {/* 空项目一键清理（破坏性，弹层单次确认后执行）。
                          删除范围为全库空项目，执行前自动备份。 */}
                      {emptyProjectTotal > 0 && (
                        <button
                          className="btn env-checkup__action"
                          type="button"
                          onClick={() => setCleanupDialogOpen(true)}
                          data-testid="env-cleanup-empty-projects-entry"
                        >
                          <Trash2 size={15} aria-hidden="true" />清理空项目
                        </button>
                      )}
                      {checkup.orphan_project_count > 0 && (
                        <p className="env-checkup__note">
                          另有 {checkup.orphan_project_count} 条无归属记录不会被归入。
                        </p>
                      )}
                      {staleAccounts.length > 0 && <button
                        className="btn btn--primary env-checkup__action"
                        type="button"
                        onClick={() =>
                          setIncorporating({
                            target: {
                              envId: "master",
                              envName: "主库",
                              accountName: currentName,
                              isMaster: true,
                            },
                            phase: { kind: "confirm" },
                          })
                        }
                        data-testid="env-incorporate-entry"
                      >
                        <Combine size={15} aria-hidden="true" />一键归入当前账号
                      </button>}
                    </>
                  ) : (
                    <p className="env-checkup__note" data-testid="env-checkup-content-ok">
                      暂未发现滞留记录，账号记录分布正常。
                    </p>
                  )
                ) : (
                  <p className="env-checkup__note">
                    {checkup ? checkupStatusNote(checkup.status) : "库内容暂时无法读取。"}
                  </p>
                )}
              </div>
            </div>
          )}
          {/* P16-T4 账号捕获：从客户端登录态捕获新账号入池（非破坏性，失败零副作用）。
              独立于体检区块常驻展示；捕获不依赖任何体检/诊断状态。 */}
          <div className="env-checkup__group" data-testid="env-capture-account-group">
            <div className="env-checkup__group-title">账号捕获</div>
            <button
              className="btn env-checkup__action"
              type="button"
              onClick={handleCaptureAccount}
              disabled={capturingAccount}
              data-testid="env-capture-account-entry"
            >
              <UserPlus size={15} aria-hidden="true" />
              {capturingAccount ? "正在捕获…" : "从客户端捕获账号"}
            </button>
            {captureNotice && (
              <p
                className="env-checkup__note"
                data-testid="env-capture-account-notice"
                style={{ color: captureNotice.kind === "ok" ? undefined : "#c0392b" }}
              >
                {captureNotice.text}
              </p>
            )}
          </div>

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
              {/* P11-5：关闭实例（仅运行中显示；确认弹层防误触）。 */}
              {masterProcessState === "running" && (
                <button
                  className="btn"
                  type="button"
                  onClick={() => setCloseDialogEnv({ envId: "master", name: "主库" })}
                  disabled={closingEnvId !== null}
                  data-testid="env-master-close"
                  title="关闭主库 TRAE 实例"
                >
                  <Square size={15} aria-hidden="true" />关闭
                </button>
              )}
              <button
                className="btn btn--primary"
                type="button"
                onClick={() => void handleLaunch()}
                disabled={launching || masterProcessState === "unknown"}
                data-testid="env-master-launch"
                title={masterProcessState === "unknown"
                  ? "无法确认主库 TRAE 是否正在运行，实例操作暂不可用"
                  : masterProcessState === "running"
                    ? "把主库 TRAE 窗口带到前台"
                    : "启动主库 TRAE 实例"}
              >
                <MonitorPlay size={15} aria-hidden="true" />
                {launching ? "处理中…" : masterProcessState === "unknown" ? "状态未知" : masterProcessState === "running" ? "聚焦主库" : "启动主库"}
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
                  // P11-5 收官：副环境体检报告（有差异才渲染区块）。
                  checkup={secondaryCheckups[env.env_id] ?? null}
                  launching={launchingEnvId === env.env_id}
                  closing={closingEnvId === env.env_id}
                  onLaunch={() => void handleLaunchEnv(env)}
                  onClose={() => setCloseDialogEnv({ envId: env.env_id, name: env.name })}
                  onLogin={() => setLoginDialogEnv(env)}
                  onRename={() => setRenameDialog({ env, name: env.name, error: null, busy: false })}
                  onDelete={() => setDeleteDialogEnv(env)}
                  onOpenDetail={
                    onOpenLibraryDetail
                      ? () => onOpenLibraryDetail({ id: env.env_id, displayName: env.name })
                      : undefined
                  }
                  onIncorporate={(targetAccountName) =>
                    setIncorporating({
                      target: {
                        envId: env.env_id,
                        envName: env.name,
                        accountName: targetAccountName,
                        isMaster: false,
                      },
                      phase: { kind: "confirm" },
                    })
                  }
                />
              ))}
            </div>
          )}
        </div>
      </div>

      {/* P5-5 收编弹层（P11-5 收官：主库/副环境共用）：确认 → 四阶段进度 → 完成回执 / 失败提示 */}
      {incorporating && (
        <IncorporateDialog
          target={incorporating.target}
          phase={incorporating.phase}
          staleAccounts={
            incorporating.target.isMaster
              ? staleAccounts
              : deriveCheckupView(secondaryCheckups[incorporating.target.envId] ?? null).staleAccounts
          }
          staleSessionTotal={
            incorporating.target.isMaster
              ? staleSessionTotal
              : deriveCheckupView(secondaryCheckups[incorporating.target.envId] ?? null).staleSessionTotal
          }
          staleProjectTotal={
            incorporating.target.isMaster
              ? staleProjectTotal
              : deriveCheckupView(secondaryCheckups[incorporating.target.envId] ?? null).staleProjectTotal
          }
          orphanProjectCount={
            incorporating.target.isMaster
              ? checkup?.orphan_project_count ?? 0
              : secondaryCheckups[incorporating.target.envId]?.orphan_project_count ?? 0
          }
          onConfirm={() => void startIncorporate(incorporating.target)}
          onClose={closeIncorporate}
        />
      )}

      {/* P5-5 清理空项目确认弹层（破坏性，ADR-0018 单次确认）。
          数量为全库空项目总数，与后端删除范围一致。 */}
      {cleanupDialogOpen && (
        <ConfirmDialog
          title="清理空项目"
          lines={[
            `将删除 ${emptyProjectTotal} 个空项目记录，删除前会自动备份。`,
            "空项目记录不含任何对话，删除不影响对话历史。",
            "无归属记录不会被清理。",
          ]}
          confirmLabel="删除"
          confirmIcon={<Trash2 size={15} aria-hidden="true" />}
          busyLabel="清理中…"
          busy={cleaningUp}
          danger
          onCancel={() => setCleanupDialogOpen(false)}
          onConfirm={() => void handleCleanupEmptyProjects()}
          testId="env-cleanup-empty-projects-dialog"
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
            notify.success(`已创建环境「${name.trim()}」。`);
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
            notify.success(`已重命名为「${name.trim()}」。`);
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
            try {
              await load();
            } catch {
              notify.warning("账号已切换，但环境状态刷新失败，请手动刷新。");
            }
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
            notify.success(`已删除环境「${deleteDialogEnv.name}」。`);
          }}
        />
      )}

      {/* P11-5 关闭实例确认弹层（主库/副环境通用）：确认后调用关闭命令。 */}
      {closeDialogEnv && (
        <ConfirmDialog
          title={`关闭「${closeDialogEnv.name}」`}
          lines={[
            `将关闭「${closeDialogEnv.name}」正在运行的 TRAE 实例。`,
            "实例内未保存的内容可能丢失，请确认已保存所需工作。",
          ]}
          confirmLabel="关闭实例"
          confirmIcon={<Square size={15} aria-hidden="true" />}
          busyLabel="正在关闭…"
          busy={closingEnvId !== null}
          onCancel={() => setCloseDialogEnv(null)}
          onConfirm={() => {
            const target = closeDialogEnv;
            setCloseDialogEnv(null);
            void handleCloseEnv(target);
          }}
          testId="env-close-dialog"
        />
      )}
    </section>
  );
}

/**
 * P11-5 主库体检整合面板的库内容状态说明（checkup 非 ready 时的降级文案；
 * 按状态给用户能直接理解的原因，不用底层码）。
 * P11-6：not_applicable = 只读库（Trae CN 数据根）不适用体检——按 absent
 * 语义说明而非「读取失败」（后端 master_checkup_not_applicable 同口径）。
 */
function checkupStatusNote(status: MasterCheckupDto["status"]): string {
  switch (status) {
    case "no_current_account":
      return "尚未确认主库当前登录账号，无法统计库内容。";
    case "no_master_data":
      return "主库尚未产生对话数据。";
    case "read_failed":
      return "库内容暂时无法读取。";
    case "not_applicable":
      return "该产品无此能力：只读浏览的库不适用体检。";
    default:
      return "库内容暂时无法读取。";
  }
}

function selfCheckLevelLabel(level: MasterSelfCheckDto["level"]): string {
  switch (level) {
    case "healthy":
      return "健康";
    case "self_healable":
      return "可自愈";
    case "needs_manual":
      return "需人工处理";
  }
}

function selfCheckKeyLabel(key: MasterSelfCheckDto["checks"][number]["key"]): string {
  switch (key) {
    case "read":
      return "读校验";
    case "consistency":
      return "一致性";
    case "switchability":
      return "可切换";
    case "deep":
      return "深度检查";
  }
}

function selfCheckStatusLabel(status: MasterSelfCheckDto["checks"][number]["status"]): string {
  switch (status) {
    case "passed":
      return "正常";
    case "attention":
      return "需关注";
    case "failed":
      return "需人工";
    case "blocked":
      return "暂不可用";
  }
}

/**
 * P6-4 辅助环境卡：名称 + 当前账号 + 运行态 + 体积 + 操作组。
 * P11-5 收官：挂副环境体检区块（有差异才显示，复用 env-checkup 结构；
 * 登录健康分组只显示注册表账号与待登录态，不出自检四项——副环境登录
 * 由工具托管，无实测语义）+「归入当前账号」+「查看详情」入口。
 */
function SecondaryEnvCard({
  env,
  checkup,
  launching,
  closing,
  onLaunch,
  onClose,
  onLogin,
  onRename,
  onDelete,
  onOpenDetail,
  onIncorporate,
}: {
  env: EnvironmentListItemDto;
  /** 副环境体检报告（进页/手动刷新时读取；无差异时不渲染区块）。 */
  checkup: MasterCheckupDto | null;
  launching: boolean;
  /** P11-5：该环境的关闭动作执行中（按钮禁用与文案切换）。 */
  closing: boolean;
  onLaunch: () => void;
  onClose: () => void;
  onLogin: () => void;
  onRename: () => void;
  onDelete: () => void;
  /** P11-5 收官：打开副库详情页（详情按钮）。 */
  onOpenDetail?: () => void;
  /** P11-5 收官：发起归入（参数 = 注册表当前账号显示名，弹层文案用）。 */
  onIncorporate: (targetAccountName: string | null) => void;
}) {
  const view = deriveCheckupView(checkup);
  const processState = environmentProcessState(env);
  // 有差异才显示体检区块（与主库口径一致：滞留/空项目/孤儿行任一存在；
  // 体检读取失败或无差异时静默——卡片本身已有登录入口与运行态）。
  const showCheckup =
    checkup?.status === "ready" &&
    (view.hasNotice || checkup.orphan_project_count > 0);

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
            <InstanceSlotBadge
              running={processState === "running"}
              processState={processState}
              loginState={env.login_state}
            />
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

      {/* P11-5 收官 副环境体检区块：有差异才显示；结构与主库体检同构。 */}
      {showCheckup && (
        <div className="env-checkup" data-testid={`env-checkup-${env.env_id}`}>
          <div className="env-checkup__head">
            <HeartPulse size={16} aria-hidden="true" />
            <div>{env.name}体检</div>
          </div>

          {/* 登录健康分组：副环境无实测语义，只显示注册表账号与待登录态。 */}
          <div className="env-checkup__group" data-testid={`env-checkup-login-${env.env_id}`}>
            <div className="env-checkup__group-title">登录健康</div>
            <p className="env-checkup__note">
              {env.current_account_name
                ? `当前登录：${env.current_account_name}。`
                : "尚未登录账号，请先登录后再使用该环境。"}
            </p>
          </div>

          {/* 库内容分组：滞留/空项目诊断 + 归入动作（语义与主库一致）。 */}
          <div className="env-checkup__group" data-testid={`env-checkup-content-${env.env_id}`}>
            <div className="env-checkup__group-title">库内容</div>
            {view.staleAccounts.length > 0 && (
              <div className="env-checkup__notice">
                <div>
                  有 {view.staleAccounts.length} 个账号的 {view.staleSessionTotal} 个会话在此环境中，未随当前账号展示
                </div>
              </div>
            )}
            {view.emptyProjectTotal > 0 && (
              <div className="env-checkup__notice">
                <div>另有 {view.emptyProjectTotal} 个空项目记录未进入归入范围</div>
              </div>
            )}
            {view.staleAccounts.length > 0 && (
              <ul className="env-checkup__list">
                {view.staleAccounts.map((account) => (
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
            )}
            {checkup.orphan_project_count > 0 && (
              <p className="env-checkup__note">
                另有 {checkup.orphan_project_count} 条无归属记录不会被归入。
              </p>
            )}
            {view.staleAccounts.length > 0 && (
              <button
                className="btn btn--primary env-checkup__action"
                type="button"
                onClick={() => onIncorporate(env.current_account_name ?? null)}
                data-testid={`env-incorporate-${env.env_id}`}
              >
                <Combine size={15} aria-hidden="true" />归入当前账号
              </button>
            )}
          </div>
        </div>
      )}

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
          {onOpenDetail && (
            <button
              className="btn"
              type="button"
              onClick={onOpenDetail}
              data-testid={`env-detail-${env.env_id}`}
              title={`查看「${env.name}」详情（对话记录、统计与备份）`}
            >
              <Database size={15} aria-hidden="true" />查看详情
            </button>
          )}
          <button
            className="btn"
            type="button"
            onClick={onLogin}
            disabled={processState === "unknown"}
            data-testid={`env-login-${env.env_id}`}
            title={processState === "unknown" ? "无法确认该环境是否正在运行，暂不能登录" : "选择一个账号登录该环境"}
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
            disabled={processState === "unknown"}
            data-testid={`env-delete-${env.env_id}`}
            title={processState === "unknown" ? "无法确认该环境是否正在运行，暂不能删除" : "删除该环境（删除前会列出数据规模）"}
          >
            <Trash2 size={15} aria-hidden="true" />删除
          </button>
          {/* P11-5：关闭实例（仅运行中显示，与启动/聚焦同区）。 */}
          {processState === "running" && (
            <button
              className="btn"
              type="button"
              onClick={onClose}
              disabled={closing}
              data-testid={`env-close-${env.env_id}`}
              title="关闭该环境的 TRAE 实例"
            >
              <Square size={15} aria-hidden="true" />
              {closing ? "关闭中…" : "关闭"}
            </button>
          )}
          <button
            className="btn btn--primary"
            type="button"
            onClick={onLaunch}
            disabled={launching || processState === "unknown"}
            data-testid={`env-launch-${env.env_id}`}
            title={processState === "unknown"
              ? "无法确认该环境是否正在运行，实例操作暂不可用"
              : processState === "running"
                ? "把该环境的 TRAE 窗口带到前台"
                : "启动该环境的 TRAE 实例"}
          >
            <MonitorPlay size={15} aria-hidden="true" />
            {launching ? "处理中…" : processState === "unknown" ? "状态未知" : processState === "running" ? "聚焦" : "启动"}
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
  const [receipt, setReceipt] = useState<{
    accountName: string;
    seeded: boolean;
    transferredProjects: number;
    switchedSessions: number;
    currentProfileSaved: boolean;
  } | null>(null);

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
      const result = await invoke<EnvironmentLoginReceiptDto>("login_environment", {
        envId: env.env_id,
        profileId: selected,
      });
      setReceipt({
        accountName,
        seeded: result.seeded,
        transferredProjects: result.transferred_projects,
        switchedSessions: result.switched_sessions,
        currentProfileSaved: result.current_profile_saved,
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
            {!receipt.currentProfileSaved && (
              <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
                <TriangleAlert size={18} aria-hidden="true" />
                <p>账号已切换，但应用未能记住当前账号；请刷新环境页，问题持续时重启应用。</p>
              </div>
            )}
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
    <ConfirmDialog
      title={`删除环境「${env.name}」`}
      confirmLabel="删除环境"
      busyLabel="删除中…"
      busy={busy}
      danger
      confirmDisabled={preview === null}
      onCancel={onClose}
      onConfirm={() => void confirm()}
      testId="env-delete-dialog"
    >
      {/* 规模预览：有记录列明数量与体积；无记录轻描淡写；读不到时如实告知。 */}
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
    </ConfirmDialog>
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
  | { kind: "done"; receipt: MasterIncorporateResultDto; refreshFailed: boolean }
  | { kind: "failed"; message: string };

/**
 * P11-5 收官 收编目标库：主库/副环境共用弹层与编排。
 * isMaster 决定进度文案（关闭/重启主库 vs 关闭/重启环境）与 invoke 参数。
 */
interface IncorporateTarget {
  /** 库实例 id：主库 "master"，副环境为 env_id。 */
  envId: string;
  /** 库显示名（进度文案与弹层说明）。 */
  envName: string;
  /** 归入目标账号显示名（弹层头像；null = 暂不可读，降级「当前账号」）。 */
  accountName: string | null;
  isMaster: boolean;
}

/** 一次收编会话：目标库 + 当前阶段。 */
interface IncorporateSession {
  target: IncorporateTarget;
  phase: IncorporatePhase;
}

/**
 * 体检报告派生（P11-5 收官抽出）：滞留账号 / 空项目诊断 / 收编目标规模。
 * 主库卡与副环境卡共用同一口径（原主库卡内联逻辑上移）。
 */
function deriveCheckupView(report: MasterCheckupDto | null) {
  const ready = report?.status === "ready";
  const accounts = ready ? report.accounts : [];
  // 滞留账号（非当前且存在正常会话的分布行）——收编目标。
  const staleAccounts = accounts.filter((account) => !account.current && account.session_count > 0);
  // 空项目只作诊断展示，不进入收编规模；归档-only 项目不算空项目。
  const emptyShellAccounts = accounts.filter(
    (account) => !account.current && account.empty_project_count > 0,
  );
  const staleSessionTotal = staleAccounts.reduce((sum, a) => sum + a.session_count, 0);
  const staleProjectTotal = staleAccounts.reduce((sum, a) => sum + a.project_count, 0);
  // 清理与提示行都按全库口径统计空项目（含当前账号名下的残留行）。
  const emptyProjectTotal = accounts.reduce((sum, a) => sum + a.empty_project_count, 0);
  const hasNotice = staleAccounts.length > 0 || emptyProjectTotal > 0;
  return { staleAccounts, emptyShellAccounts, staleSessionTotal, staleProjectTotal, emptyProjectTotal, hasNotice };
}

/**
 * 收编四阶段进度文案（与后端 master-incorporate-progress 事件一一对应）。
 * 主库沿用「主库」表述；副环境按环境名表述（界面表达纪律：对用户
 * 描述的对象是什么就叫什么）。
 */
function incorporateStageLabels(isMaster: boolean, envName: string) {
  return [
    { stage: "closing", label: isMaster ? "关闭主库" : `关闭「${envName}」` },
    { stage: "backing_up", label: "创建备份" },
    { stage: "incorporating", label: "归入记录" },
    { stage: "restarting", label: isMaster ? "重启主库" : `重启「${envName}」` },
  ];
}

/**
 * P5-5 一键收编弹层（P11-5 收官：主库/副环境共用）：确认（列明规模与
 * 备份提示，ADR-0018 单次确认）→ 四阶段进度 → 完成回执（数量表述）。
 */
function IncorporateDialog({
  target,
  phase,
  staleAccounts,
  staleSessionTotal,
  staleProjectTotal,
  orphanProjectCount,
  onConfirm,
  onClose,
}: {
  target: IncorporateTarget;
  phase: IncorporatePhase;
  staleAccounts: { user_id: string; account_name: string | null }[];
  staleSessionTotal: number;
  staleProjectTotal: number;
  orphanProjectCount: number;
  onConfirm: () => void;
  onClose: () => void;
}) {
  const stages = incorporateStageLabels(target.isMaster, target.envName);
  const allDone = phase.kind === "done" || (phase.kind === "running" && phase.stage === "done");
  const stageIndex = allDone
    ? stages.length
    : phase.kind === "running"
      ? Math.max(0, stages.findIndex((item) => item.stage === phase.stage))
      : 0;
  const progressPercent = allDone
    ? 100
    : Math.round(((stageIndex + 1) / stages.length) * 100);
  const targetName = target.accountName ?? "当前账号";

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
            <div className="switch-dialog__target-meta">
              {target.isMaster ? "主库" : target.envName} · 滞留记录全部归入 · 自动完成转移
            </div>
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
                  {target.isMaster ? "主库" : `「${target.envName}」`}会自动关闭并重启，归入前会创建数据备份。
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
            {phase.kind === "done" && phase.refreshFailed && (
              <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
                <TriangleAlert size={18} aria-hidden="true" />
                <p>归入已完成，但环境状态刷新失败，请手动刷新。</p>
              </div>
            )}
            {phase.kind === "done" && phase.receipt.relaunch_outcome === "failed" && (
              <div className="switch-dialog__notice switch-dialog__notice--danger" role="alert">
                <TriangleAlert size={18} aria-hidden="true" />
                <p>归入已完成，但环境未能自动启动；可从环境页手动启动。</p>
              </div>
            )}
            <div className="switch-dialog__bar" aria-hidden="true">
              <div className="switch-dialog__bar-fill" style={{ width: `${progressPercent}%` }} />
            </div>
            <ol className="switch-dialog__steps">
              {stages.map((item, index) => {
                const restartFailed = phase.kind === "done"
                  && item.stage === "restarting"
                  && phase.receipt.relaunch_outcome === "failed";
                const state = restartFailed ? "failed"
                  : phase.kind === "done" || index < stageIndex ? "done"
                  : phase.kind === "running" && index === stageIndex ? "run" : "idle";
                return (
                  <li key={item.stage} className={`switch-step switch-step--${state}`} data-stage={item.stage}>
                    <span className="switch-step__icon" aria-hidden="true">
                      {state === "done" ? <CheckCircle2 size={16} />
                        : state === "failed" ? <TriangleAlert size={16} />
                          : <span className="switch-step__ring" />}
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
