/**
 * 库实例引用的公共类型（P4e-9c 收敛）。
 *
 * 原定义在旧库页组件 `src/components/LibrarySessionsPanel.tsx` 内；该组件已随
 * P4e 重构删除，宿主（App / platform/productRegistry）只依赖这个形状，故上移到
 * `src/types`——避免页面/平台层反向依赖具体 UI 组件。
 */
export interface LibraryRef {
  readonly id: string;
  readonly displayName?: string;
}
