import { describe, expect, test } from "bun:test";
import { abbreviateHome, formatTabLabel } from "./tab-label";

describe("abbreviateHome", () => {
  test("replaces matching Windows and POSIX home directories with ~", () => {
    expect(abbreviateHome("C:\\Users\\akash", "C:\\Users\\akash")).toBe("~");
    expect(abbreviateHome("C:\\Users\\akash\\Projects\\Oxidite", "C:\\Users\\akash")).toBe("~\\Projects\\Oxidite");
    expect(abbreviateHome("/home/akash/code", "/home/akash")).toBe("~/code");
  });

  test("falls back cleanly when homeDir is absent", () => {
    expect(abbreviateHome("C:\\Users\\akash\\Desktop")).toBe("~\\Desktop");
    expect(abbreviateHome("C:\\Work\\Projects\\Oxidite")).toBe("C:\\Work\\Projects\\Oxidite");
  });
});

describe("formatTabLabel", () => {
  test("prefers active Agentic CLI label and sets full cwd tooltip", () => {
    const out = formatTabLabel(
      {
        cwd: "C:\\Work\\Projects\\Oxidite",
        homeDir: "C:\\Users\\akash",
        active_agent: "claude",
        agent_label: "Claude Code",
      },
      true,
    );
    expect(out).toEqual({
      text: "Claude Code",
      tooltip: "Claude Code — C:\\Work\\Projects\\Oxidite",
      agentId: "claude",
    });
  });

  test("shows full path on desktop and compacts deep paths on mobile without losing full tooltip", () => {
    const desktop = formatTabLabel(
      {
        cwd: "C:\\Users\\akash",
        homeDir: "C:\\Users\\akash",
      },
      false,
    );
    expect(desktop.text).toBe("C:\\Users\\akash");

    const out = formatTabLabel(
      {
        cwd: "C:\\Work\\Projects\\Oxidite",
        homeDir: "C:\\Users\\akash",
      },
      true,
    );
    expect(out.text).toBe("Projects/Oxidite");
    expect(out.tooltip).toBe("C:\\Work\\Projects\\Oxidite");
    expect(out.agentId).toBeNull();
  });
});
