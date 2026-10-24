#!/usr/bin/env node
// ============================================================================
// UI 护栏（docs/architecture/UI-CHARTER.md §3 硬预算 + §4 禁止模式）
// ============================================================================
//
// 五条规则（只读扫描，不改产品代码）：
//   1. 行数预算：src/app、src/features、src/ui 下单文件 > 300 行（后两者当前
//      不存在时自然跳过）。
//   2. 禁止产品分支：src/app、src/features 内出现 isWorkBuddy ? /
//      productId === "workbuddy" / WORKBUDDY_PRODUCT_ID（能力判断必须走
//      capabilities / sectionStates）。
//   3. 字面色值：src/styles 下除 tokens.css 外出现 #hex / rgb( / hsl(。
//   4. 跨 feature 深路径 import：features/<name>/<非 public> 的 from 导入。
//   5. 组件内 invoke(：src/app、src/features 内直接调用 Tauri 命令（必须经
//      platform/ 适配层）。
//
// 基线机制（关键）：宪章规则今天已有一批存量债务，硬拦会让 CI 立刻全红。
// 因此脚本把「当前所有违规」按「文件 + 规则」计数写进 .ui-guard-baseline.json：
//   - 默认运行：只对「不在基线里的新增违规」返回退出码 1，并打印 新增/既有；
//   - --update-baseline：重写基线（债务只增不减必须显式记账，禁止静默漂移）。
// 白名单：docs/architecture/exceptions.md 里出现的文件路径一律豁免（简单
// 字符串匹配；宪章 §8 登记过的破例才有效）。
//
// 无第三方依赖；注释内容不算违规（先做注释掩码再匹配，行号不变）。
// ============================================================================

import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { basename, dirname, extname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const BASELINE_FILE = ".ui-guard-baseline.json";
const EXCEPTIONS_FILE = "docs/architecture/exceptions.md";
const LINE_BUDGET = 300;

// 行数预算扫描范围（features / ui 暂不存在时跳过）。
const SOURCE_DIRS = ["src/app", "src/features", "src/ui"];
// 禁止模式（产品分支 / invoke / 跨 feature 深路径）扫描范围。
const PATTERN_DIRS = ["src/app", "src/features"];
// 字面色值扫描范围（tokens.css 例外）。styles/ 之外的聚合入口
// src/styles.css 是历史盲区（P5-1b 补入）：它承载 .product-badge 家族，
// 只扫目录会让「根目录入口」的色值逃过检查。
const STYLE_DIR = "src/styles";
const STYLE_ENTRY_FILES = ["src/styles.css"];
const TOKEN_FILE = "tokens.css";

const CODE_EXTENSIONS = new Set([".ts", ".tsx", ".js", ".jsx"]);
const STYLE_EXTENSIONS = new Set([".css"]);

const RULES = {
  "file-line-budget": "文件行数预算（上限 " + LINE_BUDGET + " 行）",
  "forbidden-product-branch": "禁止产品整页分支判断",
  "literal-color": "字面色值（tokens.css 之外）",
  "cross-feature-import": "跨 feature 深路径 import",
  "component-invoke": "组件内直接 invoke(",
};
const RULE_ORDER = Object.keys(RULES);

// 产品分支三连（宪章 §4.1）。注释已掩码，只匹配真实代码。
const PRODUCT_PATTERNS = [
  { label: "isWorkBuddy ?", re: /\bisWorkBuddy\s*\?/ },
  { label: 'productId === "workbuddy"', re: /productId\s*===\s*["']workbuddy["']/ },
  { label: "WORKBUDDY_PRODUCT_ID", re: /\bWORKBUDDY_PRODUCT_ID\b/ },
];
// 组件内直接 invoke（import { invoke } 不算调用）。
const INVOKE_RE = /\binvoke\s*\(/;
// 字面色值：#hex（3-8 位）/ rgb( / hsl(（含 rgba/hsla）。
const HEX_COLOR_RE = /#(?:[0-9a-fA-F]{3,8})\b/;
const COLOR_FN_RE = /\b(?:rgb|hsl)a?\s*\(/;
// 跨 feature 深路径：from "…features/<name>/<非 public>…"（含动态 import）。
const CROSS_FEATURE_RE = /(?:from\s+|import\s*\(\s*)["']([^"']*\bfeatures\/[a-z-]+\/(?!public)[^"']*)["']/g;

// ============================================================================
// 文件遍历与注释掩码
// ============================================================================

/** 列出目录下源码文件（绝对路径，排序稳定）；目录缺失时 missing = true。 */
function listFiles(dirRel, extensions) {
  const absDir = join(ROOT, dirRel);
  if (!existsSync(absDir)) return { files: [], missing: true };
  const files = [];
  const stack = [absDir];
  while (stack.length > 0) {
    const current = stack.pop();
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      if (entry.name.startsWith(".") || entry.name === "node_modules") continue;
      const child = join(current, entry.name);
      if (entry.isDirectory()) stack.push(child);
      else if (entry.isFile() && extensions.has(extname(entry.name).toLowerCase())) files.push(child);
    }
  }
  return { files: files.sort(), missing: false };
}

/** 仓库相对路径（统一 posix 分隔符，基线跨平台可比）。 */
function toRel(absPath) {
  return relative(ROOT, absPath).split(sep).join("/");
}

/** 行数（末尾换行不额外计一行）。 */
function countLines(source) {
  const lines = source.split(/\r?\n/);
  if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
  return lines.length;
}

/**
 * 注释掩码：把注释字符替换为空格（保留换行，行号不变），字符串字面量原样
 * 保留。lineComments=false 用于 CSS（CSS 无 // 行注释，避免把 url:// 当注释）。
 * 这样「注释里提到 isWorkBuddy ?」不会误判，真实代码照旧命中。
 */
function maskComments(source, lineComments) {
  const out = source.split("");
  const BACKTICK = "\x60";
  let state = "code";
  for (let i = 0; i < source.length; i += 1) {
    const ch = source[i];
    const next = source[i + 1];
    if (state === "code") {
      // 行注释：前面紧跟冒号的 // 是协议前缀（https://），不当作注释。
      if (lineComments && ch === "/" && next === "/" && source[i - 1] !== ":") {
        state = "line";
        out[i] = " ";
        out[i + 1] = " ";
        i += 1;
        continue;
      }
      if (ch === "/" && next === "*") {
        state = "block";
        out[i] = " ";
        out[i + 1] = " ";
        i += 1;
        continue;
      }
      if (ch === "'") state = "single";
      else if (ch === '"') state = "double";
      else if (ch === BACKTICK) state = "template";
      continue;
    }
    if (state === "line") {
      if (ch === "\n") state = "code";
      else out[i] = " ";
      continue;
    }
    if (state === "block") {
      if (ch === "*" && next === "/") {
        out[i] = " ";
        out[i + 1] = " ";
        i += 1;
        state = "code";
      } else if (ch !== "\n") {
        out[i] = " ";
      }
      continue;
    }
    // 字符串状态：保留原文，只跟踪转义与结束引号。
    if (ch === "\\") i += 1;
    else if (state === "single" && ch === "'") state = "code";
    else if (state === "double" && ch === '"') state = "code";
    else if (state === "template" && ch === BACKTICK) state = "code";
  }
  return out.join("");
}

/**
 * 字符串内容掩码：把字符串/模板字面量的内容替换为空格（保留引号与换行）。
 * 仅用于「组件内 invoke(」判定——避免把 `const s = "invoke("` 这类字符串
 * 字面量误判成真实调用；跨 feature 深路径规则需要读字符串内容，故不套用。
 */
function maskStringBodies(source) {
  const out = source.split("");
  const BACKTICK = "\x60";
  let state = "code";
  for (let i = 0; i < source.length; i += 1) {
    const ch = source[i];
    if (state === "code") {
      if (ch === "'") state = "single";
      else if (ch === '"') state = "double";
      else if (ch === BACKTICK) state = "template";
      continue;
    }
    if (ch === "\\") {
      out[i] = " ";
      if (i + 1 < source.length) out[i + 1] = " ";
      i += 1;
      continue;
    }
    if (state === "single" && ch === "'") state = "code";
    else if (state === "double" && ch === '"') state = "code";
    else if (state === "template" && ch === BACKTICK) state = "code";
    else if (ch !== "\n") out[i] = " ";
  }
  return out.join("");
}

// ============================================================================
// 白名单（docs/architecture/exceptions.md 中出现的文件路径）
// ============================================================================

function loadWhitelist() {
  const abs = join(ROOT, EXCEPTIONS_FILE);
  if (!existsSync(abs)) return { paths: new Set(), text: "" };
  const text = readFileSync(abs, "utf8");
  const paths = new Set();
  const re = /src\/[A-Za-z0-9_./-]+/g;
  let match = re.exec(text);
  while (match !== null) {
    paths.add(match[0].replace(/[.,;]+$/, ""));
    match = re.exec(text);
  }
  return { paths, text };
}

function isWhitelisted(file, whitelist) {
  if (whitelist.paths.has(file)) return true;
  // 简单字符串匹配兜底：路径出现在例外文件正文里即豁免。
  return whitelist.text.includes(file);
}

// ============================================================================
// 规则执行
// ============================================================================

const violations = [];

function record(file, line, rule, text) {
  violations.push({ file, line, rule, text: String(text || "").trim().slice(0, 120) });
}

function runLineBudget(scanned) {
  for (const dirRel of SOURCE_DIRS) {
    const { files, missing } = listFiles(dirRel, CODE_EXTENSIONS);
    scanned.push({ dir: dirRel, count: files.length, missing });
    for (const abs of files) {
      const lines = countLines(readFileSync(abs, "utf8"));
      if (lines > LINE_BUDGET) {
        // 文件级违规：定位记第 1 行，正文写明实际行数与预算。
        record(toRel(abs), 1, "file-line-budget", "共 " + lines + " 行（上限 " + LINE_BUDGET + "）");
      }
    }
  }
}

function runPatternRules(scanned) {
  for (const dirRel of PATTERN_DIRS) {
    const { files, missing } = listFiles(dirRel, CODE_EXTENSIONS);
    scanned.push({ dir: dirRel, count: files.length, missing, patterns: true });
    for (const abs of files) {
      const file = toRel(abs);
      const source = readFileSync(abs, "utf8");
      const masked = maskComments(source, true);
      const lines = masked.split(/\r?\n/);
      // invoke 判定用「注释 + 字符串内容」双掩码：字符串里的 "invoke(" 不算调用。
      const invokeLines = maskStringBodies(masked).split(/\r?\n/);
      for (let index = 0; index < lines.length; index += 1) {
        const line = lines[index];
        for (const pattern of PRODUCT_PATTERNS) {
          if (pattern.re.test(line)) record(file, index + 1, "forbidden-product-branch", pattern.label);
        }
        if (INVOKE_RE.test(invokeLines[index] || "")) record(file, index + 1, "component-invoke", line.trim());
        CROSS_FEATURE_RE.lastIndex = 0;
        let match = CROSS_FEATURE_RE.exec(line);
        while (match !== null) {
          record(file, index + 1, "cross-feature-import", match[1]);
          match = CROSS_FEATURE_RE.exec(line);
        }
      }
    }
  }
}

function runLiteralColors(scanned) {
  const { files, missing } = listFiles(STYLE_DIR, STYLE_EXTENSIONS);
  scanned.push({ dir: STYLE_DIR, count: files.length, missing });
  // 聚合入口（目录外单文件）单独计入扫描域，缺失时同样跳过。
  for (const entryRel of STYLE_ENTRY_FILES) {
    const absEntry = join(ROOT, entryRel);
    const present = existsSync(absEntry);
    scanned.push({ dir: entryRel, count: present ? 1 : 0, missing: !present });
    if (present) files.push(absEntry);
  }
  for (const abs of files) {
    if (basename(abs).toLowerCase() === TOKEN_FILE) continue;
    const file = toRel(abs);
    // CSS 只有块注释；注释里提到色值不算违规。
    const masked = maskComments(readFileSync(abs, "utf8"), false);
    const lines = masked.split(/\r?\n/);
    for (let index = 0; index < lines.length; index += 1) {
      const line = lines[index];
      if (HEX_COLOR_RE.test(line) || COLOR_FN_RE.test(line)) {
        record(file, index + 1, "literal-color", line.trim());
      }
    }
  }
}

// ============================================================================
// 基线读写与比对
// ============================================================================

function keyOf(item) {
  return item.file + "::" + item.rule;
}

function countByFileRule(items) {
  const counts = new Map();
  for (const item of items) {
    const key = keyOf(item);
    counts.set(key, (counts.get(key) || 0) + 1);
  }
  return counts;
}

function loadBaseline() {
  const abs = join(ROOT, BASELINE_FILE);
  if (!existsSync(abs)) return { entries: new Map(), exists: false };
  let parsed;
  try {
    parsed = JSON.parse(readFileSync(abs, "utf8"));
  } catch (error) {
    const message = error && error.message ? error.message : String(error);
    console.error("[ui-guard] 基线文件无法解析：" + BASELINE_FILE + "（" + message + "）");
    process.exit(2);
  }
  const entries = new Map();
  for (const entry of Array.isArray(parsed.entries) ? parsed.entries : []) {
    if (!entry || typeof entry.file !== "string" || typeof entry.rule !== "string") continue;
    const count = Number(entry.count) || 0;
    if (count > 0) entries.set(entry.file + "::" + entry.rule, count);
  }
  return { entries, exists: true };
}

function writeBaseline(counts) {
  const entries = [];
  for (const [key, count] of counts.entries()) {
    const splitAt = key.lastIndexOf("::");
    entries.push({ file: key.slice(0, splitAt), rule: key.slice(splitAt + 2), count });
  }
  entries.sort((a, b) => (a.file === b.file ? a.rule.localeCompare(b.rule) : a.file.localeCompare(b.file)));
  const payload = {
    version: 1,
    generatedBy: "scripts/ui-guard.mjs --update-baseline",
    note: "UI-CHARTER §3/§4 债务基线（按「文件 + 规则」计数）。债务只允许收敛；新增违规必须修掉或按宪章 §8 登记例外，禁止重写基线掩盖。",
    totalViolations: entries.reduce((sum, entry) => sum + entry.count, 0),
    entries,
  };
  writeFileSync(join(ROOT, BASELINE_FILE), JSON.stringify(payload, null, 2) + "\n", "utf8");
  return payload;
}

// ============================================================================
// 主流程
// ============================================================================

function main() {
  const args = process.argv.slice(2);
  const updateBaseline = args.includes("--update-baseline");

  const scanned = [];
  runLineBudget(scanned);
  runPatternRules(scanned);
  runLiteralColors(scanned);

  const whitelist = loadWhitelist();
  const exempt = violations.filter((item) => isWhitelisted(item.file, whitelist));
  const active = violations.filter((item) => !isWhitelisted(item.file, whitelist));
  const currentCounts = countByFileRule(active);

  // 同一目录可能被行数预算与禁止模式两条规则各扫一次：合并为一行展示。
  const scopeByDir = new Map();
  for (const entry of scanned) {
    const previous = scopeByDir.get(entry.dir);
    if (previous === undefined || (previous.missing && !entry.missing)) {
      scopeByDir.set(entry.dir, entry);
    }
  }
  const scope = [...scopeByDir.values()]
    .map((entry) => entry.dir + (entry.missing ? "（缺失，跳过）" : "（" + entry.count + " 文件）"))
    .join(" · ");
  console.log("[ui-guard] 扫描：" + scope);
  console.log("[ui-guard] 规则：" + RULE_ORDER.map((rule) => RULES[rule]).join(" / "));
  console.log(
    "[ui-guard] 白名单：" + EXCEPTIONS_FILE + " 豁免 " + exempt.length + " 处（" +
      new Set(exempt.map((item) => item.file)).size + " 个文件）",
  );

  if (updateBaseline) {
    const payload = writeBaseline(currentCounts);
    console.log(
      "[ui-guard] 基线已写入 " + BASELINE_FILE + "：" + payload.entries.length +
        " 条目 / " + payload.totalViolations + " 处违规",
    );
    return 0;
  }

  // 逐条比对基线：每条「文件+规则」有 count 次豁免，用完之后即为新增。
  const budget = new Map(loadBaseline().entries);
  const fresh = [];
  for (const item of active) {
    const key = keyOf(item);
    const allowed = budget.get(key) || 0;
    if (allowed > 0) budget.set(key, allowed - 1);
    else fresh.push(item);
  }
  const freshSet = new Set(fresh);

  // 违规明细（按文件分组，新增项前缀 +）。
  const byFile = new Map();
  for (const item of active) {
    if (!byFile.has(item.file)) byFile.set(item.file, []);
    byFile.get(item.file).push(item);
  }
  if (byFile.size > 0) {
    console.log("[ui-guard] 违规明细：");
    let printed = 0;
    const limit = 80;
    for (const [file, items] of byFile) {
      if (printed >= limit) break;
      const freshCount = items.filter((item) => freshSet.has(item)).length;
      console.log(
        file + "（" + items.length + " 处" + (freshCount > 0 ? "，新增 " + freshCount : "") + "）",
      );
      printed += 1;
      for (const item of items) {
        if (printed >= limit) break;
        const mark = freshSet.has(item) ? "+ " : "  ";
        console.log("  " + mark + ":" + item.line + " [" + item.rule + "] " + item.text);
        printed += 1;
      }
    }
    if (printed >= limit) console.log("… 明细过长，其余省略（用 --update-baseline 前请先修违规）");
  }

  const existing = active.length - fresh.length;
  console.log("[ui-guard] 违规 " + active.length + " 处 / " + currentCounts.size + " 个「文件+规则」条目");
  console.log("[ui-guard] 新增 " + fresh.length + " / 既有 " + existing);

  const baseline = loadBaseline();
  if (!baseline.exists) {
    console.log(
      "[ui-guard] 基线缺失（" + BASELINE_FILE + "）：当前所有违规都按新增计算；" +
        "先运行 pnpm guard -- --update-baseline 建立基线。",
    );
  } else {
    let shrunk = 0;
    for (const [key, count] of baseline.entries.entries()) {
      const now = currentCounts.get(key) || 0;
      if (now < count) shrunk += count - now;
    }
    console.log(
      "[ui-guard] 基线 " + BASELINE_FILE + "：" + baseline.entries.size + " 条目" +
        (shrunk > 0 ? "（已收敛 " + shrunk + " 处，可 --update-baseline 收缩）" : ""),
    );
  }

  if (fresh.length > 0) {
    console.log(
      "[ui-guard] 失败：新增 " + fresh.length + " 处违规不在基线内" +
        "（修掉它们，或按宪章 §8 在 " + EXCEPTIONS_FILE + " 登记例外）",
    );
    return 1;
  }
  console.log("[ui-guard] 通过：无新增债务（既有 " + existing + " 处已在基线登记）");
  return 0;
}

process.exit(main());
