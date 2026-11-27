import { icon, type IconName } from "./icons";
import { CommandHistory, changeParts, keyAction, longestCommonPrefix, toPtyInput } from "./composer-model";
import { createMobileKbdBar, type MobileKbdBarHandle } from "./mobile-kbd-bar";

export interface CompletionItem {
  value: string;
  display: string;
  kind: "dir" | "file" | "branch" | "cmd" | string;
}

export interface CompletionResult {
  replace_start: number;
  replace_end: number;
  candidates: CompletionItem[];
}

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
  /** Called with the command text when the user submits a command. */
  onCommand?(cmd: string): void;
  /** Fetches command, path, and branch completions for the current line and cursor. */
  complete?(line: string, cursor: number): Promise<CompletionResult | null>;
  /** Moves keyboard focus into the terminal for raw typing. */
  focusTerminal(): void;
  /** Shows the diff pane. */
  openChanges(): void;
  /** Called when the mobile Scroll-Lock / Read-Only mode toggles. */
  onScrollLockChange?(locked: boolean): void;
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
  /** Focuses the raw-mode keyboard proxy (phone) or composer when active. */
  focusRaw(): void;
  isScrollLocked(): boolean;
  destroy(): void;
  hasFocus(): boolean;
  isRaw(): boolean;
}

function h<K extends keyof HTMLElementTagNameMap>(tag: K, cls = "", text = ""): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  e.className = cls;
  if (text) e.textContent = text;
  return e;
}

function chip(kind: IconName, tag: "span" | "button", extraCls = ""): { root: HTMLElement; label: HTMLElement } {
  const base = tag === "button" ? "chip chip-btn" : "chip";
  const root = h(tag, extraCls ? `${base} ${extraCls}` : base);
  if (root instanceof HTMLButtonElement) root.type = "button";
  const label = h("span", "chip-t");
  root.append(icon(kind), label);
  return { root, label };
}

const PLACEHOLDER = "Run a command";

export function createComposer(opts: ComposerOptions): Composer {
  const root = h("div", "composer");
  root.dataset.raw = "0";
  root.dataset.b = "ok";

  const chips = h("div", "chips");
  const folder = chip("folder", "span");
  const branch = chip("branch", "span", "chip-branch");
  const changes = chip("file", "button");
  changes.root.title = "Show changes";
  changes.root.addEventListener("click", opts.openChanges);
  chips.append(folder.root, branch.root, changes.root);

  const editor = h("div", "editor");
  const ghost = h("div", "editor-ghost");
  const ta = h("textarea");
  ta.rows = 1;
  ta.placeholder = PLACEHOLDER;
  ta.spellcheck = false;
  ta.autocomplete = "off";
  ta.autocapitalize = "off";
  ta.setAttribute("autocorrect", "off");
  ta.setAttribute("enterkeyhint", "send");
  ta.setAttribute("aria-label", "Command input. Enter runs it in the terminal.");
  editor.append(ghost, ta);

  const acMenu = h("div", "ac-menu");
  acMenu.setAttribute("role", "listbox");
  acMenu.setAttribute("aria-label", "Completions");
  acMenu.hidden = true;

  const hint = h("div", "hint", "tab autocomplete  ·  enter run  ·  shift+enter new line");

  root.append(acMenu, chips, editor, hint);

  const history = new CommandHistory();

  let acState: {
    start: number;
    end: number;
    items: CompletionItem[];
    index: number;
    menuBrowse: boolean;
    lastAppliedText: string;
  } | null = null;
  let acSeq = 0;
  let liveTimer: number | undefined;
  let lastLiveKey = "";

  const updateGhost = (): void => {
    const val = ta.value;
    if (!val || val.includes("\n") || ta.disabled) {
      ghost.textContent = "";
      return;
    }

    const cursor = ta.selectionStart ?? val.length;
    let acGhost = "";
    if (acState && acState.items.length > 0 && cursor === val.length && acState.end === val.length) {
      const idx = acState.index >= 0 ? acState.index : 0;
      const cand = acState.items[idx];
      if (cand) {
        const typedSlice = val.slice(acState.start, acState.end);
        if (cand.value.toLowerCase().startsWith(typedSlice.toLowerCase())) {
          const built = val.slice(0, acState.start) + typedSlice + cand.value.slice(typedSlice.length);
          if (built.length > val.length) acGhost = built;
        }
      }
    }

    const histRaw = history.suggest(val);
    const histGhost = histRaw && histRaw.length > val.length ? val + histRaw.slice(val.length) : "";
    const firstKind = acState?.items[0]?.kind ?? "";
    const hasPathOrRepoArg = val.includes(" ") || firstKind === "dir" || firstKind === "branch" || firstKind === "file";

    ghost.textContent = hasPathOrRepoArg
      ? (acGhost || histGhost)
      : (histGhost || acGhost);
  };

  const closeAc = (): void => {
    acState = null;
    acMenu.hidden = true;
    acMenu.replaceChildren();
  };

  const fit = (): void => {
    ta.style.height = "auto";
    ta.style.height = `${ta.scrollHeight}px`;
    updateGhost();
  };
  const setText = (v: string, cursorPos = v.length): void => {
    ta.value = v;
    fit();
    ta.setSelectionRange(cursorPos, cursorPos);
  };

  const applyCandidate = (start: number, end: number, value: string): { nextText: string; nextEnd: number } => {
    const curVal = ta.value;
    const nextText = curVal.slice(0, start) + value + curVal.slice(end);
    const nextCursor = start + value.length;
    setText(nextText, nextCursor);
    return { nextText, nextEnd: nextCursor };
  };

  const renderAcMenu = (): void => {
    if (!acState || acState.items.length === 0) {
      acMenu.hidden = true;
      acMenu.replaceChildren();
      return;
    }
    const state = acState;
    acMenu.replaceChildren(
      ...state.items.map((item, idx) => {
        const btn = h("button", "ac-item");
        btn.type = "button";
        btn.setAttribute("role", "option");
        btn.setAttribute("aria-selected", String(idx === state.index));
        const badge = h("span", `ac-kind ac-${item.kind}`, item.kind);
        const label = h("span", "ac-label", item.display);
        btn.append(badge, label);
        btn.addEventListener("mousedown", (ev) => {
          ev.preventDefault();
          applyCandidate(state.start, state.end, item.value);
          closeAc();
          ta.focus();
          if (item.value.endsWith("\\") || item.value.endsWith("/")) scheduleLiveComplete(20);
        });
        return btn;
      }),
    );
    acMenu.hidden = false;
    if (state.index >= 0) {
      const activeEl = acMenu.children[state.index] as HTMLElement | undefined;
      activeEl?.scrollIntoView({ block: "nearest" });
    }
  };

  const scheduleLiveComplete = (delayMs = 150): void => {
    window.clearTimeout(liveTimer);
    if (!opts.complete || ta.disabled || ta.value.includes("\n")) return;
    if (ta.value.trim().length === 0) {
      lastLiveKey = "";
      closeAc();
      updateGhost();
      return;
    }
    liveTimer = window.setTimeout(() => {
      void runLiveComplete();
    }, delayMs);
  };

  const runLiveComplete = async (): Promise<void> => {
    if (!opts.complete || ta.disabled || ta.value.includes("\n")) return;
    const snapText = ta.value;
    const snapCursor = ta.selectionStart ?? snapText.length;
    if (snapText.trim().length === 0) {
      lastLiveKey = "";
      closeAc();
      updateGhost();
      return;
    }
    const cacheKey = `${snapText}\0${snapCursor}`;
    if (cacheKey === lastLiveKey && acState) return;
    const seq = ++acSeq;
    const res = await opts.complete(snapText, snapCursor).catch(() => null);
    if (seq !== acSeq || ta.value !== snapText || document.activeElement !== ta) return;
    lastLiveKey = cacheKey;
    if (!res || res.candidates.length === 0) {
      closeAc();
      updateGhost();
      return;
    }
    acState = {
      start: res.replace_start,
      end: res.replace_end,
      items: res.candidates,
      index: -1,
      menuBrowse: false,
      lastAppliedText: snapText,
    };
    renderAcMenu();
    updateGhost();
  };

  const runTabComplete = async (reverse: boolean): Promise<void> => {
    if (!opts.complete || ta.disabled) return;
    window.clearTimeout(liveTimer);
    if (acState && ta.value === acState.lastAppliedText && acState.items.length > 0) {
      const n = acState.items.length;
      if (n === 1 && acState.index === -1) {
        const only = acState.items[0];
        if (only) applyCandidate(acState.start, acState.end, only.value);
        closeAc();
        return;
      }
      if (acState.index === -1) {
        const activeToken = ta.value.slice(acState.start, acState.end);
        const lcp = longestCommonPrefix(acState.items.map((c) => c.value));
        if (lcp.length > activeToken.length) {
          const applied = applyCandidate(acState.start, acState.end, lcp);
          acState.end = applied.nextEnd;
          acState.lastAppliedText = applied.nextText;
          renderAcMenu();
          updateGhost();
          return;
        }
      }
      acState.index = (acState.index + (reverse ? -1 : 1) + n) % n;
      acState.menuBrowse = false;
      const picked = acState.items[acState.index];
      if (picked) {
        const applied = applyCandidate(acState.start, acState.end, picked.value);
        acState.end = applied.nextEnd;
        acState.lastAppliedText = applied.nextText;
        renderAcMenu();
        updateGhost();
      }
      return;
    }

    const seq = ++acSeq;
    const snapText = ta.value;
    const snapCursor = ta.selectionStart ?? snapText.length;
    const res = await opts.complete(snapText, snapCursor).catch(() => null);
    if (seq !== acSeq || ta.value !== snapText || !res || res.candidates.length === 0) {
      closeAc();
      return;
    }

    if (res.candidates.length === 1) {
      const only = res.candidates[0];
      if (only) {
        applyCandidate(res.replace_start, res.replace_end, only.value);
      }
      closeAc();
      return;
    }

    const activeToken = snapText.slice(res.replace_start, res.replace_end);
    const lcp = longestCommonPrefix(res.candidates.map((c) => c.value));
    if (lcp.length > activeToken.length) {
      const applied = applyCandidate(res.replace_start, res.replace_end, lcp);
      acState = {
        start: res.replace_start,
        end: applied.nextEnd,
        items: res.candidates,
        index: -1,
        menuBrowse: false,
        lastAppliedText: applied.nextText,
      };
      renderAcMenu();
      updateGhost();
      return;
    }

    const startIdx = reverse ? res.candidates.length - 1 : 0;
    const first = res.candidates[startIdx];
    if (!first) return;
    const applied = applyCandidate(res.replace_start, res.replace_end, first.value);
    acState = {
      start: res.replace_start,
      end: applied.nextEnd,
      items: res.candidates,
      index: startIdx,
      menuBrowse: false,
      lastAppliedText: applied.nextText,
    };
    renderAcMenu();
    updateGhost();
  };

  ta.addEventListener("input", () => {
    closeAc();
    fit();
    scheduleLiveComplete();
  });
  ta.addEventListener("blur", () => {
    window.clearTimeout(liveTimer);
    lastLiveKey = "";
    closeAc();
  });
  ta.addEventListener("keydown", (e) => {
    // ArrowRight at end of line accepts inline ghost autosuggestion (history or live path/cmd completion)
    if (
      e.key === "ArrowRight" &&
      !e.ctrlKey &&
      !e.shiftKey &&
      !e.altKey &&
      !e.metaKey &&
      ta.selectionStart === ta.value.length &&
      ta.selectionEnd === ta.value.length &&
      ghost.textContent &&
      ghost.textContent.length > ta.value.length
    ) {
      e.preventDefault();
      const accepted = ghost.textContent;
      setText(accepted);
      closeAc();
      if (accepted.endsWith("\\") || accepted.endsWith("/")) scheduleLiveComplete(20);
      return;
    }

    const act = keyAction(e, { text: ta.value, selStart: ta.selectionStart, selEnd: ta.selectionEnd });
    switch (act) {
      case "send": {
        e.preventDefault();
        window.clearTimeout(liveTimer);
        acSeq += 1;
        if (acState && acState.menuBrowse && acState.index >= 0) {
          const chosen = acState.items[acState.index];
          if (chosen) {
            applyCandidate(acState.start, acState.end, chosen.value);
            closeAc();
            if (chosen.value.endsWith("\\") || chosen.value.endsWith("/")) scheduleLiveComplete(20);
            break;
          }
        }
        closeAc();
        lastLiveKey = "";
        const text = ta.value;
        opts.onCommand?.(text);
        opts.send(toPtyInput(text));
        history.push(text);
        setText("");
        break;
      }
      case "tab-complete":
        e.preventDefault();
        void runTabComplete(false);
        break;
      case "tab-prev":
        e.preventDefault();
        void runTabComplete(true);
        break;
      case "interrupt":
        e.preventDefault();
        window.clearTimeout(liveTimer);
        closeAc();
        opts.send("\x03");
        break;
      case "clear":
        e.preventDefault();
        window.clearTimeout(liveTimer);
        closeAc();
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
        if (acState && !acMenu.hidden && acState.menuBrowse && acState.items.length > 0) {
          e.preventDefault();
          if (acState.index <= 0) {
            acState.menuBrowse = false;
            acState.index = -1;
          } else {
            acState.index -= 1;
          }
          renderAcMenu();
          updateGhost();
          break;
        }
        window.clearTimeout(liveTimer);
        closeAc();
        const v = history.prev(ta.value);
        if (v !== null) {
          e.preventDefault();
          setText(v);
        }
        break;
      }
      case "history-next": {
        if (acState && !acMenu.hidden && acState.items.length > 0 && !history.isBrowsing()) {
          e.preventDefault();
          acState.menuBrowse = true;
          acState.index = (acState.index + 1) % acState.items.length;
          renderAcMenu();
          updateGhost();
          break;
        }
        window.clearTimeout(liveTimer);
        closeAc();
        const v = history.next();
        if (v !== null) {
          e.preventDefault();
          setText(v);
        }
        break;
      }
      case "dismiss":
        window.clearTimeout(liveTimer);
        if (acState) {
          e.preventDefault();
          closeAc();
          updateGhost();
        }
        break;
      default:
        break;
    }
  });

  let last = "";
  const kbd: MobileKbdBarHandle = createMobileKbdBar({
    textarea: ta,
    sendRaw: opts.send,
    insertText: (text: string) => {
      if (root.dataset.raw === "1") {
        opts.send(text);
        return;
      }
      const start = ta.selectionStart ?? ta.value.length;
      const end = ta.selectionEnd ?? start;
      const next = ta.value.slice(0, start) + text + ta.value.slice(end);
      setText(next, start + text.length);
      closeAc();
      scheduleLiveComplete(20);
    },
    onShiftEnter: () => {
      const start = ta.selectionStart ?? ta.value.length;
      const end = ta.selectionEnd ?? start;
      const next = ta.value.slice(0, start) + "\n" + ta.value.slice(end);
      setText(next, start + 1);
    },
    onScrollLockChange: (locked: boolean) => {
      root.dataset.locked = locked ? "1" : "0";
      opts.onScrollLockChange?.(locked);
    },
    isRawMode: () => root.dataset.raw === "1",
  });
  hint.before(kbd.element);
  if (typeof document !== "undefined" && document.body) {
    document.body.append(kbd.rawProxy);
  }

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
      if (!kbd.isScrollLocked()) ta.focus();
    },
    focusRaw() {
      kbd.focusActiveInput();
    },
    isScrollLocked() {
      return kbd.isScrollLocked();
    },
    destroy() {
      kbd.destroy();
    },
    hasFocus() {
      return root.contains(document.activeElement) || document.activeElement === kbd.rawProxy;
    },
    isRaw() {
      return root.dataset.raw === "1";
    },
  };
}
