/**
 * 账号页本地偏好（P19-B1② 从 AccountCenter 上移）：视图/排序的 localStorage
 * 读写收口到共享层。key 自本切片起按产品分前缀（`accounts.<product_id>.*`），
 * 避免多产品共用全局 key 互相污染——既有全局 key（accounts.view/accounts.sort）
 * 的偏好随此变化重置一次（P19 拍板决策 5，可接受）。
 */

/** 读取本地偏好；非法值回退默认，保证 localStorage 脏数据不炸页面。 */
export function readStoredPreference<T extends string>(key: string, fallback: T, allowed: readonly T[]): T {
  try {
    const value = window.localStorage.getItem(key);
    return allowed.includes(value as T) ? (value as T) : fallback;
  } catch {
    return fallback;
  }
}

/** 写入本地偏好；存储不可用时仅本次会话生效（调用方无需处理）。 */
export function writeStoredPreference(key: string, value: string): void {
  try {
    window.localStorage.setItem(key, value);
  } catch { /* 存储不可用时仅本次会话生效 */ }
}

/** 账号页偏好键：按产品分 key（视图 / 排序）。 */
export function accountPreferenceKey(productId: string, preference: "view" | "sort"): string {
  return `accounts.${productId}.${preference}`;
}
