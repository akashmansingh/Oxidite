import { describe, expect, test } from "bun:test";
import { applyStickyCtrl } from "./sticky-ctrl";

describe("applyStickyCtrl", () => {
  test("maps lowercase and uppercase letters to ASCII control bytes", () => {
    expect(applyStickyCtrl("a")).toBe("\x01");
    expect(applyStickyCtrl("c")).toBe("\x03");
    expect(applyStickyCtrl("C")).toBe("\x03");
    expect(applyStickyCtrl("d")).toBe("\x04");
    expect(applyStickyCtrl("l")).toBe("\x0c");
    expect(applyStickyCtrl("z")).toBe("\x1a");
  });

  test("maps bracket, backslash, and space control chords", () => {
    expect(applyStickyCtrl("[")).toBe("\x1b");
    expect(applyStickyCtrl("\\")).toBe("\x1c");
    expect(applyStickyCtrl("]")).toBe("\x1d");
    expect(applyStickyCtrl(" ")).toBe("\x00");
  });

  test("returns null for unmapped characters", () => {
    expect(applyStickyCtrl("")).toBeNull();
    expect(applyStickyCtrl("5")).toBeNull();
  });
});
