/**
 * Mobile sleep & network-switch wake probe.
 *
 * Browser WebSocket APIs cannot send or observe protocol-level Ping/Pong frames, so
 * on `window.online` and `document.visibilitychange` ("visible") we send an
 * application-level `ClientMsg::Ping` and force an immediate reconnect if no
 * `ServerMsg::Pong` arrives within `deadlineMs` (default 2000ms).
 */
export interface WakeProbeOptions {
  isSocketOpen: () => boolean;
  sendAppPing: () => void;
  reconnectNow: () => void;
  deadlineMs?: number;
}

export interface WakeProbeHandle {
  notifyPongReceived: () => void;
  destroy: () => void;
}

export function installWakeProbe(opts: WakeProbeOptions): WakeProbeHandle {
  const deadlineMs = opts.deadlineMs ?? 2000;
  let pendingTimer: number | null = null;

  const clearPending = () => {
    if (pendingTimer !== null) {
      window.clearTimeout(pendingTimer);
      pendingTimer = null;
    }
  };

  const probe = () => {
    clearPending();
    if (!opts.isSocketOpen()) {
      opts.reconnectNow();
      return;
    }
    try {
      opts.sendAppPing();
    } catch {
      opts.reconnectNow();
      return;
    }
    pendingTimer = window.setTimeout(() => {
      pendingTimer = null;
      opts.reconnectNow();
    }, deadlineMs);
  };

  const onOnline = () => probe();
  const onVisibility = () => {
    if (document.visibilityState === "visible") {
      probe();
    }
  };

  window.addEventListener("online", onOnline);
  document.addEventListener("visibilitychange", onVisibility);

  return {
    notifyPongReceived: clearPending,
    destroy: () => {
      clearPending();
      window.removeEventListener("online", onOnline);
      document.removeEventListener("visibilitychange", onVisibility);
    },
  };
}
