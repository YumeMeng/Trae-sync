// ============================================================================
// CommandName golden 测试（P12-R1c，Q4 定案：构建期生成 + golden 断言）
// ============================================================================
// 在 vitest 里重新读 src-tauri/src/lib.rs 现算一遍命令清单，与
// src/types/commandNames.ts（生成物）比对。不一致即红，提示跑 pnpm gen:commands。
// 提取逻辑刻意独立实现（不 import 生成脚本）：两份实现交叉验证，
// 脚本提取逻辑坏了 golden 也能抓住。
//
// 注意：不 import scripts/gen-command-union.mjs 还有一个工程原因——
// tsconfig.app.json 覆盖 tests 且 strict 模式，无类型的 .mjs 会报 TS7016。

/// <reference types="node" />
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * golden 侧的第二实现：从 lib.rs 源码提取 generate_handler![...] 清单。
 * 与脚本同一套处理规则：定位宏 → 括号深度扫描（跳过注释/字符串）→
 * 剥注释按逗号拆分 → 取路径最后一段（Tauri 2 invoke 名 = 函数名）→ 去重排序。
 */
function extractNamesFromLibRs(source: string): string[] {
  const macroStart = source.search(/generate_handler!\s*\[/);
  if (macroStart === -1) throw new Error("lib.rs 中未找到 generate_handler![...]");
  const openBracket = source.indexOf("[", macroStart);

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
    } else if (inBlockComment) {
      if (ch === "*" && next === "/") {
        inBlockComment = false;
        i++;
      }
    } else if (inString) {
      if (ch === "\\") i++;
      else if (ch === '"') inString = false;
    } else if (ch === "/" && next === "/") {
      inLineComment = true;
      i++;
    } else if (ch === "/" && next === "*") {
      inBlockComment = true;
      i++;
    } else if (ch === '"') {
      inString = true;
    } else if (ch === "[") {
      depth++;
    } else if (ch === "]") {
      depth--;
      if (depth === 0) break;
    }
  }
  if (i >= source.length) throw new Error("generate_handler![...] 括号未闭合");
  const body = source.slice(openBracket + 1, i);
  const names = body
    .replace(/\/\/[^\n]*/g, "")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean)
    .map((item) => item.split("::").pop() ?? "")
    .filter(Boolean);
  if (names.length === 0) throw new Error("generate_handler![...] 清单提取为空");
  return [...new Set(names)].sort();
}

/** 从生成文件文本解析出名字清单（只认引号内的标识符字面量）。 */
function parseNamesFromGeneratedFile(text: string): string[] {
  return [...text.matchAll(/"([A-Za-z_][A-Za-z0-9_]*)"/g)].map((m) => m[1]);
}

// 读文件用项目根相对路径（vitest 从项目根启动，process.cwd() 即根目录），
// 与 uiDesignTokens.test.ts 的既有模式一致；jsdom 环境下 import.meta.url 非
// file:// scheme，不能用 URL 形式喂给 node:fs。
const libRsSource = readFileSync("src-tauri/src/lib.rs", "utf8");

let generatedText: string;
try {
  generatedText = readFileSync("src/types/commandNames.ts", "utf8");
} catch {
  throw new Error("src/types/commandNames.ts 不存在，请先运行 pnpm gen:commands 生成");
}

describe("CommandName golden（lib.rs ↔ src/types/commandNames.ts）", () => {
  const expected = extractNamesFromLibRs(libRsSource);
  const actual = parseNamesFromGeneratedFile(generatedText);

  it("generate_handler! 清单非空（防 lib.rs 结构变化导致静默空生成）", () => {
    expect(expected.length).toBeGreaterThan(0);
  });

  it("生成文件与 lib.rs 现算清单一致（漂移即红）", () => {
    const missing = expected.filter((name) => !actual.includes(name));
    const extra = actual.filter((name) => !expected.includes(name));
    if (missing.length > 0 || extra.length > 0) {
      const lines = [
        "src/types/commandNames.ts 与 lib.rs 的 generate_handler! 清单不一致。",
        "请运行 pnpm gen:commands 重新生成 src/types/commandNames.ts。",
      ];
      if (missing.length > 0) lines.push(`缺失（lib.rs 有、生成文件无）：${missing.join(", ")}`);
      if (extra.length > 0) lines.push(`多余（生成文件有、lib.rs 无）：${extra.join(", ")}`);
      throw new Error(lines.join("\n"));
    }
    // 内容一致后再比顺序（两边都应为字典序，防止生成物乱序漂移）。
    expect(actual).toEqual(expected);
  });

  it("生成文件结构合法：GENERATED 头 + 无重复项", () => {
    expect(generatedText).toContain("GENERATED FILE");
    expect(generatedText).toContain("export type CommandName =");
    const names = parseNamesFromGeneratedFile(generatedText);
    expect(new Set(names).size).toBe(names.length);
  });
});
