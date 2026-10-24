#!/usr/bin/env node
// 生成 docs/architecture/MODULE_MAP.md 的三段清单：源码目录级统计、IPC 命令引用、测试/探针清单。
// 统计口径：src/ 只算 .ts/.tsx/.css；Rust 只算 .rs；行数 = 可见行数（有尾换行时等价 wc -l）。
// 用法：
//   node scripts/gen-module-map-inventory.mjs          写入文档三段
//   node scripts/gen-module-map-inventory.mjs --check   只比对不写；退出码 1 = 文档已过期
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const DOC = path.join(ROOT, 'docs/architecture/MODULE_MAP.md');
const CHECK_ONLY = process.argv.includes('--check');
const BT = String.fromCharCode(96);

// 已注册命令 → 定义模块。命令清单以 src-tauri/src/lib.rs 的 generate_handler! 为准；
// 增删命令时改这里（或跑 pnpm gen:commands 后同步），不要手改文档表格。
const COMMANDS = [
  ['apply_master_archive_changes', 'src-tauri/src/library_archive.rs'],
  ['apply_storage_deletion', 'src-tauri/src/lib.rs'],
  ['begin_checkin_login', 'src-tauri/src/oauth_login.rs'],
  ['begin_trae_cn_login', 'src-tauri/src/oauth_login.rs'],
  ['begin_workbuddy_login', 'src-tauri/src/workbuddy_login_commands.rs'],
  ['browse_plugin_market', 'src-tauri/src/plugins.rs'],
  ['cancel_checkin', 'src-tauri/src/checkin_schedule.rs'],
  ['cancel_checkin_login', 'src-tauri/src/oauth_login.rs'],
  ['cancel_trae_cn_login', 'src-tauri/src/oauth_login.rs'],
  ['cancel_workbuddy_login', 'src-tauri/src/workbuddy_login_commands.rs'],
  ['capture_current_account_credential', 'src-tauri/src/credential_handover.rs'],
  ['capture_trae_cn_account', 'src-tauri/src/account_capture.rs'],
  ['capture_workbuddy_account', 'src-tauri/src/workbuddy_commands.rs'],
  ['check_trae_cn_account_health', 'src-tauri/src/credential_handover.rs'],
  ['check_workbuddy_login_health', 'src-tauri/src/workbuddy_commands.rs'],
  ['cleanup_master_empty_projects', 'src-tauri/src/library_archive.rs'],
  ['clear_remote_device', 'src-tauri/src/remote_devices.rs'],
  ['close_environment_instance', 'src-tauri/src/environment.rs'],
  ['complete_checkin_login', 'src-tauri/src/oauth_login.rs'],
  ['complete_trae_cn_login', 'src-tauri/src/oauth_login.rs'],
  ['complete_workbuddy_login', 'src-tauri/src/workbuddy_login_commands.rs'],
  ['create_environment', 'src-tauri/src/environment.rs'],
  ['create_master_backup', 'src-tauri/src/library_archive.rs'],
  ['create_workbuddy_backup', 'src-tauri/src/workbuddy_backup_commands.rs'],
  ['delete_environment', 'src-tauri/src/environment.rs'],
  ['delete_master_backups', 'src-tauri/src/library_archive.rs'],
  ['delete_master_sessions', 'src-tauri/src/library_archive.rs'],
  ['delete_workbuddy_backups', 'src-tauri/src/workbuddy_backup_commands.rs'],
  ['delete_workbuddy_sessions', 'src-tauri/src/workbuddy_library_commands.rs'],
  ['export_recovery_package_command', 'src-tauri/src/lib.rs'],
  ['force_reset_master_current_account', 'src-tauri/src/credential_handover.rs'],
  ['get_account_registry', 'src-tauri/src/credential_handover.rs'],
  ['get_account_storage_footprint', 'src-tauri/src/credential_handover.rs'],
  ['get_archive_draft', 'src-tauri/src/library_archive.rs'],
  ['get_auto_checkin_settings', 'src-tauri/src/checkin_schedule.rs'],
  ['get_backup_dir', 'src-tauri/src/library_archive.rs'],
  ['get_backup_retention', 'src-tauri/src/library_archive.rs'],
  ['get_checkin_capability', 'src-tauri/src/checkin_schedule.rs'],
  ['get_checkin_overview', 'src-tauri/src/checkin_schedule.rs'],
  ['get_current_product', 'src-tauri/src/workbuddy_commands.rs'],
  ['get_environment_delete_preview', 'src-tauri/src/environment.rs'],
  ['get_environment_state', 'src-tauri/src/environment.rs'],
  ['get_managed_account_state', 'src-tauri/src/credential_handover.rs'],
  ['get_managed_credential_statuses', 'src-tauri/src/credential_handover.rs'],
  ['get_master_backup_chain', 'src-tauri/src/library_archive.rs'],
  ['get_master_checkup', 'src-tauri/src/library_archive.rs'],
  ['get_master_history', 'src-tauri/src/library_archive.rs'],
  ['get_master_library_stats', 'src-tauri/src/library_archive.rs'],
  ['get_master_self_check', 'src-tauri/src/credential_handover.rs'],
  ['get_master_session_messages', 'src-tauri/src/library_archive.rs'],
  ['get_master_verification', 'src-tauri/src/library_archive.rs'],
  ['get_plugin_tab_state', 'src-tauri/src/plugins.rs'],
  ['get_product_identity_state', 'src-tauri/src/credential_handover.rs'],
  ['get_product_operation_states', 'src-tauri/src/product_operations.rs'],
  ['get_relay_ledger', 'src-tauri/src/library_archive.rs'],
  ['get_storage_root_state', 'src-tauri/src/lib.rs'],
  ['get_trae_instance_states', 'src-tauri/src/instance_tools.rs'],
  ['get_workbuddy_auto_checkin_status', 'src-tauri/src/workbuddy_schedule.rs'],
  ['get_workbuddy_checkin_overview', 'src-tauri/src/workbuddy_schedule.rs'],
  ['get_workbuddy_library_checkup', 'src-tauri/src/workbuddy_library_commands.rs'],
  ['get_workbuddy_library_history', 'src-tauri/src/workbuddy_library_commands.rs'],
  ['get_workbuddy_library_stats', 'src-tauri/src/workbuddy_library_commands.rs'],
  ['get_workbuddy_probe_info', 'src-tauri/src/workbuddy_backup_commands.rs'],
  ['get_workbuddy_session_health', 'src-tauri/src/workbuddy_commands.rs'],
  ['get_workbuddy_session_messages', 'src-tauri/src/workbuddy_library_commands.rs'],
  ['get_workspace_state', 'src-tauri/src/lib.rs'],
  ['import_recovery_package_command', 'src-tauri/src/lib.rs'],
  ['incorporate_master_records', 'src-tauri/src/library_archive.rs'],
  ['install_plugin', 'src-tauri/src/plugins.rs'],
  ['launch_environment', 'src-tauri/src/environment.rs'],
  ['launch_master_library', 'src-tauri/src/environment.rs'],
  ['list_checkin_accounts', 'src-tauri/src/credential_handover.rs'],
  ['list_environments', 'src-tauri/src/environment.rs'],
  ['list_products', 'src-tauri/src/workbuddy_commands.rs'],
  ['list_remote_devices', 'src-tauri/src/remote_devices.rs'],
  ['list_trae_cn_accounts', 'src-tauri/src/credential_handover.rs'],
  ['list_trae_cn_foreign_projects', 'src-tauri/src/library_archive.rs'],
  ['list_workbuddy_accounts', 'src-tauri/src/workbuddy_commands.rs'],
  ['list_workbuddy_backups', 'src-tauri/src/workbuddy_backup_commands.rs'],
  ['list_workbuddy_checkin_accounts', 'src-tauri/src/workbuddy_schedule.rs'],
  ['login_environment', 'src-tauri/src/environment.rs'],
  ['merge_master_projects', 'src-tauri/src/library_archive.rs'],
  ['merge_trae_cn_project', 'src-tauri/src/library_archive.rs'],
  ['merge_workbuddy_cwd_projects', 'src-tauri/src/workbuddy_library_commands.rs'],
  ['migrate_master_backups', 'src-tauri/src/library_archive.rs'],
  ['migrate_storage_root_command', 'src-tauri/src/lib.rs'],
  ['open_device_management_docs', 'src-tauri/src/remote_devices.rs'],
  ['plan_storage_deletion', 'src-tauri/src/lib.rs'],
  ['prepare_handoff_intent', 'src-tauri/src/credential_handover.rs'],
  ['prepare_managed_account_switch', 'src-tauri/src/credential_handover.rs'],
  ['preview_master_switch_plugins', 'src-tauri/src/credential_handover.rs'],
  ['preview_trae_cn_project_merge', 'src-tauri/src/library_archive.rs'],
  ['preview_workbuddy_switch', 'src-tauri/src/workbuddy_commands.rs'],
  ['purge_account_records', 'src-tauri/src/credential_handover.rs'],
  ['purge_workbuddy_accounts', 'src-tauri/src/workbuddy_commands.rs'],
  ['purge_workbuddy_sessions', 'src-tauri/src/workbuddy_library_commands.rs'],
  ['read_work_cn_state', 'src-tauri/src/lib.rs'],
  ['refresh_checkin_credentials', 'src-tauri/src/credential_handover.rs'],
  ['refresh_checkin_credits', 'src-tauri/src/credential_handover.rs'],
  ['refresh_managed_current_account', 'src-tauri/src/credential_handover.rs'],
  ['refresh_workbuddy_account', 'src-tauri/src/workbuddy_commands.rs'],
  ['remove_checkin_account', 'src-tauri/src/credential_handover.rs'],
  ['rename_environment', 'src-tauri/src/environment.rs'],
  ['repair_master_current_account', 'src-tauri/src/credential_handover.rs'],
  ['reset_checkin_device', 'src-tauri/src/credential_handover.rs'],
  ['restore_master_backup', 'src-tauri/src/library_archive.rs'],
  ['restore_operation', 'src-tauri/src/lib.rs'],
  ['restore_workbuddy_backup', 'src-tauri/src/workbuddy_backup_commands.rs'],
  ['run_checkin', 'src-tauri/src/checkin_schedule.rs'],
  ['set_account_archived', 'src-tauri/src/credential_handover.rs'],
  ['set_account_auto_checkin', 'src-tauri/src/checkin_schedule.rs'],
  ['set_account_display_name', 'src-tauri/src/credential_handover.rs'],
  ['set_account_mobile', 'src-tauri/src/credential_handover.rs'],
  ['set_archive_draft', 'src-tauri/src/library_archive.rs'],
  ['set_auto_checkin_settings', 'src-tauri/src/checkin_schedule.rs'],
  ['set_backup_dir', 'src-tauri/src/library_archive.rs'],
  ['set_backup_retention', 'src-tauri/src/library_archive.rs'],
  ['set_current_product', 'src-tauri/src/workbuddy_commands.rs'],
  ['set_environment_current_account', 'src-tauri/src/environment.rs'],
  ['set_workbuddy_account_archived', 'src-tauri/src/workbuddy_commands.rs'],
  ['set_workbuddy_account_auto_checkin', 'src-tauri/src/workbuddy_schedule.rs'],
  ['set_workbuddy_account_display_name', 'src-tauri/src/workbuddy_commands.rs'],
  ['set_workbuddy_auto_checkin_settings', 'src-tauri/src/workbuddy_schedule.rs'],
  ['switch_account', 'src-tauri/src/credential_handover.rs'],
  ['switch_and_handoff', 'src-tauri/src/credential_handover.rs'],
  ['switch_master_account', 'src-tauri/src/credential_handover.rs'],
  ['switch_trae_cn_account', 'src-tauri/src/credential_handover.rs'],
  ['switch_workbuddy_account', 'src-tauri/src/workbuddy_commands.rs'],
  ['transfer_workbuddy_sessions', 'src-tauri/src/workbuddy_commands.rs'],
  ['uninstall_plugin_everywhere', 'src-tauri/src/plugins.rs'],
  ['verify_workbuddy_capture_renewal', 'src-tauri/src/workbuddy_commands.rs'],
  ['workbuddy_checkin_claim', 'src-tauri/src/workbuddy_commands.rs'],
  ['workbuddy_checkin_claim_batch', 'src-tauri/src/workbuddy_schedule.rs'],
  ['workbuddy_checkin_claim_profile', 'src-tauri/src/workbuddy_schedule.rs'],
  ['workbuddy_checkin_status', 'src-tauri/src/workbuddy_commands.rs'],
];

const SRC_EXT = new Set(['.ts', '.tsx', '.css']);
const RUST_ROOTS = [
  ['src-tauri/src', 'Tauri 根：命令注册与编排'],
  ['src-tauri/crates/commands/src', 'commands：IPC 薄壳'],
  ['src-tauri/crates/application/src', 'application：用例编排'],
  ['src-tauri/crates/domain/src', 'domain：纯领域模型'],
  ['src-tauri/crates/infrastructure/src', 'infrastructure：文件 / SQLCipher / HTTP 实现'],
  ['src-tauri/crates/ports/src', 'ports：端口 trait'],
];

const rel = (p) => path.relative(ROOT, p).split(path.sep).join('/');
const countLines = (p) => {
  const t = fs.readFileSync(p, 'utf8');
  if (!t.length) return 0;
  const n = t.split('\n').length;
  return t.endsWith('\n') ? n - 1 : n;
};
const fmt = (n) => n.toLocaleString('en-US');

function walk(dir, extSet, out) {
  out = out || [];
  if (!fs.existsSync(dir)) return out;
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) walk(p, extSet, out);
    else if (!extSet || extSet.has(path.extname(e.name))) out.push(p);
  }
  return out;
}
// ---------- 1. src/ 目录级统计 ----------
const srcDirs = new Map();
for (const f of walk(path.join(ROOT, 'src'), SRC_EXT)) {
  const key = rel(path.dirname(f)) + '/';
  if (!srcDirs.has(key)) srcDirs.set(key, { files: 0, lines: 0 });
  const rec = srcDirs.get(key);
  rec.files += 1;
  rec.lines += countLines(f);
}

// 目录职责：新增目录请补一行，未登记会显示「未登记职责」
const DIR_ROLE = {
  'src/': '壳入口：App 编排、main 挂载、样式聚合入口',
  'src/components/': '壳层组件（导航、标题栏、能力段、设置、切号、设备、徽章）；目标结构的 src/app/ 尚未拆分',
  'src/components/shared/': '跨 feature 共享控件（确认框、空态、产品徽章）',
  'src/components/shared/account/': '账号行与列表控件、账号偏好（Work 与 CN 共用骨架）',
  'src/components/shared/checkin/': '签到行、流转行、互斥等待倒计时与结果文案',
  'src/components/workbuddy/': 'WorkBuddy 专属小控件',
  'src/features/accounts/': '账号 feature：页面、视图模型、产品 builder、详情六区与动作封装',
  'src/features/accounts/components/': '账号页组件与 hook（行、详情页、弹层、批量进度、登录与刷新流程）',
  'src/features/checkin/': '签到 feature：页面、视图模型、产品 builder',
  'src/features/checkin/components/': '签到页组件与 hook（批量条、页头、自动签到结果、运行与事件）',
  'src/features/environment/': '环境 feature：页面、视图模型、产品 builder',
  'src/features/environment/components/': '环境页装配、动作分发、卡片、体检、删除/收编/登录/改名弹层与 hook',
  'src/features/library/': '库 feature：页面、视图模型、产品 builder、会话树/接力腿/信息页/插件页纯逻辑',
  'src/features/library/components/': '库页壳、树、查看器、归档与批量条、删除与归并弹层、hook',
  'src/features/overview/': '总览 feature：页面与 stats 登记表（无 registry）',
  'src/features/settings/': '设置 feature：页面、视图模型、产品 builder',
  'src/features/settings/components/': '设置页组件与 hook（自动签到段、备份段与弹层、动作分产品）',
  'src/hooks/': '跨页 hook：归档草稿、Tauri 事件订阅宿主',
  'src/platform/': '唯一允许 invoke( 的 IPC 转接层 + 产品注册与能力开关（宪章 4.4）',
  'src/styles/': 'L0-L3 样式：聚合入口、令牌、壳层基础、共享控件、通知、标题栏',
  'src/styles/pages/': 'L4 各页面样式（按页面拆分）',
  'src/types/': 'DTO / 事件 / 命令契约；不直接触发业务',
  'src/ui/': '与业务无关的通用展示原子（徽章、空态、区块标题、统计块）',
  'src/utils/': '纯工具：账号展示与标签、账号注册表、UI 错误脱敏',
};

// 无文件的残留目录（重构后遗留）
const emptyDirs = [];
(function scanEmpty(dir) {
  if (!fs.existsSync(dir)) return;
  const ents = fs.readdirSync(dir, { withFileTypes: true });
  const subdirs = ents.filter((e) => e.isDirectory());
  if (!ents.some((e) => e.isFile()) && subdirs.length === 0) emptyDirs.push(rel(dir) + '/');
  for (const e of subdirs) scanEmpty(path.join(dir, e.name));
})(path.join(ROOT, 'src'));

// ---------- 2. Rust crate 级统计 ----------
const rustStats = RUST_ROOTS.map(([dir, label]) => {
  const files = walk(path.join(ROOT, dir), new Set(['.rs']));
  return { dir, label, files: files.length, lines: files.reduce((a, f) => a + countLines(f), 0) };
});

const sum = (arr, k) => arr.reduce((a, x) => a + x[k], 0);
const srcTotal = { files: sum([...srcDirs.values()], 'files'), lines: sum([...srcDirs.values()], 'lines') };
const rustTotal = { files: sum(rustStats, 'files'), lines: sum(rustStats, 'lines') };
const srcKeys = [...srcDirs.keys()].sort((a, b) => (a === 'src/' ? -1 : b === 'src/' ? 1 : a.localeCompare(b, 'en')));

const inventory = [
  '**统计口径**（P6-2a 重建）：' + BT + 'src/' + BT + ' 只统计 .ts / .tsx / .css，共 ' + srcTotal.files + ' 个文件 / ' + fmt(srcTotal.lines) + ' 行；Rust 只统计 .rs，共 ' + rustTotal.files + ' 个文件 / ' + fmt(rustTotal.lines) + ' 行。行数按可见行计（有尾换行时等价 ' + BT + 'wc -l' + BT + '）。',
  '',
  '**为什么只到目录级**：逐文件列举在 200+ 文件规模下维护成本高且必然过期——上一版清单只覆盖 154/200 个文件，另有 12 行指向重构中已删除的文件。目录级统计 + 生成脚本才是可持续口径：改代码后重跑脚本即可，不需要手工改行号或增删行。需要具体文件时以磁盘为准。',
  '',
  '> 重跑：' + BT + 'node scripts/gen-module-map-inventory.mjs' + BT + '；' + BT + '--check' + BT + ' 只比对不写，退出码 1 表示文档过期。',
  '',
  '#### 前端 ' + BT + 'src/' + BT + '（目录级）',
  '',
  '| 目录 | 文件数 | 行数 | 职责 |',
  '|---|---|---|---|',
  ...srcKeys.map((k) => {
    const d = srcDirs.get(k);
    return '| [' + BT + k + BT + '](../../' + k + ') | ' + d.files + ' | ' + fmt(d.lines) + ' | ' + (DIR_ROLE[k] || '未登记职责，请补 DIR_ROLE') + ' |';
  }),
  '| **合计** | **' + srcTotal.files + '** | **' + fmt(srcTotal.lines) + '** | 脚本生成，与磁盘一致 |',
  '',
  emptyDirs.length
    ? '重构残留空目录（无任何文件）：' + emptyDirs.map((d) => BT + d + BT).join('、') + '。只影响导航心智、不含代码；清理或复用留待后续 UI 票决定，本文只登记事实。'
    : '无残留空目录。',
  '',
  '#### Rust 源码（crate 级）',
  '',
  '| 位置 | 文件数 | 行数 | 说明 |',
  '|---|---|---|---|',
  ...rustStats.map((r) => '| [' + BT + r.dir + '/' + BT + '](../../' + r.dir + '/) | ' + r.files + ' | ' + fmt(r.lines) + ' | ' + r.label + ' |'),
  '| **合计** | **' + rustTotal.files + '** | **' + fmt(rustTotal.lines) + '** | 五个 crate + Tauri 根，不含 tests / examples |',
  '',
  '责任归属按上方「责任与依赖」表的模块职责判定，不按文件路径判定，因此目录级清单足以支撑归属核对。',
].join('\n');

// ---------- 2b. 「六个 feature 目录」表的文件/行数与页面入口行数同步 ----------
// 该表在正文（§前端目录职责）里，数字最容易随重构过期，故一并生成。
const FEATURES = ['accounts', 'checkin', 'environment', 'library', 'overview', 'settings'];
function featureStat(name) {
  const files = walk(path.join(ROOT, 'src/features', name), SRC_EXT);
  return { files: files.length, lines: files.reduce((a, f) => a + countLines(f), 0) };
}
const PAGE_ENTRY = {
  accounts: 'AccountsPage.tsx',
  checkin: 'CheckinPage.tsx',
  environment: 'EnvironmentPage.tsx',
  library: 'LibraryPage.tsx',
  overview: 'OverviewPage.tsx',
  settings: 'SettingsPage.tsx',
};
// ---------- 3. IPC 命令表：引用列从磁盘静态扫描重建 ----------
const Q = [String.fromCharCode(39), String.fromCharCode(34), BT];
const readAll = (dir, exts) => walk(path.join(ROOT, dir), new Set(exts)).map((f) => [rel(f), fs.readFileSync(f, 'utf8')]);
const srcTexts = readAll('src', ['.ts', '.tsx']).filter(([r]) => r !== 'src/types/commandNames.ts');
const testTexts = readAll('tests', ['.ts', '.tsx']).concat(readAll('e2e', ['.ts', '.tsx', '.mjs']));
const hitsOf = (texts, cmd) => texts.filter(([, t]) => Q.some((q) => t.includes(q + cmd + q))).map(([r]) => r);
const link = (r) => '[' + r + '](../../' + r + ')';
const missingDef = [];
const commandRows = COMMANDS.map(([cmd, def]) => {
  if (!fs.existsSync(path.join(ROOT, def))) missingDef.push(cmd);
  const prod = hitsOf(srcTexts, cmd);
  const test = hitsOf(testTexts, cmd);
  return '| ' + cmd + ' | [' + def + '](../../' + def + ') | ' + (prod.length ? prod.map(link).join('<br>') : '无') + ' | ' + (test.length ? test.map(link).join('<br>') : '无') + ' |';
});
// 实际注册的 handler 数（generate_handler! 内的命令条目），用于核对本表覆盖率。
// 用括号深度扫描 + 去注释/字符串，避免注释里的 "]" 提前截断、也避免末项无尾逗号被漏计；
// 口径与 scripts/gen-command-union.mjs 的 extractCommandNames 一致（同一份真源）。
function extractHandlerNames(source) {
  const m = source.search(/generate_handler!\s*\[/);
  if (m === -1) return null;
  const open = source.indexOf('[', m);
  let depth = 0;
  let inLine = false;
  let inBlock = false;
  let inString = false;
  let i = open;
  for (; i < source.length; i += 1) {
    const ch = source[i];
    const next = source[i + 1];
    if (inLine) { if (ch === '\n') inLine = false; continue; }
    if (inBlock) { if (ch === '*' && next === '/') { inBlock = false; i += 1; } continue; }
    if (inString) { if (ch === '\\') i += 1; else if (ch === '"') inString = false; continue; }
    if (ch === '/' && next === '/') { inLine = true; i += 1; continue; }
    if (ch === '/' && next === '*') { inBlock = true; i += 1; continue; }
    if (ch === '"') { inString = true; continue; }
    if (ch === '[') depth += 1;
    else if (ch === ']') { depth -= 1; if (depth === 0) break; }
  }
  if (i >= source.length) return null;
  const body = source.slice(open + 1, i).replace(/\/\/[^\n]*/g, '').replace(/\/\*[\s\S]*?\*\//g, '');
  const names = body.split(',').map((x) => x.trim()).filter(Boolean).map((x) => x.split('::').pop());
  return names.length ? { total: names.length, names } : null;
}
function countHandlers() {
  const libPath = path.join(ROOT, 'src-tauri/src/lib.rs');
  if (!fs.existsSync(libPath)) return null;
  return extractHandlerNames(fs.readFileSync(libPath, 'utf8'));
}
const handlers = countHandlers();
const unregistered = handlers ? handlers.names.filter((n) => !COMMANDS.some(([c]) => c === n)) : [];
// 反向：本表登记但 handler 未注册（历史遗留/已退役命令），必须为零
const tableOnly = handlers ? COMMANDS.map(([c]) => c).filter((c) => !handlers.names.includes(c)) : [];
const coverageNote = handlers
  ? '**口径（P6-2b 定稿）**：本表登记 ' + BT + 'generate_handler!' + BT + ' 的**全量命令**，不按产品线裁剪——命令只要在 Rust 侧注册就登记一行，是否被前端消费由「生产前端字符串命中」列回答（列内「无」表示没有前端字符串引用，**不是删除许可**，24 条分类见上表）。' +
    ' 当前 handler 实际注册 ' + handlers.total + ' 条，本表登记 ' + COMMANDS.length + ' 条' +
    (unregistered.length || tableOnly.length
      ? '；**不一致**：handler 有而本表缺 ' + unregistered.length + ' 条' + (unregistered.length ? '（' + unregistered.join(' / ') + '）' : '') +
        '，本表有而 handler 无 ' + tableOnly.length + ' 条' + (tableOnly.length ? '（' + tableOnly.join(' / ') + '）' : '') + '。请同步 COMMANDS 表。'
      : '，与 handler 完全一致（逐条比对无缺、无多）。')
  : '未能读取 ' + BT + 'generate_handler!' + BT + '，覆盖率未核对。';

const commands = [
  coverageNote,
  '',
  '| 命令 | 定义模块 | 生产前端字符串命中 | 测试 / mock 字符串命中 |',
  '|---|---|---|---|',
  ...commandRows,
].join('\n');

// ---------- 4. 测试、探针与构建辅助 ----------
const auxFiles = [];
for (const d of ['e2e', 'scripts', 'src-tauri/crates/infrastructure/examples', 'src-tauri/crates/infrastructure/tests', 'src-tauri/tests', 'tests']) {
  auxFiles.push(...walk(path.join(ROOT, d), null).map(rel));
}
const aux = auxFiles.sort((a, b) => a.localeCompare(b, 'en')).map((r) => '- [' + r + '](../../' + r + ')').join('\n');

// ---------- 5. 注入到标记之间 ----------
function replaceBlock(text, name, body) {
  const begin = '<!-- gen:' + name + ':begin -->';
  const end = '<!-- gen:' + name + ':end -->';
  const i = text.indexOf(begin);
  const j = text.indexOf(end);
  if (i < 0 || j < 0) throw new Error('缺少标记 ' + begin + ' / ' + end);
  return text.slice(0, i + begin.length) + '\n' + body + '\n' + text.slice(j);
}

// 正文 feature 表：把每行的「文件/行数」与「页面入口行数」改成磁盘真值（按行重写，不用整行正则）
function syncFeatureTable(text) {
  const out = [];
  let changed = 0;
  for (const line of text.split('\n')) {
    const m = line.match(/^\| \[src\/features\/([a-z]+)\/\]\([^)]+\) \| ([0-9,]+ \/ [0-9,]+) \| `([A-Za-z]+\.tsx)`（([0-9]+) 行） \|/);
    if (!m) { out.push(line); continue; }
    const name = m[1];
    if (!FEATURES.includes(name)) { out.push(line); continue; }
    const st = featureStat(name);
    const pagePath = path.join(ROOT, 'src/features', name, m[3]);
    const pageLines = fs.existsSync(pagePath) ? countLines(pagePath) : Number(m[4]);
    const rebuilt = line
      .replace(m[2], st.files + ' / ' + fmt(st.lines))
      .replace(BT + m[3] + BT + '（' + m[4] + ' 行）', BT + m[3] + BT + '（' + pageLines + ' 行）');
    if (rebuilt !== line) changed += 1;
    out.push(rebuilt);
  }
  return { text: out.join('\n'), changed };
}
const docTextRaw = fs.readFileSync(DOC, 'utf8');
const featureSync = syncFeatureTable(docTextRaw);
const docText = featureSync.text;
console.log('feature 表同步 ' + featureSync.changed + ' 行');
let out = replaceBlock(docText, 'inventory', inventory);
out = replaceBlock(out, 'commands', commands);
out = replaceBlock(out, 'aux', aux);

// ---------- 6. 链接自检：文档里任何 ../../ 相对链接都必须指向磁盘真实文件 ----------
// 这条是 P6-2a 的验收机制：旧清单 12 行指向已删文件，靠人工 grep 不可靠；
// 直接校验链接可达性，删文件后重跑生成会立刻暴露。
function brokenLinks(text) {
  const bad = [];
  for (const m of text.matchAll(/\[([^\]]*)\]\(([^)]+)\)/g)) {
    const target = m[2].split('#')[0];
    if (!target || /^https?:/.test(target)) continue;
    if (!fs.existsSync(path.resolve(path.dirname(DOC), target))) bad.push(target);
  }
  return [...new Set(bad)];
}

const broken = brokenLinks(out);
console.log(broken.length ? '失效链接 ' + broken.length + ' 条:\n  ' + broken.join('\n  ') : '链接自检通过：所有相对链接均指向磁盘真实路径');
if (broken.length && !CHECK_ONLY) {
  console.log('警告：文档已写入，但仍有失效链接（多为已删文件的正文提及，需人工改文案）');
}
if (missingDef.length) console.log('定义模块缺失: ' + missingDef.join(', '));
if (CHECK_ONLY) {
  console.log(out === docText ? '一致：文档与磁盘相同' : '过期：文档与磁盘不一致，请重跑生成');
  process.exit(out === docText ? 0 : 1);
}
fs.writeFileSync(DOC, out, 'utf8');
console.log('已写入 ' + rel(DOC));
console.log('src: ' + srcTotal.files + ' 文件 / ' + srcTotal.lines + ' 行；Rust: ' + rustTotal.files + ' 文件 / ' + rustTotal.lines + ' 行');
console.log('命令: ' + commandRows.length + ' 条；辅助条目: ' + aux.split('\n').length + ' 条');
if (emptyDirs.length) console.log('空目录: ' + emptyDirs.join(', '));