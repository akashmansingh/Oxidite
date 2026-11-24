import { describe, expect, test } from "bun:test";
import { idBytes, idStr } from "./protocol";

const ID = "10f709d5-1111-4222-8333-444455556666";

describe("session ids", () => {
  test("bytes from the wire become the canonical string", () => expect(idStr(idBytes(ID))).toBe(ID));
  test("a string passes through unchanged", () => expect(idStr(ID)).toBe(ID));
  test("a string becomes exactly 16 bytes", () => {
    const b = idBytes(ID);
    expect(b.length).toBe(16);
    expect(b[0]).toBe(0x10);
    expect(b[15]).toBe(0x66);
  });
});
