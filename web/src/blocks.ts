import type { IBuffer, IBufferCell, Terminal } from "@xterm/xterm";

export interface BlockContext {
  folder: string | null;
  branch: string | null;
  files: number;
  add: number;
  del: number;
}

export interface StyledSpan {
  text: string;
  fg: string | null;
  bg: string | null;
  bold: boolean;
  dim: boolean;
  italic: boolean;
  underline: boolean;
}

export interface RenderedLine {
  plain: string;
  spans: StyledSpan[];
}

export interface CommandBlockView {
  el: HTMLElement;
  startCommand(cmd: string, ctx: BlockContext, term?: Terminal): void;
  updateActiveContext(ctx: BlockContext): void;
  syncFromTerminal(term: Terminal): void;
  reset(): void;
}

const MAX_BLOCKS = 200;

const ANSI_16 = [
  "#1b1e2b",
  "#ff5470",
  "#34c47c",
  "#ffcb6b",
  "#5b8cff",
  "#b69cff",
  "#5ad7ff",
  "#e8ebf5",
  "#7c8296",
  "#ff758c",
  "#5fe29f",
  "#ffe08f",
  "#8ab0ff",
  "#d0bfff",
  "#8aecff",
  "#ffffff",
];

function color256(idx: number): string {
  if (idx < 16) return ANSI_16[idx] ?? "#e8ebf5";
  if (idx < 232) {
    const n = idx - 16;
    const r = Math.floor(n / 36);
    const g = Math.floor((n % 36) / 6);
    const b = n % 6;
    const ch = (v: number) => (v === 0 ? 0 : 55 + v * 40);
    return `rgb(${ch(r)},${ch(g)},${ch(b)})`;
  }
  const gray = 8 + (idx - 232) * 10;
  return `rgb(${gray},${gray},${gray})`;
}

function rgbHex(n: number): string {
  return `#${(n & 0xffffff).toString(16).padStart(6, "0")}`;
}

const PS_PROMPT_RE = /^(?:PS\s+(?:Microsoft\.PowerShell\.Core\\FileSystem::)?[^\r\n>]*>\s*)+$/;
const UNIX_PROMPT_RE = /^[^\r\n@]+@[^\r\n:]+:[^\r\n]*[$#%]\s*$/;

export function isPromptLine(plainLine: string): boolean {
  const trimmed = plainLine.trim();
  if (!trimmed) return false;
  return PS_PROMPT_RE.test(trimmed) || UNIX_PROMPT_RE.test(trimmed);
}

export function isEchoedCommandLine(plainLine: string, sentCmd: string | null): boolean {
  if (!sentCmd) return false;
  const cmdTrimmed = sentCmd.trim();
  if (!cmdTrimmed) return false;
  const trimmed = plainLine.trim();
  if (trimmed === cmdTrimmed) return true;
  if (/^PS\s+(?:Microsoft\.PowerShell\.Core\\FileSystem::)?[^\r\n>]*>/.test(trimmed)) {
    const afterGt = trimmed.replace(/^PS\s+(?:Microsoft\.PowerShell\.Core\\FileSystem::)?[^\r\n>]*>\s*/, "").trim();
    if (afterGt === cmdTrimmed) return true;
  }
  if (/^[^\r\n@]+@[^\r\n:]+:[^\r\n]*[$#%]\s*/.test(trimmed)) {
    const afterPrompt = trimmed.replace(/^[^\r\n@]+@[^\r\n:]+:[^\r\n]*[$#%]\s*/, "").trim();
    if (afterPrompt === cmdTrimmed) return true;
  }
  return false;
}

function resolveCellStyle(cell: IBufferCell): Omit<StyledSpan, "text"> {
  let fg: string | null = null;
  let bg: string | null = null;
  if (cell.isFgPalette()) fg = color256(cell.getFgColor());
  else if (cell.isFgRGB()) fg = rgbHex(cell.getFgColor());
  if (cell.isBgPalette()) bg = color256(cell.getBgColor());
  else if (cell.isBgRGB()) bg = rgbHex(cell.getBgColor());

  if (cell.isInverse()) {
    const tmp = fg;
    fg = bg ?? "#000000";
    bg = tmp ?? "#e8ebf5";
  }

  return {
    fg,
    bg,
    bold: cell.isBold() !== 0,
    dim: cell.isDim() !== 0,
    italic: cell.isItalic() !== 0,
    underline: cell.isUnderline() !== 0,
  };
}

function sameStyle(a: Omit<StyledSpan, "text">, b: Omit<StyledSpan, "text">): boolean {
  return (
    a.fg === b.fg &&
    a.bg === b.bg &&
    a.bold === b.bold &&
    a.dim === b.dim &&
    a.italic === b.italic &&
    a.underline === b.underline
  );
}

/** Extracts logical wrapped lines with exact cell colors from an xterm buffer slice. */
export function extractBufferLines(buffer: IBuffer, startRow: number, endRow: number): RenderedLine[] {
  const maxRow = Math.min(endRow, buffer.length - 1);
  if (maxRow < 0 || startRow > maxRow) return [];

  const groups: number[][] = [];
  for (let r = Math.max(0, startRow); r <= maxRow; r++) {
    const line = buffer.getLine(r);
    if (!line) continue;
    if (line.isWrapped && groups.length > 0) {
      groups[groups.length - 1]?.push(r);
    } else {
      groups.push([r]);
    }
  }

  const reusableCell = buffer.getLine(0)?.getCell(0);
  const out: RenderedLine[] = [];

  for (const rows of groups) {
    const spans: StyledSpan[] = [];
    let plain = "";

    for (let idx = 0; idx < rows.length; idx++) {
      const r = rows[idx] ?? 0;
      const line = buffer.getLine(r);
      if (!line) continue;
      // Preserve explicit spaces on intermediate wrapped physical rows; trim right on the final physical row of a logical line
      const trimmedStr = line.translateToString(idx === rows.length - 1);
      const len = idx === rows.length - 1 ? trimmedStr.length : line.length;

      for (let c = 0; c < len; c++) {
        const cell = line.getCell(c, reusableCell);
        if (!cell || cell.getWidth() === 0) continue;
        const ch = cell.getChars() || " ";
        const st = resolveCellStyle(cell);
        plain += ch;
        const last = spans[spans.length - 1];
        if (last && sameStyle(last, st)) {
          last.text += ch;
        } else {
          spans.push({ text: ch, ...st });
        }
      }
    }

    out.push({ plain: plain.replace(/\s+$/, ""), spans });
  }

  return out;
}

/** Filters prompts, echoed command line, and outer blank lines from extracted lines. */
export function filterBlockLines(lines: RenderedLine[], sentCmd: string | null): RenderedLine[] {
  let skippedCmdEcho = false;
  const kept: RenderedLine[] = [];
  for (const line of lines) {
    if (isPromptLine(line.plain)) continue;
    if (!skippedCmdEcho && isEchoedCommandLine(line.plain, sentCmd)) {
      skippedCmdEcho = true;
      continue;
    }
    kept.push(line);
  }
  while (kept.length > 0 && (kept[0]?.plain.trim() ?? "") === "") kept.shift();
  while (kept.length > 0 && (kept[kept.length - 1]?.plain.trim() ?? "") === "") kept.pop();
  return kept;
}

function renderLinesInto(container: HTMLElement, lines: RenderedLine[]): void {
  const frag = document.createDocumentFragment();
  for (const line of lines) {
    const row = document.createElement("div");
    row.className = "blk-line";
    if (line.spans.length === 0 || line.plain === "") {
      row.textContent = " ";
      frag.append(row);
      continue;
    }
    for (const s of line.spans) {
      if (!s.fg && !s.bg && !s.bold && !s.dim && !s.italic && !s.underline) {
        row.append(document.createTextNode(s.text));
        continue;
      }
      const span = document.createElement("span");
      if (s.fg) span.style.color = s.fg;
      if (s.bg) span.style.backgroundColor = s.bg;
      if (s.bold) span.style.fontWeight = "700";
      if (s.dim) span.style.opacity = "0.7";
      if (s.italic) span.style.fontStyle = "italic";
      if (s.underline) span.style.textDecoration = "underline";
      span.textContent = s.text;
      row.append(span);
    }
    frag.append(row);
  }
  container.replaceChildren(frag);
}

interface BlockEntry {
  root: HTMLElement;
  headEl: HTMLElement;
  outEl: HTMLElement;
  timeEl: HTMLElement;
  cmd: string | null;
  startedAt: number;
  startRow: number;
  frozen: boolean;
  lastSig: string;
}

export function createCommandBlocks(): CommandBlockView {
  const root = document.createElement("div");
  root.className = "blocks";
  const spacer = document.createElement("div");
  spacer.className = "blocks-spacer";
  spacer.setAttribute("aria-hidden", "true");
  const empty = document.createElement("div");
  empty.className = "blocks-empty";
  empty.textContent = "Run a command below — command history and output appear here.";
  root.append(spacer, empty);

  const entries: BlockEntry[] = [];
  let active: BlockEntry | null = null;
  let preCommandStartRow = 0;

  const trimOld = () => {
    while (entries.length > MAX_BLOCKS) {
      const old = entries.shift();
      old?.root.remove();
    }
  };

  const renderMetaInto = (head: HTMLElement, ctx: BlockContext, existingTime?: HTMLElement): HTMLElement => {
    const timeEl = existingTime ?? document.createElement("span");
    timeEl.className = "blk-time";
    head.replaceChildren();
    if (ctx.folder) {
      const f = document.createElement("span");
      f.className = "blk-path";
      f.textContent = ctx.folder;
      head.append(f);
    }
    if (ctx.branch) {
      const b = document.createElement("span");
      b.className = "blk-git";
      b.textContent = `git:(${ctx.branch})`;
      head.append(b);
      const st = document.createElement("span");
      st.className = "blk-stat";
      st.textContent = `${ctx.files} • +${ctx.add} -${ctx.del}`;
      head.append(st);
    }
    head.append(timeEl);
    return timeEl;
  };

  return {
    el: root,
    startCommand(cmd, ctx, term) {
      const cleanCmd = cmd.trim().toLowerCase();
      if (cleanCmd === "clear" || cleanCmd === "cls" || cleanCmd === "clear-host") {
        // Freeze previous active block before clearing or keep clean history; Warp resets on `clear`
        if (active) active.frozen = true;
      } else if (active) {
        active.frozen = true;
      }

      empty.hidden = true;
      const blk = document.createElement("article");
      blk.className = "blk";
      const headEl = document.createElement("div");
      headEl.className = "blk-head";
      const timeEl = renderMetaInto(headEl, ctx);
      const cmdEl = document.createElement("div");
      cmdEl.className = "blk-cmd";
      cmdEl.textContent = cmd;
      const outEl = document.createElement("div");
      outEl.className = "blk-out";
      outEl.hidden = true;
      blk.append(headEl, cmdEl, outEl);
      root.append(blk);

      const startRow = term ? term.buffer.active.baseY + term.buffer.active.cursorY : 0;
      const entry: BlockEntry = {
        root: blk,
        headEl,
        outEl,
        timeEl,
        cmd,
        startedAt: Date.now(),
        startRow,
        frozen: false,
        lastSig: "",
      };
      entries.push(entry);
      active = entry;
      trimOld();
      root.scrollTop = root.scrollHeight;
    },
    updateActiveContext(ctx) {
      if (!active || active.frozen) return;
      renderMetaInto(active.headEl, ctx, active.timeEl);
    },
    syncFromTerminal(term) {
      const buf = term.buffer.active;
      if (buf.type !== "normal") return;
      const endRow = buf.baseY + buf.cursorY;

      if (!active) {
        // Show pre-command output (like MOTD or SSH banner) only if non-prompt content exists
        const raw = extractBufferLines(buf, preCommandStartRow, endRow);
        const filtered = filterBlockLines(raw, null);
        if (filtered.length === 0) return;

        empty.hidden = true;
        const blk = document.createElement("article");
        blk.className = "blk blk-banner";
        const headEl = document.createElement("div");
        const outEl = document.createElement("div");
        outEl.className = "blk-out";
        const timeEl = document.createElement("span");
        blk.append(outEl);
        root.append(blk);

        const entry: BlockEntry = {
          root: blk,
          headEl,
          outEl,
          timeEl,
          cmd: null,
          startedAt: Date.now(),
          startRow: preCommandStartRow,
          frozen: false,
          lastSig: "",
        };
        entries.push(entry);
        active = entry;
        trimOld();
      }

      if (active.frozen) return;
      // Handle terminal clear/reset where cursor jumped above startRow
      if (endRow < active.startRow) {
        active.startRow = 0;
      }

      const raw = extractBufferLines(buf, active.startRow, endRow);
      const filtered = filterBlockLines(raw, active.cmd);
      const sig = filtered.map((l) => l.plain).join("\n");
      if (sig !== active.lastSig) {
        active.lastSig = sig;
        if (filtered.length > 0) {
          active.outEl.hidden = false;
          renderLinesInto(active.outEl, filtered);
        } else {
          active.outEl.hidden = true;
          active.outEl.replaceChildren();
        }
        if (active.cmd !== null) {
          const secs = ((Date.now() - active.startedAt) / 1000).toFixed(2);
          active.timeEl.textContent = `(${secs}s)`;
        }
        root.scrollTop = root.scrollHeight;
      }
    },
    reset() {
      entries.length = 0;
      active = null;
      preCommandStartRow = 0;
      empty.hidden = false;
      root.replaceChildren(spacer, empty);
    },
  };
}
