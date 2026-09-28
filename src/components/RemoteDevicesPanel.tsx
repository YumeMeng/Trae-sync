import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, LogOut, MonitorSmartphone, RefreshCw } from "lucide-react";
import { ConfirmDialog } from "./ConfirmDialog";
import { safeUiErrorMessage } from "../utils/safeUiError";
import type { RemoteDeviceEntry, RemoteDeviceSnapshot } from "../types/product";

interface RemoteDevicesPanelProps {
  /** 面板归属的产品（work_cn / trae_cn）；后端据此选择凭据材料根与 OAuth client。 */
  productId: string;
  /** 产品名：归属标注用，明确这是账号级列表而非产品切换入口。 */
  productName: string;
  /** 面板归属账号的 profile_id（必传）：P10-5 起面板只挂在账号详情页，固定绑定单账号。 */
  profileId: string;
  /** 账号显示名（详情页父层必然已知）；空串时回退后端返回的 account_label。 */
  accountLabel: string;
  /**
   * 退出设备动作（破坏性，调用前必须已完成单次确认）。
   * 缺省实现调用 clear_remote_device 命令（P10-5 切片 3 接入后端）；
   * 测试注入 fake 验证确认流程。
   */
  onSignOut?: (deviceId: string) => Promise<void>;
  /**
   * 退出成功且列表重拉完成后触发一次（ADR-0031 决策 6：父层借此联动
   * 该账号健康检查）；失败路径不触发，面板自身不耦合健康检查逻辑。
   * 参数为本面板归属账号的 profile_id。
   */
  onSignedOut?: (profileId: string) => void;
}

/** 常见设备类型值的友好映射；未知值一律归入「其他设备」，不透传内部枚举。 */
const DEVICE_TYPE_LABELS: Record<string, string> = {
  IDE_PC: "桌面设备",
  SOLO_PC: "桌面设备",
  PC: "桌面设备",
  DESKTOP: "桌面设备",
  MOBILE: "移动设备",
  PHONE: "移动设备",
  ANDROID: "移动设备",
  IOS: "移动设备",
  WEB: "网页端",
};

/** 中性归属回退文案：与后端 lib.rs 的 account_label 回退值保持一致。 */
const NEUTRAL_ACCOUNT_LABEL = "当前账号";

function deviceTypeLabel(type: string | null): string {
  if (!type || !type.trim()) return "其他设备";
  return DEVICE_TYPE_LABELS[type.trim().toUpperCase()] ?? "其他设备";
}

function pad2(value: number): string {
  return value < 10 ? `0${value}` : String(value);
}

/**
 * 解析最近活跃时刻为毫秒时间戳；解析不了返回 null（排序沉底）。
 * 三形态兼容：数字毫秒时间戳 / 字符串（数字串或日期串）/ null。
 */
function lastActiveTime(value: RemoteDeviceEntry["last_active_at"]): number | null {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string") {
    const text = value.trim();
    if (!text) return null;
    if (/^\d+$/.test(text)) return Number(text);
    // 优先手工解析 "YYYY-MM-DD HH:mm(:ss)"（服务端实测形态，跨引擎确定性），
    // 失败再尝试 Date.parse（ISO 等标准形态）。
    const matched = text.match(/^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2})/);
    if (matched) {
      const [, year, month, day, hour, minute] = matched.map(Number);
      const parsed = new Date(year, month - 1, day, hour, minute).getTime();
      return Number.isNaN(parsed) ? null : parsed;
    }
    const parsed = Date.parse(text);
    return Number.isNaN(parsed) ? null : parsed;
  }
  return null;
}

/** 绝对时刻：MM-DD HH:mm（当年内足够辨识；跨年信息由相对时间补充）。 */
function formatAbsolute(timestamp: number): string {
  const date = new Date(timestamp);
  return `${pad2(date.getMonth() + 1)}-${pad2(date.getDate())} ${pad2(date.getHours())}:${pad2(date.getMinutes())}`;
}

/** 相对时刻：刚刚 / N 分钟前 / N 小时前 / N 天前。 */
function formatRelative(timestamp: number): string {
  const minutes = Math.floor(Math.max(0, Date.now() - timestamp) / 60_000);
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  return `${Math.floor(hours / 24)} 天前`;
}

/** 最近活跃展示：绝对 + 相对双形式；解析失败显示原文或「—」。 */
function formatLastActive(value: RemoteDeviceEntry["last_active_at"]): string {
  const timestamp = lastActiveTime(value);
  if (timestamp === null) {
    if (typeof value === "string" && value.trim()) return value.trim();
    return "—";
  }
  return `${formatAbsolute(timestamp)}（${formatRelative(timestamp)}）`;
}

/**
 * 设备行排序（ADR-0031 决策 9 基线之上）：最近活跃倒序；
 * 时刻缺失或不可解析的行沉底，保持列表主体可扫读。
 */
function sortDevices(devices: readonly RemoteDeviceEntry[]): RemoteDeviceEntry[] {
  return [...devices].sort((a, b) => {
    const left = lastActiveTime(a.last_active_at);
    const right = lastActiveTime(b.last_active_at);
    if (left === null && right === null) return 0;
    if (left === null) return 1;
    if (right === null) return -1;
    return right - left;
  });
}

/**
 * 账号级远程设备面板（ADR-0031）：展示当前账号在服务端的登录设备占用，
 * 支持逐台退出（单次确认；本机行不渲染退出按钮——UI 第一道防线，
 * 后端 clear_refresh_token 拒绝本机目标为第二道防线）。
 * 列表分三段：汇总行 → 本机设备区块（置顶强调）→「其他设备 (N)」折叠分组
 * （默认折叠，点头部展开）；两组各自保持最近活跃倒序。
 * P10-5 起只内嵌在账号详情视图，固定绑定传入的单账号。
 * 默认折叠，展开才拉取列表，避免打开详情页就触网。
 */
export function RemoteDevicesPanel({
  productId,
  productName,
  profileId,
  accountLabel,
  onSignOut,
  onSignedOut,
}: RemoteDevicesPanelProps) {
  const [expanded, setExpanded] = useState(false);
  // 「其他设备」折叠分组：默认折叠，面板本地状态；面板整体收起再展开时保留。
  const [othersExpanded, setOthersExpanded] = useState(false);
  const [snapshot, setSnapshot] = useState<RemoteDeviceSnapshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 退出确认弹层：目标设备行 + 执行中状态 + 失败文案（失败收起弹层后回落到面板提示）。
  const [signOutTarget, setSignOutTarget] = useState<RemoteDeviceEntry | null>(null);
  const [signOutBusy, setSignOutBusy] = useState(false);
  const [signOutError, setSignOutError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // 账号代次：切换归属账号时递增，让飞行中的旧账号请求结果作废，
  // 防止旧账号快照在重置后被迟到的响应重新写入。
  const accountGenerationRef = useRef(0);

  // 切换归属账号（productId/profileId 变化）：重置为折叠并清空快照、加载中、
  // 错误、确认弹层与其他设备折叠分组等状态；下次展开按新账号重新拉取，
  // 避免以新账号名显示旧账号设备。
  useEffect(() => {
    accountGenerationRef.current += 1;
    setExpanded(false);
    setOthersExpanded(false);
    setSnapshot(null);
    setLoading(false);
    setError(null);
    setNotice(null);
    setSignOutTarget(null);
    setSignOutError(null);
  }, [productId, profileId]);

  const load = useCallback(async () => {
    const generation = accountGenerationRef.current;
    setLoading(true);
    setError(null);
    setNotice(null);
    try {
      const result = await invoke<RemoteDeviceSnapshot>("list_remote_devices", {
        productId,
        profileId,
      });
      // 请求飞行中账号已切换：丢弃过期结果，不写入任何状态。
      if (generation !== accountGenerationRef.current) return;
      setSnapshot(result);
    } catch (err) {
      if (generation !== accountGenerationRef.current) return;
      setError(safeUiErrorMessage(err, "设备列表读取失败，请稍后重试。"));
    } finally {
      // 过期请求不回写 loading（重置流程已处理），避免误关新账号的加载态。
      if (generation === accountGenerationRef.current) setLoading(false);
    }
  }, [productId, profileId]);

  const toggleExpanded = () => {
    const next = !expanded;
    setExpanded(next);
    // 懒加载：只在首次展开时拉取；收起再展开沿用已加载的快照，手动刷新更新。
    if (next && snapshot === null && !loading && error === null) {
      void load();
    }
  };

  const confirmSignOut = async () => {
    if (!signOutTarget || signOutBusy) return;
    setSignOutBusy(true);
    setSignOutError(null);
    try {
      const action =
        onSignOut ??
        (async (deviceId: string) => {
          await invoke("clear_remote_device", { productId, profileId, deviceId });
        });
      await action(signOutTarget.device_id);
      setSignOutTarget(null);
      await load();
      // 重拉完成后再提示：文案与列表状态一致，且不会被子序列 load 清空。
      setNotice("已退出该设备，列表已刷新。");
      // ADR-0031 决策 6：退出成功联动父层后置动作（该账号健康检查），触发一次即可。
      onSignedOut?.(profileId);
    } catch (err) {
      setSignOutError(safeUiErrorMessage(err, "退出设备失败，请稍后重试。"));
      setSignOutTarget(null);
    } finally {
      setSignOutBusy(false);
    }
  };

  // 归属标注：父层显示名优先，回退后端 account_label；都没有时不硬造。
  // 后端中性回退「当前账号」时不套「账号「…」」模板，避免重复表述。
  const ownerLabel = accountLabel?.trim() || snapshot?.account_label || null;
  const ownerLine =
    ownerLabel === null
      ? null
      : ownerLabel === NEUTRAL_ACCOUNT_LABEL
        ? `${ownerLabel} · ${productName}`
        : `账号「${ownerLabel}」 · ${productName}`;
  const nearLimit =
    snapshot?.max_count != null && snapshot.used_count >= snapshot.max_count - 1;
  const sortedDevices = snapshot ? sortDevices(snapshot.devices) : [];
  // 拆分本机与其他设备（filter 保持稳定排序，两组各自维持最近活跃倒序）。
  const localDevices = sortedDevices.filter((device) => device.is_local);
  const otherDevices = sortedDevices.filter((device) => !device.is_local);

  // 设备行渲染（本机区块与其他分组共用同一布局）；退出按钮只挂在非本机行
  // （ADR-0031 决策 3 UI 防线：本机行绝不渲染退出按钮）。
  const renderDeviceRow = (device: RemoteDeviceEntry) => (
    <li
      key={device.device_id}
      className="remote-devices__row"
      data-testid={`remote-device-row-${device.device_id}`}
    >
      <div className="remote-devices__row-main">
        <span className="remote-devices__name">
          {device.device_name?.trim() || "未命名设备"}
        </span>
        <span className="remote-devices__type">
          {deviceTypeLabel(device.device_type)}
        </span>
        {device.is_local && (
          <span className="remote-devices__badge">本机</span>
        )}
        {!device.is_local && (
          <button
            className="btn remote-devices__signout"
            type="button"
            onClick={() => {
              setSignOutError(null);
              setSignOutTarget(device);
            }}
            disabled={signOutBusy}
            data-testid={`remote-device-signout-${device.device_id}`}
          >
            <LogOut size={14} aria-hidden="true" />
            退出
          </button>
        )}
      </div>
      <div className="remote-devices__row-meta">
        <span>最近活跃：{formatLastActive(device.last_active_at)}</span>
        {device.bound_products.length > 0 ? (
          <span className="remote-devices__products">
            {device.bound_products.map((product, index) => (
              <span
                key={`${product}-${index}`}
                className="remote-devices__product-tag"
              >
                {product}
              </span>
            ))}
          </span>
        ) : (
          <span>未显示绑定产品</span>
        )}
      </div>
    </li>
  );

  return (
    <section
      className="account-center__profiles remote-devices"
      aria-labelledby="remote-devices-heading"
      data-testid="remote-devices-panel"
    >
      <div className="account-center__section-heading">
        <h3 id="remote-devices-heading">
          <MonitorSmartphone size={16} aria-hidden="true" />
          登录设备
        </h3>
        <button
          className="btn"
          type="button"
          onClick={toggleExpanded}
          aria-expanded={expanded}
          data-testid="remote-devices-toggle"
        >
          {expanded ? "收起" : "查看设备"}
        </button>
        {expanded && (
          <button
            className="btn"
            type="button"
            onClick={() => void load()}
            disabled={loading || signOutBusy}
            data-testid="remote-devices-refresh"
          >
            <RefreshCw size={15} className={loading ? "icon-spin" : undefined} aria-hidden="true" />
            {loading ? "读取中…" : "刷新"}
          </button>
        )}
      </div>
      {expanded && (
        <>
          {ownerLine && (
            <p className="remote-devices__owner">{ownerLine}</p>
          )}
          {loading && !snapshot && (
            <p className="account-center__meta" role="status">
              正在读取登录设备…
            </p>
          )}
          {error && (
            <div className="remote-devices__state">
              <p className="workbench__error" role="alert" data-testid="remote-devices-error">
                {error}
              </p>
              <button
                className="btn"
                type="button"
                onClick={() => void load()}
                data-testid="remote-devices-retry"
              >
                重试
              </button>
              {/* ADR-0031 决策 2 兜底入口：原生列表不可用时仍可到达官方设备管理。
                  官方授权页无公开稳定 URL，打开官方文档页；说明文案告知实际入口。 */}
              <button
                className="btn"
                type="button"
                title="可在官方客户端或网页登录页管理设备"
                onClick={() => void invoke("open_device_management_docs").catch(() => undefined)}
                data-testid="remote-devices-official-docs"
              >
                打开官方设备管理页
              </button>
              <p className="account-center__meta">可在 TRAE 官方客户端或网页登录页管理设备。</p>
            </div>
          )}
          {!error && snapshot && (
            <>
              {notice && (
                <p className="account-center__message" role="status">
                  {notice}
                </p>
              )}
              {signOutError && (
                <p className="workbench__error" role="alert">
                  {signOutError}
                </p>
              )}
              <div
                className={`remote-devices__summary${nearLimit ? " remote-devices__summary--warn" : ""}`}
                data-testid="remote-devices-summary"
              >
                {snapshot.max_count != null
                  ? `已用 ${snapshot.used_count} / 上限 ${snapshot.max_count} 台`
                  : `已用 ${snapshot.used_count} 台`}
              </div>
              {sortedDevices.length > 0 ? (
                <>
                  {/* 本机设备区块：置顶强调展示。匹配失败是合法状态
                      （服务端可能未登记本机或字段格式差异），空态用中性提示。 */}
                  <div className="remote-devices__local" data-testid="remote-devices-local">
                    {localDevices.length > 0 ? (
                      <ul className="remote-devices__list">
                        {localDevices.map(renderDeviceRow)}
                      </ul>
                    ) : (
                      <p
                        className="remote-devices__local-empty"
                        data-testid="remote-devices-local-empty"
                      >
                        未识别到本机对应的设备
                      </p>
                    )}
                  </div>
                  {/* 其他设备折叠分组：默认折叠，点头部展开/收起；
                      展开后行布局与原列表一致，组内保持最近活跃倒序。 */}
                  <div className="remote-devices__others" data-testid="remote-devices-others">
                    <button
                      className="remote-devices__others-toggle"
                      type="button"
                      onClick={() => setOthersExpanded((value) => !value)}
                      aria-expanded={othersExpanded}
                      data-testid="remote-devices-others-toggle"
                    >
                      <ChevronDown
                        size={14}
                        aria-hidden="true"
                        className="remote-devices__others-chevron"
                      />
                      其他设备 ({otherDevices.length})
                    </button>
                    {othersExpanded && otherDevices.length > 0 && (
                      <ul className="remote-devices__list">
                        {otherDevices.map(renderDeviceRow)}
                      </ul>
                    )}
                  </div>
                </>
              ) : (
                // 防御态：协议保证成功响应至少含本机一行，理论不出现。
                <p className="account-center__empty">暂无设备记录。</p>
              )}
            </>
          )}
        </>
      )}
      {signOutTarget && (
        <ConfirmDialog
          title="退出这台设备？"
          danger
          busy={signOutBusy}
          confirmLabel="退出设备"
          busyLabel="正在退出…"
          lines={[
            `设备：${signOutTarget.device_name?.trim() || "未命名设备"}（${deviceTypeLabel(signOutTarget.device_type)}）`,
            signOutTarget.bound_products.length > 0
              ? `绑定产品：${signOutTarget.bound_products.join("、")}`
              : "绑定产品：无",
            "退出后，这台设备上登录的全部 TRAE 产品都需要重新登录。",
          ]}
          onCancel={() => {
            if (!signOutBusy) setSignOutTarget(null);
          }}
          onConfirm={() => void confirmSignOut()}
          testId="remote-device-signout-confirm"
        />
      )}
    </section>
  );
}
