/**
 * 账号展示名统一口径（U-1 数据层约定）：
 * 本地备注名优先，回退服务端 screen_name。
 * 卡片/详情/签到页等所有账号名展示点必须经由此函数，
 * 保证备注名设置后全局立即一致。
 */
export function effectiveDisplayName(entry: {
  display_name: string | null;
  screen_name: string;
}): string {
  return entry.display_name?.trim() || entry.screen_name;
}
