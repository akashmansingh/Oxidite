import { icon, type IconName } from "./icons";
import { CommandHistory, changeParts, keyAction, toPtyInput } from "./composer-model";

export interface ComposerContext {
  /** Folder or host the session is working in; null hides the chip. */
  folder: string | null;
  /** Git branch; null hides the branch and changes chips. */
  branch: string | null;
  files: number;
  add: number;
  del: number;
}

export interface ComposerOptions {
  /** Sends raw bytes (as a string) to the session. */
  send(data: string): void;
  /** Moves keyboard focus into the terminal for raw typing. */
  focusTerminal(): void;
  /** Shows the diff pane. */
  openChanges(): void;
}

export interface Composer {
  el: HTMLElement;
  setContext(c: ComposerContext): void;
  /** `ok`, `warn`, `retry` or `down`: colors the prompt glyph to match the session. */
  setHealth(b: string): void;
  /** True while a full-screen program owns the terminal: the editor hides. */
  setRaw(raw: boolean): void;
  /** Turns typing off with a reason shown as the placeholder (a finished session, say). */
  setEnabled(on: boolean, reason?: string): void;
  focus(): void;
  hasFocus(): boolean;
  isRaw(): boolean;
}

function h<K extends keyof HTMLElementTagNameMap>(tag: K, cls = "", text = ""): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  e.className = cls;
  if (text) e.textContent = text;
  return e;
}

function chip(kind: IconName, tag: "span" | "button"): { root: HTMLElement; label: HTMLElement } {
  const root = h(tag, tag === "button" ? "chip chip-btn" : "chip");
  if (root instanceof HTMLButtonElement) root.type = "button";
  const label = h("span", "chip-t");
  root.append(icon(kind), label);
  return { root, label };
}

function kbd(...keys: string[]): HTMLElement {
  const wrap = h("span", "keys");
  for (const k of keys) wrap.append(h("kbd", "", k));
  return wrap;
}

const PLACEHOLDER = "Run a command";

export function createComposer(opts: ComposerOptions): Composer {
  const root = h("div", "composer");
  root.dataset.raw = "0";
  root.dataset.b = "ok";

  const chips = h("div", "chips");
  const folder = chip("folder", "span");
  const branch = chip("branch", "span");
  const changes = chip("file", "button");
  changes.root.title = "Show changes";
  changes.root.addEventListener("click", opts.openChanges);
  chips.append(folder.root, branch.root, changes.root);

  const editor = h("div", "editor");
  const prompt = icon("prompt");
  prompt.classList.add("pmt");
  const ta = h("textarea");
  ta.rows = 1;
  ta.placeholder = PLACEHOLDER;
  ta.spellcheck = false;
  ta.autocomplete = "off";
  ta.autocapitalize = "off";
  ta.setAttribute("autocorrect", "off");
  ta.setAttribute("enterkeyhint", "send");
  ta.setAttribute("aria-label", "Command input. Enter runs it in the terminal.");
  editor.append(prompt, ta);

  const hint = h("div", "hint");
  const enter = h("span");
  enter.append(kbd("Enter"), " run");
  const line = h("span");
  line.append(kbd("Shift", "Enter"), " new line");
  const esc = h("span");
  esc.append(kbd("Esc"), " type in terminal");
  hint.append(enter, line, esc);

  root.append(chips, editor, hint);

  const history = new CommandHistory();

  const fit = (): void => {
    ta.style.height = "auto";
    ta.style.height = `${ta.scrollHeight}px`;
  };
  const setText = (v: string): void => {
    ta.value = v;
    fit();
    ta.setSelectionRange(v.length, v.length);
  };

  ta.addEventListener("input", fit);
  ta.addEventListener("keydown", (e) => {
    const act = keyAction(e, { text: ta.value, selStart: ta.selectionStart, selEnd: ta.selectionEnd });
    switch (act) {
      case "send": {
        e.preventDefault();
        const text = ta.value;
        opts.send(toPtyInput(text));
        history.push(text);
        setText("");
        break;
      }
      case "interrupt":
        e.preventDefault();
        opts.send("\x03");
        break;
      case "clear":
        e.preventDefault();
        history.reset();
        setText("");
        break;
      case "eof":
        e.preventDefault();
        opts.send("\x04");
        break;
      case "clear-screen":
        e.preventDefault();
        opts.send("\x0c");
        break;
      case "history-prev": {
        const v = history.prev(ta.value);
        if (v !== null) {
          e.preventDefault();
          setText(v);
        }
        break;
      }
      case "history-next": {
        const v = history.next();
        if (v !== null) {
          e.preventDefault();
          setText(v);
        }
        break;
      }
      case "to-terminal":
        e.preventDefault();
        opts.focusTerminal();
        break;
      default:
        break;
    }
  });

  let last = "";
  return {
    el: root,
    setContext(c) {
      const key = JSON.stringify(c);
      if (key === last) return;
      last = key;
      folder.root.hidden = !c.folder;
      folder.label.textContent = c.folder ?? "";
      branch.root.hidden = c.branch === null;
      branch.label.textContent = c.branch ?? "";
      const showChanges = c.branch !== null && c.files > 0;
      changes.root.hidden = !showChanges;
      if (showChanges) {
        const [files, add, del] = changeParts(c.files, c.add, c.del);
        const a = h("span", "add", add);
        const d = h("span", "del", del);
        changes.label.replaceChildren(files, h("span", "sep", "•"), a, d);
      }
      chips.hidden = folder.root.hidden && branch.root.hidden && changes.root.hidden;
    },
    setHealth(b) {
      root.dataset.b = b;
    },
    setRaw(raw) {
      root.dataset.raw = raw ? "1" : "0";
    },
    setEnabled(on, reason) {
      ta.disabled = !on;
      ta.placeholder = on ? PLACEHOLDER : (reason ?? "This session has ended");
      root.dataset.off = on ? "0" : "1";
    },
    focus() {
      ta.focus();
    },
    hasFocus() {
      return root.contains(document.activeElement);
    },
    isRaw() {
      return root.dataset.raw === "1";
    },
  };
}
