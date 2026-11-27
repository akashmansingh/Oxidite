import { applyStickyCtrl } from "./sticky-ctrl";

export interface MobileKbdBarOptions {
  /** Primary input element (composer textarea). */
  textarea: HTMLTextAreaElement;
  /** Sends raw terminal bytes directly to the active PTY session. */
  sendRaw: (data: string) => void;
  /** Inserts text into the composer (or forwards to PTY in raw mode). */
  insertText: (text: string) => void;
  /** Triggered when Shift+Enter is tapped in composer mode. */
  onShiftEnter?: () => void;
  /** Triggered when Scroll-Lock / Read-Only mode toggles. */
  onScrollLockChange?: (locked: boolean) => void;
  /** Returns true when the terminal is in raw/alternate-screen mode (e.g. vim). */
  isRawMode?: () => boolean;
}

export interface MobileKbdBarHandle {
  element: HTMLElement;
  rawProxy: HTMLTextAreaElement;
  isCtrlLatched: () => boolean;
  isScrollLocked: () => boolean;
  setScrollLocked: (locked: boolean) => void;
  focusActiveInput: () => void;
  destroy: () => void;
}

const SYMBOLS = ["\\", "/", "-", "|", "..", "~", "_", ";", "&"];

export function createMobileKbdBar(opts: MobileKbdBarOptions): MobileKbdBarHandle {
  let ctrlLatched = false;
  let scrollLocked = false;
  let repeatTimer: number | null = null;
  let repeatInterval: number | null = null;

  // Off-screen focusable textarea proxy so raw mode (vim) keeps phone keyboard open.
  const rawProxy = document.createElement("textarea");
  rawProxy.className = "oxidite-raw-kbd-proxy";
  rawProxy.setAttribute("aria-label", "Raw terminal keyboard proxy");
  configureMobileInputAttrs(rawProxy);
  configureMobileInputAttrs(opts.textarea);

  const bar = document.createElement("div");
  bar.className = "kbd-bar";
  bar.setAttribute("role", "toolbar");
  bar.setAttribute("aria-label", "Mobile terminal keys");

  const fixedRow = document.createElement("div");
  fixedRow.className = "kbd-bar-fixed";

  const symbolStrip = document.createElement("div");
  symbolStrip.className = "kbd-bar-symbols";

  const ctrlBtn = makeFixedButton("Ctrl", "Toggle sticky Ctrl", () => {
    setCtrlLatched(!ctrlLatched);
  });
  ctrlBtn.classList.add("kbd-btn-modifier");

  const lockBtn = makeFixedButton("Lock", "Toggle Read-Only Scroll Lock", () => {
    setScrollLocked(!scrollLocked);
  });
  lockBtn.classList.add("kbd-btn-lock");

  const setCtrlLatched = (next: boolean) => {
    ctrlLatched = next;
    ctrlBtn.classList.toggle("is-active", ctrlLatched);
    ctrlBtn.setAttribute("aria-pressed", String(ctrlLatched));
  };

  const setScrollLocked = (next: boolean) => {
    scrollLocked = next;
    lockBtn.classList.toggle("is-active", scrollLocked);
    lockBtn.setAttribute("aria-pressed", String(scrollLocked));
    lockBtn.textContent = scrollLocked ? "Locked" : "Lock";
    opts.textarea.readOnly = scrollLocked;
    rawProxy.readOnly = scrollLocked;
    if (scrollLocked) {
      opts.textarea.blur();
      rawProxy.blur();
    }
    opts.onScrollLockChange?.(scrollLocked);
  };

  const focusActiveInput = () => {
    if (scrollLocked) return;
    if (opts.isRawMode?.()) {
      rawProxy.focus({ preventScroll: true });
    } else {
      opts.textarea.focus({ preventScroll: true });
    }
  };

  function makeFixedButton(
    label: string,
    title: string,
    onTrigger: () => void,
    repeatable = false
  ): HTMLButtonElement {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "kbd-btn";
    btn.textContent = label;
    btn.title = title;
    btn.tabIndex = -1;

    const stopRepeat = () => {
      if (repeatTimer !== null) {
        window.clearTimeout(repeatTimer);
        repeatTimer = null;
      }
      if (repeatInterval !== null) {
        window.clearInterval(repeatInterval);
        repeatInterval = null;
      }
    };

    // Prevent soft-keyboard blur on touch/mouse press
    btn.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      if (scrollLocked && btn !== lockBtn) return;
      onTrigger();
      if (repeatable) {
        stopRepeat();
        repeatTimer = window.setTimeout(() => {
          repeatInterval = window.setInterval(() => {
            if (scrollLocked) {
              stopRepeat();
              return;
            }
            onTrigger();
          }, 75);
        }, 360);
      }
    });

    btn.addEventListener("pointerup", stopRepeat);
    btn.addEventListener("pointercancel", stopRepeat);
    btn.addEventListener("pointerleave", stopRepeat);

    return btn;
  }

  // Fixed action buttons
  fixedRow.append(
    makeFixedButton("Tab", "Send Tab", () => opts.sendRaw("\t")),
    ctrlBtn,
    makeFixedButton("^C", "Send Ctrl+C (SIGINT)", () => {
      setCtrlLatched(false);
      opts.sendRaw("\x03");
    }),
    makeFixedButton("Esc", "Send Escape", () => opts.sendRaw("\x1b")),
    makeFixedButton("⇧↵", "Shift+Enter (newline)", () => {
      if (opts.onShiftEnter) {
        opts.onShiftEnter();
      } else {
        opts.insertText("\n");
      }
    }),
    makeFixedButton("↑", "Arrow Up", () => opts.sendRaw("\x1b[A"), true),
    makeFixedButton("↓", "Arrow Down", () => opts.sendRaw("\x1b[B"), true),
    makeFixedButton("←", "Arrow Left", () => opts.sendRaw("\x1b[D"), true),
    makeFixedButton("→", "Arrow Right", () => opts.sendRaw("\x1b[C"), true)
  );

  // Paste button: invokes navigator.clipboard.readText() synchronously in click/pointerup
  // to satisfy iOS Safari user-gesture restrictions.
  const pasteBtn = document.createElement("button");
  pasteBtn.type = "button";
  pasteBtn.className = "kbd-btn";
  pasteBtn.textContent = "Paste";
  pasteBtn.title = "Paste from clipboard";
  pasteBtn.tabIndex = -1;
  pasteBtn.addEventListener("mousedown", (e) => e.preventDefault());
  pasteBtn.addEventListener("click", (e) => {
    e.preventDefault();
    if (scrollLocked || !navigator.clipboard?.readText) return;
    navigator.clipboard
      .readText()
      .then((text) => {
        if (!text) return;
        if (opts.isRawMode?.()) {
          opts.sendRaw(text);
        } else {
          opts.insertText(text);
        }
        focusActiveInput();
      })
      .catch(() => {
        // Ignore clipboard permission denial silently
      });
  });
  fixedRow.append(pasteBtn, lockBtn);

  // Scrollable symbol strip: DO NOT call e.preventDefault() on pointerdown so horizontal
  // touch panning works smoothly on iOS Safari & Android Chrome.
  for (const sym of SYMBOLS) {
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "kbd-btn kbd-chip";
    chip.textContent = sym;
    chip.tabIndex = -1;

    let startX = 0;
    let startY = 0;
    let activePointerId: number | null = null;

    chip.addEventListener("mousedown", (e) => e.preventDefault());
    chip.addEventListener("pointerdown", (e) => {
      activePointerId = e.pointerId;
      startX = e.clientX;
      startY = e.clientY;
    });
    chip.addEventListener("pointerup", (e) => {
      if (activePointerId !== e.pointerId) return;
      activePointerId = null;
      const dx = Math.abs(e.clientX - startX);
      const dy = Math.abs(e.clientY - startY);
      if (dx < 8 && dy < 8 && !scrollLocked) {
        if (opts.isRawMode?.()) {
          opts.sendRaw(sym);
        } else {
          opts.insertText(sym);
        }
        focusActiveInput();
      }
    });
    chip.addEventListener("pointercancel", () => {
      activePointerId = null;
    });

    symbolStrip.append(chip);
  }

  bar.append(fixedRow, symbolStrip);

  // Sticky Ctrl interception (supports both standard beforeinput AND Android Gboard IME
  // composition where beforeinput.preventDefault() is ignored).
  const attachCtrlInterceptor = (el: HTMLTextAreaElement, isProxy: boolean) => {
    let handledInBeforeInput = false;

    const onBeforeInput = (e: InputEvent) => {
      handledInBeforeInput = false;
      if (!ctrlLatched || !e.data || e.data.length === 0) return;

      const ctrlByte = applyStickyCtrl(e.data);
      if (!ctrlByte) return;

      if (!e.isComposing && e.cancelable) {
        e.preventDefault();
        handledInBeforeInput = true;
        setCtrlLatched(false);
        opts.sendRaw(ctrlByte);
      }
    };

    const onInput = (e: Event) => {
      const ie = e as InputEvent;
      if (ctrlLatched && !handledInBeforeInput) {
        const inserted = ie.data ?? el.value.slice(-1);
        const ctrlByte = applyStickyCtrl(inserted);
        if (ctrlByte) {
          if (isProxy) {
            el.value = "";
          } else {
            const pos = el.selectionStart ?? el.value.length;
            if (pos > 0) {
              el.value = el.value.slice(0, pos - 1) + el.value.slice(pos);
              el.setSelectionRange(pos - 1, pos - 1);
            }
          }
          setCtrlLatched(false);
          opts.sendRaw(ctrlByte);
          return;
        }
      }
      handledInBeforeInput = false;

      // In raw proxy mode, forward normal phone typing straight to the PTY.
      if (isProxy && el.value.length > 0) {
        const payload = el.value;
        el.value = "";
        if (!scrollLocked) {
          opts.sendRaw(payload);
        }
      }
    };

    const onKeyDown = (e: KeyboardEvent) => {
      if (isProxy && !scrollLocked) {
        if (e.key === "Enter") {
          e.preventDefault();
          opts.sendRaw("\r");
        } else if (e.key === "Backspace") {
          e.preventDefault();
          opts.sendRaw("\x7f");
        }
      }
    };

    el.addEventListener("beforeinput", onBeforeInput);
    el.addEventListener("input", onInput);
    el.addEventListener("keydown", onKeyDown);

    return () => {
      el.removeEventListener("beforeinput", onBeforeInput);
      el.removeEventListener("input", onInput);
      el.removeEventListener("keydown", onKeyDown);
    };
  };

  const cleanupPrimary = attachCtrlInterceptor(opts.textarea, false);
  const cleanupProxy = attachCtrlInterceptor(rawProxy, true);

  return {
    element: bar,
    rawProxy,
    isCtrlLatched: () => ctrlLatched,
    isScrollLocked: () => scrollLocked,
    setScrollLocked,
    focusActiveInput,
    destroy: () => {
      cleanupPrimary();
      cleanupProxy();
      bar.remove();
      rawProxy.remove();
    },
  };
}

function configureMobileInputAttrs(el: HTMLTextAreaElement) {
  el.setAttribute("autocomplete", "off");
  el.setAttribute("autocorrect", "off");
  el.setAttribute("autocapitalize", "none");
  el.setAttribute("spellcheck", "false");
  el.setAttribute("enterkeyhint", "send");
}

/**
 * Syncs `--app-h` and `--vv-offset-top` CSS variables with `window.visualViewport`
 * on BOTH `resize` and `scroll` so iOS Safari and Android Chrome keep `.composer`
 * and `.kbd-bar` pinned cleanly above the virtual keyboard.
 */
export function installVisualViewportSync(): () => void {
  const root = document.documentElement;
  const update = () => {
    const vv = window.visualViewport;
    if (!vv) {
      root.style.setProperty("--app-h", `${window.innerHeight}px`);
      root.style.setProperty("--vv-offset-top", "0px");
      return;
    }
    root.style.setProperty("--app-h", `${Math.round(vv.height)}px`);
    root.style.setProperty("--vv-offset-top", `${Math.round(vv.offsetTop)}px`);
  };

  update();
  window.addEventListener("resize", update);
  window.visualViewport?.addEventListener("resize", update);
  window.visualViewport?.addEventListener("scroll", update);

  return () => {
    window.removeEventListener("resize", update);
    window.visualViewport?.removeEventListener("resize", update);
    window.visualViewport?.removeEventListener("scroll", update);
  };
}
