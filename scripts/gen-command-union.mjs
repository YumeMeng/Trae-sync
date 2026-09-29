// ============================================================================
// 命令名联合类型构建期生成脚本（P12-R1c，Q4 定案：构建期生成）
// ============================================================================
// 从 src-tauri/src/lib.rs 的 tauri::generate_handler![...] 宏中提取命令清单，
// 生成前端 invoke 命令名联合类型 src/types/commandNames.ts。
// 零依赖，node >= 18。用法：node scripts/gen-command-union.mjs（或 pnpm gen:commands）。
// 本脚本对 src-tauri/** 只读；漂移检测由 tests/commandNames.test.ts（golden）兜底。

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, "..");
const libRsPath = resolve(repoRoot, "src-tauri/src/lib.rs");
const outPath = resolve(repoRoot, "src/types/commandNames.ts");

/**
 * 从 lib.rs 源码提取 generate_handler! 宏内的命令名清单（去重、排序）。
 * 处理要点（与 lib.rs 实际写法对应）：跨多行逗号分隔；含 // 行注释；
 * 兼容 `module::cmd` 路径与裸 `cmd`（Tauri 2 invoke 名 = 函数名，取路径最后一段）。
 */
export function extractCommandNames(source) {
  // 定位宏起点：兼容 `generate_handler![` 与 `generate_handler! [`（! 与 [ 之间可有空白）。
  const macroStart = source.search(/generate_handler!\s*\[/);
  if (macroStart === -1) {
    throw new Error("未在 lib.rs 中找到 generate_handler![...] 宏调用，结构可能已变化");
  }
  const openBracket = source.indexOf("[", macroStart);

  // 括号深度扫描：跳过行注释/块注释/字符串字面量，防止注释里的 "]" 提前截断，
  // 也兼容未来清单里出现嵌套方括号的写法。
  let depth = 0;
  let inLineComment = false;
  let inBlockComment = false;
  let inString = false;
  let i = openBracket;
  for (; i < source.length; i++) {
    const ch = source[i];
    const next = source[i + 1];
    if (inLineComment) {
      if (ch === "\n") inLineComment = false;
      continue;
    }
    if (inBlockComment) {
      if (ch === "*" && next === "/") {
        inBlockComment = false;
        i++;
      }
      continue;
    }
    if (inString) {
      if (ch === "\\") i++;
      else if (ch === '"') inString = false;
      continue;
    }
    if (ch === "/" && next === "/") {
      inLineComment = true;
      i++;
      continue;
    }
    if (ch === "/" && next === "*") {
      inBlockComment = true;
      i++;
      continue;
    }
    if (ch === '"') {
      inString = true;
      continue;
    }
    if (ch === "[") depth++;
    else if (ch === "]") {
      depth--;
      if (depth === 0) break;
    }
  }
  if (i >= source.length) {
    throw new Error("generate_handler![...] 宏括号未闭合，lib.rs 结构可能已变化");
  }
  const body = source.slice(openBracket + 1, i);

  // 剥离注释后按逗号拆分；宏体内每项是命令路径（标识符 + 可选 :: 前缀），无内嵌逗号。
  const cleaned = body.replace(/\/\/[^\n]*/g, "").replace(/\/\*[\s\S]*?\*\//g, "");
  const names = cleaned
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean)
    // Tauri 2 惯例：invoke 命令名 = 函数名（路径最后一段）。已核对项目前端
    // invoke 调用全部为裸函数名，与该转换一致。
    .map((item) => item.split("::").pop() ?? "")
    .filter(Boolean);
  for (const name of names) {
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) {
      throw new Error(`提取到非法命令名 "${name}"，请检查 lib.rs 的 generate_handler! 清单`);
    }
  }
  if (names.length === 0) {
    throw new Error("generate_handler![...] 清单提取结果为空，拒绝生成空 union");
  }
  return [...new Set(names)].sort();
}

/** 渲染 commandNames.ts 内容：每命令一行，diff 友好（新增命令 = 单行 diff）。 */
export function renderTypeFile(names) {
  const lines = [
    "// GENERATED FILE —— 本文件由 scripts/gen-command-union.mjs 自动生成，勿手改。",
    `// 来源：src-tauri/src/lib.rs 的 tauri::generate_handler![...] 命令清单（${names.length} 条）。`,
    "// 重新生成：pnpm gen:commands。漂移检测：tests/commandNames.test.ts（golden 比对）。",
    "",
    "/** 前端可 invoke 的命令名集合（Tauri 2 惯例：invoke 名 = 后端函数名）。 */",
    "export type CommandName =",
  ];
  for (const name of names) {
    lines.push(`  | "${name}"`);
  }
  lines.push(";", "");
  return lines.join("\n");
}

function main() {
  if (!existsSync(libRsPath)) {
    throw new Error(`未找到 ${libRsPath}，请在仓库根目录运行本脚本`);
  }
  const names = extractCommandNames(readFileSync(libRsPath, "utf8"));
  writeFileSync(outPath, renderTypeFile(names), "utf8");
  console.log(`已生成 ${outPath}`);
  console.log(`共 ${names.length} 个命令（已去重排序）`);
}

// 仅直接执行本脚本时运行主流程；被 golden 测试导入时只提供函数，不产生副作用。
const invokedDirectly =
  process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href;
if (invokedDirectly) {
  main();
}
