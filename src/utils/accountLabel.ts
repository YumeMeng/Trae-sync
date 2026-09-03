import type { CurrentAccountStateDto } from "../types/workspace";

// 生产界面只展示不可逆账号指纹，不展示原始 user_id 或认证材料。
// 2026-09-01 UI 纪律：文案用自然表达（「当前账号 · 短码」），不再使用「指纹」术语。
export function renderSafeTargetAccount(account?: CurrentAccountStateDto): string {
  const fingerprint = account?.user_fingerprint?.trim();
  return fingerprint ? `当前账号 · ${fingerprint}` : "";
}
