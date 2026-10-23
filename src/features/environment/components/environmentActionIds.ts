/**
 * 环境页动作 id 常量（P4d-8a-2b 从 EnvironmentPage 抽出）：builder 约定 testId = id，
 * 副环境卡动作 = env-<动词>-<env_id>。页面分发与忙碌位映射共用同一份字面量，
 * 避免两处各写一份字符串（弹层 testid 另见各弹层组件）。
 */
export const MASTER_DETAIL_ACTION = "env-master-detail";
export const MASTER_LAUNCH_ACTION = "env-master-launch";
export const MASTER_CLOSE_ACTION = "env-master-close";
export const REPAIR_ACTION = "env-self-check-repair";
export const CREATE_ACTION = "env-create-entry";
export const CAPTURE_ACTION = "env-capture-account-entry";
export const RENAME_PREFIX = "env-rename-";
export const DELETE_PREFIX = "env-delete-";
export const LAUNCH_PREFIX = "env-launch-";
export const CLOSE_PREFIX = "env-close-";
