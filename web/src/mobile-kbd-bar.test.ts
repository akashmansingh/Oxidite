import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { createMobileKbdBar } from "./mobile-kbd-bar";

class FakeElement {
  tagName: string;
  className = "";
  children: FakeElement[] = [];
  attrs = new Map<string, string>();
  listeners = new Map<string, ((ev: any) => void)[]>();
  textContent = "";
  title = "";
  type = "";
  value = "";
  readOnly = false;
  selectionStart: number | null = 0;
  selectionEnd: number | null = 0;
  focused = false;
  removed = false;
  tabIndex = 0;
  classList = {
    _set: new Set<string>(),
    add: (...cls: string[]) => cls.forEach((c) => this.classList._set.add(c)),
    toggle: (c: string, force?: boolean) => {
      if (force === undefined) {
        if (this.classList._set.has(c)) {
          this.classList._set.delete(c);
          return false;
        }
        this.classList._set.add(c);
        return true;
      }
      if (force) this.classList._set.add(c);
      else this.classList._set.delete(c);
      return force;
    },
    contains: (c: string) => this.classList._set.has(c),
  };

  constructor(tag: string) {
    this.tagName = tag.toUpperCase();
  }

  append(...nodes: FakeElement[]) {
    this.children.push(...nodes);
  }
  remove() {
    this.removed = true;
  }
  setAttribute(k: string, v: string) {
    this.attrs.set(k, v);
  }
  getAttribute(k: string) {
    return this.attrs.get(k) ?? null;
  }
  focus() {
    this.focused = true;
  }
  blur() {
    this.focused = false;
  }
  setSelectionRange(s: number, e: number) {
    this.selectionStart = s;
    this.selectionEnd = e;
  }
  addEventListener(type: string, fn: (ev: any) => void) {
    if (!this.listeners.has(type)) this.listeners.set(type, []);
    this.listeners.get(type)!.push(fn);
  }
  removeEventListener(type: string, fn: (ev: any) => void) {
    const arr = this.listeners.get(type);
    if (!arr) return;
    const idx = arr.indexOf(fn);
    if (idx >= 0) arr.splice(idx, 1);
  }
  dispatch(type: string, ev: any = {}) {
    let defaultPrevented = false;
    const event = {
      type,
      cancelable: true,
      preventDefault: () => {
        defaultPrevented = true;
      },
      ...ev,
      get defaultPrevented() {
        return defaultPrevented;
      },
    };
    for (const fn of this.listeners.get(type) ?? []) fn(event);
    return event;
  }
}

describe("mobile-kbd-bar", () => {
  let prevDocument: unknown;
  let prevWindow: unknown;

  beforeEach(() => {
    prevDocument = (globalThis as any).document;
    prevWindow = (globalThis as any).window;
    (globalThis as any).window = {
      setTimeout: globalThis.setTimeout.bind(globalThis),
      clearTimeout: globalThis.clearTimeout.bind(globalThis),
      setInterval: globalThis.setInterval.bind(globalThis),
      clearInterval: globalThis.clearInterval.bind(globalThis),
    };
    (globalThis as any).document = {
      createElement: (tag: string) => new FakeElement(tag),
    };
  });

  afterEach(() => {
    (globalThis as any).document = prevDocument;
    (globalThis as any).window = prevWindow;
  });

  function setup(raw = false) {
    const sentRaw: string[] = [];
    const inserted: string[] = [];
    const ta = new FakeElement("textarea") as unknown as HTMLTextAreaElement;
    const bar = createMobileKbdBar({
      textarea: ta,
      sendRaw: (d) => sentRaw.push(d),
      insertText: (t) => inserted.push(t),
      isRawMode: () => raw,
    });
    const fixedButtons = (bar.element as unknown as FakeElement).children[0]!.children;
    const chips = (bar.element as unknown as FakeElement).children[1]!.children;
    const findBtn = (label: string) => fixedButtons.find((b) => b.textContent === label)!;
    return { bar, ta: ta as unknown as FakeElement, sentRaw, inserted, fixedButtons, chips, findBtn };
  }

  test("buttons are keyboard-focusable, accessible via aria-label, and activate on click without double-firing on pointerdown", () => {
    const { bar, fixedButtons, chips, sentRaw, findBtn } = setup();
    for (const b of [...fixedButtons, ...chips]) {
      expect(b.tabIndex).not.toBe(-1);
      expect((b.getAttribute("aria-label") ?? "").length).toBeGreaterThan(0);
    }

    const tabBtn = findBtn("Tab");
    tabBtn.dispatch("click"); // keyboard Enter/Space activation
    expect(sentRaw).toEqual(["\t"]);

    // pointerdown + browser click must fire only once
    tabBtn.dispatch("pointerdown");
    tabBtn.dispatch("click");
    expect(sentRaw).toEqual(["\t", "\t"]);
    bar.destroy();
  });

  test("sticky Ctrl clears on mapped keys, unmapped digits, action keys, chips, and clearCtrl()", () => {
    const { bar, ta, sentRaw, findBtn, chips } = setup(true);
    const ctrlBtn = findBtn("Ctrl");

    // Mapped key clears latch
    ctrlBtn.dispatch("click");
    expect(bar.isCtrlLatched()).toBe(true);
    ta.dispatch("beforeinput", { data: "c", isComposing: false });
    expect(sentRaw).toEqual(["\x03"]);
    expect(bar.isCtrlLatched()).toBe(false);

    // Unmapped digit clears latch instead of staying stuck
    ctrlBtn.dispatch("click");
    expect(bar.isCtrlLatched()).toBe(true);
    ta.dispatch("beforeinput", { data: "5", isComposing: false });
    expect(bar.isCtrlLatched()).toBe(false);

    // Esc clears latch
    ctrlBtn.dispatch("click");
    expect(bar.isCtrlLatched()).toBe(true);
    findBtn("Esc").dispatch("click");
    expect(bar.isCtrlLatched()).toBe(false);
    expect(sentRaw).toEqual(["\x03", "\x1b"]);

    // Symbol chip clears latch
    ctrlBtn.dispatch("click");
    expect(bar.isCtrlLatched()).toBe(true);
    chips[0]!.dispatch("click");
    expect(bar.isCtrlLatched()).toBe(false);

    // Tab switch clearCtrl() clears latch
    ctrlBtn.dispatch("click");
    expect(bar.isCtrlLatched()).toBe(true);
    bar.clearCtrl();
    expect(bar.isCtrlLatched()).toBe(false);
    bar.destroy();
  });

  test("IME composition in rawProxy waits until compositionend before flushing", () => {
    const { bar, sentRaw } = setup(true);
    const proxy = bar.rawProxy as unknown as FakeElement;

    proxy.dispatch("compositionstart");
    proxy.value = "ni";
    proxy.dispatch("input", { isComposing: true, data: "ni" });
    expect(sentRaw).toEqual([]);
    expect(proxy.value).toBe("ni");

    proxy.value = "nihao";
    proxy.dispatch("compositionend", { data: "nihao" });
    expect(sentRaw).toEqual(["nihao"]);
    expect(proxy.value).toBe("");
    bar.destroy();
  });
});
