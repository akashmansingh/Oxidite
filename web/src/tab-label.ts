export interface TabSessionMeta {
  cwd?: string | null;
  homeDir?: string | null;
  active_agent?: string | null;
  agent_label?: string | null;
}

export interface FormattedTabLabel {
  text: string;
  tooltip: string;
  agentId: string | null;
}

/**
 * Formats a session tab label:
 * - Prefers active Agentic CLI label (`Codex`, `Claude Code`, `Devin`, `Cursor`, `Antigravity`).
 * - Otherwise replaces user home prefix with `~` and compacts deep paths on narrow screens.
 * - Never leaves "local shell" once a CWD is known.
 */
export function formatTabLabel(meta: TabSessionMeta, isCompact: boolean): FormattedTabLabel {
  const fullPath = meta.cwd?.trim() || "";

  if (meta.active_agent) {
    const label = meta.agent_label?.trim() || meta.active_agent;
    return {
      text: label,
      tooltip: fullPath ? `${label} — ${fullPath}` : label,
      agentId: meta.active_agent,
    };
  }

  if (!fullPath) {
    return { text: "shell", tooltip: "Local shell", agentId: null };
  }

  if (!isCompact) {
    return { text: fullPath, tooltip: fullPath, agentId: null };
  }

  const segments = fullPath.split(/[\\/]/).filter(Boolean);
  if (segments.length <= 2) {
    return { text: fullPath, tooltip: fullPath, agentId: null };
  }
  const tail = segments.slice(-2).join("/");
  return { text: tail, tooltip: fullPath, agentId: null };
}

export function abbreviateHome(path: string, homeDir?: string): string {
  if (homeDir) {
    const normHome = homeDir.replace(/[\\/]+$/, "");
    if (path.toLowerCase() === normHome.toLowerCase()) return "~";
    if (
      path.toLowerCase().startsWith(normHome.toLowerCase() + "\\") ||
      path.toLowerCase().startsWith(normHome.toLowerCase() + "/")
    ) {
      return "~" + path.slice(normHome.length);
    }
  }
  // Heuristic fallback for Windows C:\Users\<name>
  const winMatch = path.match(/^[A-Za-z]:\\Users\\[^\\]+(.*)$/i);
  if (winMatch) {
    return "~" + (winMatch[1] || "");
  }
  return path;
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
