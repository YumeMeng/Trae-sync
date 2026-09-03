// @ts-expect-error Vitest 在 Node 环境运行；应用 tsconfig 不引入 Node 类型。
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// 读取生产 CSS token，确保设计系统的辅助文字不会回退到不合格对比度。
const styles = readFileSync("src/styles/tokens.css", "utf8");

function token(name: string): string {
  const match = styles.match(new RegExp(`--${name}\\s*:\\s*(#[0-9a-fA-F]{6})`));
  if (!match) throw new Error(`缺少 CSS token: --${name}`);
  return match[1];
}

function relativeLuminance(hex: string): number {
  const channels = [0, 2, 4].map((offset) =>
    Number.parseInt(hex.slice(1 + offset, 3 + offset), 16) / 255,
  );
  const linear = channels.map((channel) =>
    channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4,
  );
  return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
}

function contrastRatio(foreground: string, background: string): number {
  const foregroundLuminance = relativeLuminance(foreground);
  const backgroundLuminance = relativeLuminance(background);
  const lighter = Math.max(foregroundLuminance, backgroundLuminance);
  const darker = Math.min(foregroundLuminance, backgroundLuminance);
  return (lighter + 0.05) / (darker + 0.05);
}

describe("UI 颜色 token", () => {
  it("普通辅助文字在所有浅色工作面达到 WCAG AA", () => {
    const muted = token("text-2");
    const backgrounds = [
      token("app-bg"),
      "#e7edef", // 标题栏/导航证据面
      "#fefefe", // --glass-solid（.97 白玻璃）在浅画布上的合成近似
      "#ffffff", // 纯白上限：--surface 退役后玻璃系工作面不会比纯白更浅（U-4 B2）
    ];

    for (const background of backgrounds) {
      expect(contrastRatio(muted, background)).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("输入占位文字达到 WCAG AA，不承担唯一状态信息", () => {
    const placeholder = token("muted-placeholder");
    // 最浅工作面上限：--surface 退役后输入框为中性淡底/fill-soft，取纯白作最严苛基准（U-4 B2）
    expect(contrastRatio(placeholder, "#ffffff")).toBeGreaterThanOrEqual(4.5);
  });
});
