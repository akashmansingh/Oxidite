import { describe, expect, test } from "bun:test";
import { renderWordmark } from "./wordmark";

function maxLineLen(s: string): number {
  return Math.max(...s.split("\n").map((l) => [...l].length));
}

describe("renderWordmark", () => {
  test("renders wide outlined block art when folder fits", () => {
    const res = renderWordmark("proj", 60);
    expect(res.isArt).toBe(true);
    expect(res.tier).toBe("wide");
    expect(res.text).toContain("█");
    expect(res.text).toContain("╔");
    expect(maxLineLen(res.text)).toBeLessThanOrEqual(60);
  });

  test("steps down to compact art for medium-length names", () => {
    const res = renderWordmark("Oxidite", 50);
    expect(res.isArt).toBe(true);
    expect(res.tier).toBe("compact");
    expect(maxLineLen(res.text)).toBeLessThanOrEqual(50);
  });

  test("truncates cleanly without exceeding maxCols on long names", () => {
    const res = renderWordmark("my-super-long-microservice-repo", 36);
    expect(res.isArt).toBe(true);
    expect(res.tier).toBe("truncated");
    expect(maxLineLen(res.text)).toBeLessThanOrEqual(36);
  });

  test("falls back to wide-tracked bold text on very narrow viewports", () => {
    const res = renderWordmark("Oxidite", 18);
    expect(res.isArt).toBe(false);
    expect(res.tier).toBe("fallback");
    expect(res.text).toBe("O X I D I T E");
  });

  test("preserves non-ASCII folder names cleanly without mangling", () => {
    const res = renderWordmark("プロジェクト", 80);
    expect(res.isArt).toBe(false);
    expect(res.tier).toBe("fallback");
    expect(res.text).toBe("プロジェクト");
  });
});
