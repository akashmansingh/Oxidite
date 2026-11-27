import { describe, expect, test } from "bun:test";
import { filterBlockLines, isEchoedCommandLine, isPromptLine, type RenderedLine } from "./blocks";

describe("blocks", () => {
  test("detects Windows PowerShell and Unix shell prompt lines", () => {
    expect(isPromptLine("PS C:\\Work\\Projects\\Oxidite>")).toBe(true);
    expect(isPromptLine("PS C:\\Work\\Projects\\Oxidite> ")).toBe(true);
    expect(isPromptLine("user@host:~/repo$ ")).toBe(true);
    expect(isPromptLine("On branch main")).toBe(false);
  });

  test("detects echoed command line with or without prompt prefix", () => {
    expect(isEchoedCommandLine("PS C:\\Work\\Projects> ls", "ls")).toBe(true);
    expect(isEchoedCommandLine("git status", "git status")).toBe(true);
    expect(isEchoedCommandLine("On branch main", "git status")).toBe(false);
  });

  test("filters echoed command and prompts while preserving clean lines", () => {
    const mk = (plain: string): RenderedLine => ({
      plain,
      spans: [{ text: plain, fg: null, bg: null, bold: false, dim: false, italic: false, underline: false }],
    });
    const input = [
      mk("PS C:\\Work\\Projects> ls"),
      mk("    Directory: C:\\Work\\Projects"),
      mk("Mode                 LastWriteTime         Length Name"),
      mk("PS C:\\Work\\Projects\\OrderFlow> "),
    ];
    const filtered = filterBlockLines(input, "ls");
    expect(filtered.map((l) => l.plain)).toEqual([
      "    Directory: C:\\Work\\Projects",
      "Mode                 LastWriteTime         Length Name",
    ]);
  });
});
