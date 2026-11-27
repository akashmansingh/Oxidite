// Pure logic behind the command input bar. No DOM, so it can be tested on its own.

export type KeyAction =
  | "send"
  | "newline"
  | "interrupt"
  | "clear"
  | "eof"
  | "clear-screen"
  | "history-prev"
  | "history-next"
  | "to-terminal"
  | "none";

export interface KeyInfo {
  key: string;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
  isComposing?: boolean;
}

export interface EditorState {
  text: string;
  selStart: number;
  selEnd: number;
}

/** Decides what a key press in the input editor should do. */
export function keyAction(k: KeyInfo, s: EditorState): KeyAction {
  if (k.isComposing) return "none";
  const hasSelection = s.selStart !== s.selEnd;
  const bareCtrl = k.ctrlKey && !k.shiftKey && !k.altKey && !k.metaKey;
  const bare = !k.ctrlKey && !k.shiftKey && !k.altKey && !k.metaKey;
  const letter = k.key.toLowerCase();

  if (k.key === "Enter" && !k.ctrlKey && !k.altKey && !k.metaKey) return k.shiftKey ? "newline" : "send";
  if (k.key === "Escape" && bare) return "to-terminal";
  if (bareCtrl && letter === "c") return hasSelection ? "none" : s.text === "" ? "interrupt" : "clear";
  if (bareCtrl && letter === "d" && s.text === "") return "eof";
  if (bareCtrl && letter === "l") return "clear-screen";
  if (k.key === "ArrowUp" && bare && !hasSelection && !s.text.slice(0, s.selStart).includes("\n")) return "history-prev";
  if (k.key === "ArrowDown" && bare && !hasSelection && !s.text.slice(s.selEnd).includes("\n")) return "history-next";
  return "none";
}

/** Turns the editor text into the bytes a shell expects: one carriage return per line, plus a final one. */
export function toPtyInput(text: string): string {
  const body = text.replace(/[\r\n]+$/, "");
  return body.replace(/\r\n?|\n/g, "\r") + "\r";
}

/** Command history for one session. Kept in memory only: commands can hold secrets, so nothing is persisted. */
export class CommandHistory {
  private items: string[] = [];
  private pos = -1;
  private draft = "";

  constructor(private readonly max = 200) {}

  push(cmd: string): void {
    const clean = cmd.replace(/[\r\n]+$/, "");
    if (clean.trim() && this.items.at(-1) !== clean) {
      this.items.push(clean);
      if (this.items.length > this.max) this.items.shift();
    }
    this.reset();
  }

  /** Older entry, or null when there is none. The first call remembers what was typed so far. */
  prev(current: string): string | null {
    if (this.items.length === 0) return null;
    if (this.pos === -1) {
      this.draft = current;
      this.pos = this.items.length - 1;
      return this.items[this.pos] ?? null;
    }
    if (this.pos > 0) {
      this.pos -= 1;
      return this.items[this.pos] ?? null;
    }
    return null;
  }

  /** Newer entry, then the remembered draft, or null when not browsing. */
  next(): string | null {
    if (this.pos === -1) return null;
    if (this.pos < this.items.length - 1) {
      this.pos += 1;
      return this.items[this.pos] ?? null;
    }
    this.pos = -1;
    return this.draft;
  }

  reset(): void {
    this.pos = -1;
    this.draft = "";
  }
}

/** Text for the three parts of the changes chip: file count, added lines, removed lines. */
export function changeParts(files: number, add: number, del: number): [string, string, string] {
  return [String(files), `+${add}`, `−${del}`];
}
