import type { AccountRegistryAuthorizationState } from "../types/account_switch";

export type AccountRegistryTone = "safe" | "warning" | "danger" | "neutral";

// 注册表状态只描述本机档案可用性，不代表历史数据或远程签到状态。
export function accountRegistryStateLabel(state: AccountRegistryAuthorizationState): string {
  switch (state) {
    case "current_verified": return "当前已验证";
    case "credential_saved": return "可快速切换";
    case "profile_saved": return "未保存登录";
    case "credential_stale": return "登录已过期";
    case "credential_invalid": return "登录材料无效";
    case "reverification_required": return "需要重新验证";
    case "conflict": return "档案冲突";
    case "unbound": return "暂无授权";
  }
}

export function accountRegistryStateTone(state: AccountRegistryAuthorizationState): AccountRegistryTone {
  switch (state) {
    case "current_verified":
    case "credential_saved":
      return "safe";
    case "credential_stale":
    case "reverification_required":
      return "warning";
    case "credential_invalid":
    case "conflict":
      return "danger";
    case "profile_saved":
    case "unbound":
      return "neutral";
  }
}
