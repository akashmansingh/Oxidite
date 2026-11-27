import { describe, expect, test } from "bun:test";
import { CommandHistory, changeParts, keyAction, longestCommonPrefix, toPtyInput, type KeyInfo } from "./composer-model";

const key = (k: string, mods: Partial<KeyInfo> = {}): KeyInfo => ({
  key: k,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  metaKey: false,
  ...mods,
});
const at = (text: string, selStart = text.length, selEnd = selStart) => ({ text, selStart, selEnd });

describe("keyAction", () => {
  test("Enter sends, Shift+Enter keeps typing", () => {
    expect(keyAction(key("Enter"), at("ls"))).toBe("send");
    expect(keyAction(key("Enter", { shiftKey: true }), at("ls"))).toBe("newline");
  });

  test("Enter during IME composition is left alone", () => {
    expect(keyAction(key("Enter", { isComposing: true }), at("にほん"))).toBe("none");
  });

  test("Ctrl+C clears typed text, interrupts when empty, and copies a selection", () => {
    expect(keyAction(key("c", { ctrlKey: true }), at("sleep 5"))).toBe("clear");
    expect(keyAction(key("c", { ctrlKey: true }), at(""))).toBe("interrupt");
    expect(keyAction(key("c", { ctrlKey: true }), at("sleep 5", 0, 5))).toBe("none");
  });

  test("Ctrl+D only ends input on an empty editor", () => {
    expect(keyAction(key("d", { ctrlKey: true }), at(""))).toBe("eof");
    expect(keyAction(key("d", { ctrlKey: true }), at("x"))).toBe("none");
  });

  test("Escape dismisses suggestions without leaving the composer", () => {
    expect(keyAction(key("Escape"), at("ls"))).toBe("dismiss");
  });

  test("arrows browse history only at the first and last line", () => {
    expect(keyAction(key("ArrowUp"), at("one\ntwo", 2))).toBe("history-prev");
    expect(keyAction(key("ArrowUp"), at("one\ntwo", 6))).toBe("none");
    expect(keyAction(key("ArrowDown"), at("one\ntwo", 6))).toBe("history-next");
    expect(keyAction(key("ArrowDown"), at("one\ntwo", 1))).toBe("none");
  });

  test("Tab triggers forward and backward completion without leaving editor", () => {
    expect(keyAction(key("Tab"), at("cd O"))).toBe("tab-complete");
    expect(keyAction(key("Tab", { shiftKey: true }), at("cd O"))).toBe("tab-prev");
    expect(longestCommonPrefix(["OrderFlow\\", "Oxidite\\"])).toBe("O");
    expect(longestCommonPrefix(["src\\main.ts", "src\\blocks.ts"])).toBe("src\\");
  });
});

describe("toPtyInput", () => {
  test("ends a single line with one carriage return", () => {
    expect(toPtyInput("ls -la")).toBe("ls -la\r");
  });

  test("sends an empty editor as a bare Enter", () => {
    expect(toPtyInput("")).toBe("\r");
  });

  test("joins pasted lines with carriage returns and drops trailing newlines", () => {
    expect(toPtyInput("cd /tmp\r\nls\n\n")).toBe("cd /tmp\rls\r");
  });
});

describe("CommandHistory", () => {
  test("walks back and forward, then restores the draft", () => {
    const h = new CommandHistory();
    h.push("ls");
    h.push("pwd");
    expect(h.prev("git st")).toBe("pwd");
    expect(h.isBrowsing()).toBe(true);
    expect(h.prev("git st")).toBe("ls");
    expect(h.prev("git st")).toBeNull();
    expect(h.next()).toBe("pwd");
    expect(h.next()).toBe("git st");
    expect(h.isBrowsing()).toBe(false);
    expect(h.next()).toBeNull();
  });

  test("skips blanks and immediate repeats, and caps its size", () => {
    const h = new CommandHistory(2);
    h.push("   ");
    h.push("a");
    h.push("a");
    h.push("b");
    h.push("c");
    expect(h.prev("")).toBe("c");
    expect(h.prev("")).toBe("b");
    expect(h.prev("")).toBeNull();
  });

  test("returns null with no history", () => {
    expect(new CommandHistory().prev("x")).toBeNull();
  });
});

describe("changeParts", () => {
  test("formats counts for the changes chip", () => {
    expect(changeParts(1, 666, 0)).toEqual(["1", "+666", "−0"]);
  });
});
