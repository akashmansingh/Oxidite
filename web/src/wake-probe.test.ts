import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { installWakeProbe } from "./wake-probe";

type Listener = (ev?: any) => void;

describe("wake-probe", () => {
  let prevWindow: unknown;
  let prevDocument: unknown;
  let winListeners: Map<string, Set<Listener>>;
  let docListeners: Map<string, Set<Listener>>;
  let visibilityState: "visible" | "hidden";

  beforeEach(() => {
    prevWindow = (globalThis as any).window;
    prevDocument = (globalThis as any).document;
    winListeners = new Map();
    docListeners = new Map();
    visibilityState = "visible";

    (globalThis as any).window = {
      setTimeout: globalThis.setTimeout.bind(globalThis),
      clearTimeout: globalThis.clearTimeout.bind(globalThis),
      addEventListener(type: string, fn: Listener) {
        if (!winListeners.has(type)) winListeners.set(type, new Set());
        winListeners.get(type)!.add(fn);
      },
      removeEventListener(type: string, fn: Listener) {
        winListeners.get(type)?.delete(fn);
      },
    };

    (globalThis as any).document = {
      get visibilityState() {
        return visibilityState;
      },
      addEventListener(type: string, fn: Listener) {
        if (!docListeners.has(type)) docListeners.set(type, new Set());
        docListeners.get(type)!.add(fn);
      },
      removeEventListener(type: string, fn: Listener) {
        docListeners.get(type)?.delete(fn);
      },
    };
  });

  afterEach(() => {
    (globalThis as any).window = prevWindow;
    (globalThis as any).document = prevDocument;
  });

  const emitDoc = (type: string) => {
    for (const fn of docListeners.get(type) ?? []) fn();
  };
  const emitWin = (type: string) => {
    for (const fn of winListeners.get(type) ?? []) fn();
  };

  test("probes on visibility restore and reconnects when deadline expires", async () => {
    let pings = 0;
    let reconnects = 0;
    const handle = installWakeProbe({
      isSocketOpen: () => true,
      sendAppPing: () => {
        pings += 1;
      },
      reconnectNow: () => {
        reconnects += 1;
      },
      deadlineMs: 25,
    });

    visibilityState = "visible";
    emitDoc("visibilitychange");
    expect(pings).toBe(1);
    expect(reconnects).toBe(0);

    await new Promise((r) => setTimeout(r, 45));
    expect(reconnects).toBe(1);
    handle.destroy();
  });

  test("cancels pending probe on Pong or server traffic during busy output", async () => {
    let reconnects = 0;
    const handle = installWakeProbe({
      isSocketOpen: () => true,
      sendAppPing: () => {},
      reconnectNow: () => {
        reconnects += 1;
      },
      deadlineMs: 30,
    });

    emitWin("online");
    handle.notifyTrafficReceived();
    await new Promise((r) => setTimeout(r, 50));
    expect(reconnects).toBe(0);

    emitWin("online");
    handle.notifyPongReceived();
    await new Promise((r) => setTimeout(r, 50));
    expect(reconnects).toBe(0);
    handle.destroy();
  });

  test("ignores online event when tab is hidden and reconnects immediately if socket closed", () => {
    let pings = 0;
    let reconnects = 0;
    let open = true;
    const handle = installWakeProbe({
      isSocketOpen: () => open,
      sendAppPing: () => {
        pings += 1;
      },
      reconnectNow: () => {
        reconnects += 1;
      },
      deadlineMs: 30,
    });

    visibilityState = "hidden";
    emitWin("online");
    expect(pings).toBe(0);
    expect(reconnects).toBe(0);

    open = false;
    visibilityState = "visible";
    emitDoc("visibilitychange");
    expect(reconnects).toBe(1);
    handle.destroy();
  });
});
