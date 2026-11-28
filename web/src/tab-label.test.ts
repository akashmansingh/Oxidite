import { describe, expect, test } from "bun:test";
import { formatTabLabel } from "./tab-label";

describe("formatTabLabel", () => {
  test("prefers active Agentic CLI label and sets full cwd tooltip and agent logo", () => {
    const out = formatTabLabel(
      {
        cwd: "C:\\Work\\Projects\\Oxidite",
        homeDir: "C:\\Users\\akash",
        active_agent: "claude",
        agent_label: "Claude Code",
        shell_kind: "powershell",
      },
      true,
    );
    expect(out).toEqual({
      text: "Claude Code",
      tooltip: "Claude Code — C:\\Work\\Projects\\Oxidite",
      agentId: "claude",
      logoId: "claude",
    });
  });

  test("shows full path on desktop and compacts deep paths on mobile without losing full tooltip", () => {
    const desktop = formatTabLabel(
      {
        cwd: "C:\\Users\\akash",
        homeDir: "C:\\Users\\akash",
        shell_kind: "powershell",
      },
      false,
    );
    expect(desktop.text).toBe("C:\\Users\\akash");
    expect(desktop.logoId).toBe("powershell");

    const out = formatTabLabel(
      {
        cwd: "C:\\Work\\Projects\\Oxidite",
        homeDir: "C:\\Users\\akash",
        shell_kind: "bash",
      },
      true,
    );
    expect(out.text).toBe("Projects/Oxidite");
    expect(out.tooltip).toBe("C:\\Work\\Projects\\Oxidite");
    expect(out.agentId).toBeNull();
    expect(out.logoId).toBeNull();
  });

  test("omits PowerShell logo on generic shell and SSH tabs", () => {
    const sshTab = formatTabLabel({ cwd: null, shell_kind: "ssh" }, false);
    expect(sshTab.logoId).toBeNull();

    const bashTab = formatTabLabel({ cwd: "/home/user/repo", shell_kind: "bash" }, false);
    expect(bashTab.logoId).toBeNull();

    const unknownTab = formatTabLabel({ cwd: "/tmp" }, false);
    expect(unknownTab.logoId).toBeNull();
  });
});
