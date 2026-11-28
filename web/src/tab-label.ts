export interface TabSessionMeta {
  cwd?: string | null;
  homeDir?: string | null;
  active_agent?: string | null;
  agent_label?: string | null;
  shell_kind?: string | null;
}

export interface FormattedTabLabel {
  text: string;
  tooltip: string;
  agentId: string | null;
  logoId: string | null;
}

/**
 * Formats a session tab label:
 * - Prefers active Agentic CLI label (`Codex`, `Claude Code`, `Devin`, `Cursor`, `Antigravity`).
 * - Otherwise displays the full working directory path on wide screens and compacts deep paths on narrow screens.
 * - Resolves `logoId` from active agent or reported shell (`powershell`), returning `null` for generic shells/SSH.
 */
export function formatTabLabel(meta: TabSessionMeta, isCompact: boolean): FormattedTabLabel {
  const fullPath = meta.cwd?.trim() || "";
  const shellLogo = meta.shell_kind === "powershell" ? "powershell" : null;

  if (meta.active_agent) {
    const label = meta.agent_label?.trim() || meta.active_agent;
    return {
      text: label,
      tooltip: fullPath ? `${label} — ${fullPath}` : label,
      agentId: meta.active_agent,
      logoId: meta.active_agent,
    };
  }

  if (!fullPath) {
    return {
      text: meta.shell_kind === "powershell" ? "powershell" : "shell",
      tooltip: meta.shell_kind === "powershell" ? "PowerShell" : "Local shell",
      agentId: null,
      logoId: shellLogo,
    };
  }

  if (!isCompact) {
    return { text: fullPath, tooltip: fullPath, agentId: null, logoId: shellLogo };
  }

  const segments = fullPath.split(/[\\/]/).filter(Boolean);
  if (segments.length <= 2) {
    return { text: fullPath, tooltip: fullPath, agentId: null, logoId: shellLogo };
  }
  const tail = segments.slice(-2).join("/");
  return { text: tail, tooltip: fullPath, agentId: null, logoId: shellLogo };
}

/**
 * Renders an `<img class="tab-logo">` with automatic fallback to a clean monogram badge
 * if the user hasn't dropped the official SVG into `web/public/logos/` yet.
 */
export function createAgentLogoElement(agentId: string, label: string): HTMLElement {
  const wrapper = document.createElement("span");
  wrapper.style.display = "inline-flex";
  wrapper.style.alignItems = "center";

  const img = document.createElement("img");
  img.className = "tab-logo";
  img.src = `/logos/${encodeURIComponent(agentId)}.svg`;
  img.alt = label;

  img.addEventListener("error", () => {
    img.remove();
    const badge = document.createElement("span");
    badge.className = "tab-logo-fallback";
    badge.textContent = label.slice(0, 1).toUpperCase();
    wrapper.append(badge);
  });

  wrapper.append(img);
  return wrapper;
}
