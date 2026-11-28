import { renderWordmark } from "./wordmark";

export interface ProjectReport {
  wordmark: string;
  name: string;
  path: string;
  git_branch: string | null;
  language: string | null;
  frameworks: string[];
  pkg_manager: string | null;
  prod_deps: number | null;
  dev_deps: number | null;
  files: number;
  directories: number;
  truncated: boolean;
  scan_time_ms: number;
}

export interface BannerCommand {
  name: string;
  desc: string;
  onClick?: () => void;
}

export interface ProjectBannerOptions {
  fetchJson: <T>(url: string) => Promise<T>;
  commands: BannerCommand[];
  version?: string;
  helpHint?: string;
}

export interface ProjectBannerHandle {
  element: HTMLElement;
  refreshForSession: (sid: string, cwdHint?: string | null, forceRefresh?: boolean) => Promise<void>;
  clearCache: () => void;
  destroy: () => void;
}

/** Self-contained home-directory abbreviation (`C:\Users\<name>` -> `~`). */
export function abbreviateHomePath(path: string, homeDir?: string): string {
  if (!path) return "";
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
  const winMatch = path.match(/^[A-Za-z]:\\Users\\[^\\]+(.*)$/i);
  if (winMatch) return "~" + (winMatch[1] || "");
  const unixMatch = path.match(/^\/(?:home|Users)\/[^/]+(.*)$/);
  if (unixMatch) return "~" + (unixMatch[1] || "");
  return path;
}

export function createProjectBanner(opts: ProjectBannerOptions): ProjectBannerHandle {
  const root = document.createElement("section");
  root.className = "proj-banner";
  root.setAttribute("aria-label", "Project overview");

  const cache = new Map<string, ProjectReport>();
  let activeKey = "";
  let lastReport: ProjectReport | null = null;
  let lastRenderedCols = -1;

  function computeAvailableWordmarkCols(): number {
    const widthPx = root.clientWidth || (typeof window !== "undefined" ? window.innerWidth : 900);
    const isStacked = widthPx < 760;
    const colPx = isStacked ? widthPx - 44 : widthPx * 0.54 - 44;
    return Math.max(16, Math.floor(colPx / 7.2));
  }

  let resizeObserver: ResizeObserver | null = null;
  if (typeof ResizeObserver !== "undefined") {
    resizeObserver = new ResizeObserver(() => {
      if (!lastReport) return;
      const cols = computeAvailableWordmarkCols();
      if (cols !== lastRenderedCols) {
        renderReport(lastReport);
      }
    });
    resizeObserver.observe(root);
  }

  function renderLoading(cwdHint?: string | null) {
    const label = cwdHint
      ? cwdHint.split(/[\\/]/).filter(Boolean).pop() || "OXIDITE"
      : "OXIDITE";
    const cols = computeAvailableWordmarkCols();
    lastRenderedCols = cols;
    const wm = renderWordmark(label, cols);
    root.replaceChildren(
      buildHeaderFrame(wm, [
        opts.version ? `Oxidite ${opts.version}` : "Oxidite",
        cwdHint ? abbreviateHomePath(cwdHint) : "Local workspace",
      ]),
      buildStatusLine("Scanning repository...")
    );
  }

  function renderReport(report: ProjectReport) {
    lastReport = report;
    const cols = computeAvailableWordmarkCols();
    lastRenderedCols = cols;
    const wm = renderWordmark(report.wordmark || report.name, cols);

    const metaLine1 = [
      opts.version ? `Oxidite ${opts.version}` : "Oxidite",
      report.git_branch ? `branch:${report.git_branch}` : null,
    ]
      .filter(Boolean)
      .join(" · ");

    const metaLine2 = abbreviateHomePath(report.path);

    root.replaceChildren(
      buildHeaderFrame(wm, [metaLine1, metaLine2]),
      buildStatusLine(`Scanned repository in ${report.scan_time_ms}ms`),
      buildReportBlock(report)
    );
  }

  function buildHeaderFrame(
    wm: { text: string; isArt: boolean },
    metaLines: string[]
  ): HTMLElement {
    const frame = document.createElement("div");
    frame.className = "proj-frame";

    const left = document.createElement("div");
    left.className = "proj-col-left";

    const wordmarkEl = document.createElement("pre");
    wordmarkEl.className = wm.isArt
      ? "proj-wordmark"
      : "proj-wordmark proj-wordmark-fallback";
    wordmarkEl.textContent = wm.text;
    left.append(wordmarkEl);

    const meta = document.createElement("div");
    meta.className = "proj-meta";
    for (const line of metaLines) {
      if (!line) continue;
      const div = document.createElement("div");
      div.textContent = line;
      meta.append(div);
    }
    left.append(meta);

    const right = document.createElement("div");
    right.className = "proj-col-right";

    const cmdHeading = document.createElement("div");
    cmdHeading.className = "proj-heading";
    cmdHeading.textContent = "Commands";
    right.append(cmdHeading);

    const cmdList = document.createElement("div");
    cmdList.className = "proj-vbar-list";
    for (const cmd of opts.commands) {
      const row = document.createElement(cmd.onClick ? "button" : "div");
      row.className = "proj-cmd-row";
      if (row instanceof HTMLButtonElement) {
        row.type = "button";
        row.addEventListener("click", () => cmd.onClick?.());
      }
      const nameSpan = document.createElement("span");
      nameSpan.className = "proj-cmd-name";
      nameSpan.textContent = cmd.name;

      const descSpan = document.createElement("span");
      descSpan.className = "proj-cmd-desc";
      descSpan.textContent = cmd.desc;

      row.append(nameSpan, descSpan);
      cmdList.append(row);
    }

    if (opts.helpHint) {
      const hint = document.createElement("div");
      hint.className = "proj-cmd-hint";
      hint.textContent = opts.helpHint;
      cmdList.append(hint);
    }

    right.append(cmdList);
    frame.append(left, right);
    return frame;
  }

  function buildStatusLine(text: string): HTMLElement {
    const status = document.createElement("div");
    status.className = "proj-status";
    status.textContent = `- ${text}`;
    return status;
  }

  function buildReportBlock(r: ProjectReport): HTMLElement {
    const wrap = document.createElement("div");
    wrap.className = "proj-report";

    const title = document.createElement("div");
    title.className = "proj-heading proj-report-title";
    title.textContent = "Oxidite Project Report";

    const rule1 = document.createElement("hr");
    rule1.className = "proj-rule";

    const sub = document.createElement("div");
    sub.className = "proj-heading proj-section-title";
    sub.textContent = "Project";

    const rule2 = document.createElement("hr");
    rule2.className = "proj-rule";

    const dl = document.createElement("dl");
    dl.className = "proj-kv";

    addRow(dl, "Name:", r.name, true);
    if (r.language) addRow(dl, "Language:", r.language, true);
    if (r.frameworks.length > 0) addRow(dl, "Frameworks:", r.frameworks.join(", "), true);
    if (r.pkg_manager) addRow(dl, "Pkg Manager:", r.pkg_manager, true);

    if (r.prod_deps !== null || r.dev_deps !== null) {
      const parts: string[] = [];
      if (r.prod_deps !== null) parts.push(`${r.prod_deps} prod`);
      if (r.dev_deps !== null) parts.push(`${r.dev_deps} dev`);
      addRow(dl, "Dependencies:", parts.join(", "), false);
    }
    addRow(dl, "Files:", `${r.files}${r.truncated ? "+" : ""}`, false);
    addRow(dl, "Directories:", `${r.directories}${r.truncated ? "+" : ""}`, false);
    addRow(dl, "Scan Time:", `${r.scan_time_ms}ms`, false);

    wrap.append(title, rule1, sub, rule2, dl);
    return wrap;
  }

  function addRow(dl: HTMLDListElement, label: string, value: string, boldValue: boolean) {
    const dt = document.createElement("dt");
    dt.textContent = label;
    const dd = document.createElement("dd");
    if (boldValue) dd.className = "is-strong";
    dd.textContent = value;
    dl.append(dt, dd);
  }

  return {
    element: root,
    async refreshForSession(sid: string, cwdHint?: string | null, forceRefresh = false) {
      const key = `${sid}::${cwdHint ?? ""}`;
      activeKey = key;

      if (!forceRefresh) {
        const cached = cache.get(key);
        if (cached) {
          renderReport(cached);
          return;
        }
      }

      renderLoading(cwdHint);
      try {
        const report = await opts.fetchJson<ProjectReport>(
          `/api/project/scan?sid=${encodeURIComponent(sid)}`
        );
        cache.set(key, report);
        if (activeKey === key) {
          renderReport(report);
        }
      } catch {
        if (activeKey === key) {
          const status = root.querySelector(".proj-status");
          if (status) status.textContent = "- Workspace scan unavailable";
        }
      }
    },
    clearCache() {
      cache.clear();
    },
    destroy() {
      resizeObserver?.disconnect();
      root.remove();
    },
  };
}
