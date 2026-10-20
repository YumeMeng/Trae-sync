/**
 * 签到结果文案表（P19-B2④，架构草案 §1.2D）：checkinOutcomeLabel 的
 * per-product 映射——TRAE 表原样迁入（ADR-0019 业务码透出规则），
 * 其他产品回退通用 outcome 档（WB 错误细节走 notify 通道，不进行内徽章）。
 */

/** TRAE 签到结果文案（业务码 → 人话；原 CheckinPage.checkinOutcomeLabel 原样迁入）。 */
function traeOutcomeLabel(outcome: string, detailCode: string | null): string {
  switch (detailCode) {
    case "account_not_logged_in": return "未登录，请先添加账号";
    case "recovery_required": return "存在中断的续期现场，需先恢复再签到";
    case "credential_missing": return "登录材料缺失，请重新登录";
    case "credential_unavailable": return "登录存储不可用，请重新登录";
    case "credential_invalid": return "登录材料无效，请重新登录";
    case "device_too_new": return "设备刚创建，请等待约 5 分钟后重试";
    case "business_9074": return "设备信息未获服务端信任（9074），可在账号详情重置签到设备";
    case "business_9095": return "该设备今日名额已用（9095），明日再试";
    case "remint_failed": return "重置设备失败，请在账号详情手动重试";
    case "business_20324": return "登录已失效（20324），请重新登录";
    default: break;
  }
  // 未知业务码原样透出数值，保证排障不丢信息（ADR-0019：业务码不得压平）。
  if (detailCode && detailCode.startsWith("business_")) return `服务端拒绝（${detailCode.slice("business_".length)}）`;
  return genericOutcomeLabel(outcome);
}

/** 通用 outcome 档（未注册透出表的产品回退；WB 一期复用）。 */
function genericOutcomeLabel(outcome: string): string {
  switch (outcome) {
    case "claimed": return "签到成功";
    case "already_checked_in": return "今日已签到";
    case "not_eligible": return "当前不可领取";
    case "verification_failed": return "结果待复核";
    case "auth_mismatch": return "授权不匹配";
    case "credential_refresh_failed": return "登录续期失败，请重新登录";
    case "network_error": return "网络错误";
    case "profile_busy": return "账号忙";
    default: return "签到失败";
  }
}

/** 按产品查询签到结果文案（productId 是数据键，不是布局分支）。 */
export function outcomeLabelFor(productId: string, outcome: string, detailCode: string | null = null): string {
  switch (productId) {
    case "trae_cn":
    case "work_cn": // Work 签到档案链的协议值（ADR-0035），与 TRAE 同表。
      return traeOutcomeLabel(outcome, detailCode);
    default:
      return genericOutcomeLabel(outcome);
  }
}

/** 演进式行（checkin-flow）结果语气：claimed/already 安静（中性）；其余琥珀（需要关注）。 */
export function flowResultTone(outcome: string | null): "ok" | "warn" {
  return outcome === "claimed" || outcome === "already_checked_in" ? "ok" : "warn";
}

/** 选择行（account-center__profile）结果语气：与状态徽章三档对齐。 */
export function selectResultTone(outcome: string | null): "safe" | "neutral" | "warning" {
  return outcome === "claimed" ? "safe"
    : outcome === "already_checked_in" ? "neutral" : "warning";
}
