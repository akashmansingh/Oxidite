import { PROTOCOL_VERSION, pack, parse, bytesOf, idBytes, idStr, type BackendSpec, type Msg, type SessionState, type TermSize } from "./protocol";

export type Link = "ok" | "retry" | "down";
export interface Handlers {
  output(b: Uint8Array, done: () => void): void;
  reset(): void;
  state(s: SessionState): void;
  link(l: Link): void;
  title(t: string): void;
  prompt(kind: "hostkey" | "auth", m: Msg): void;
  created(id: string): void;
  error(msg: string): void;
  exit(code: number | null): void;
}

/** One browser connection to one session. Reconnects with full jitter and resumes from the last offset. */
export class Connection {
  id: string | null = null;
  offset = 0;
  private spec: BackendSpec | null = null;
  private ws?: WebSocket;
  private attempt = 0;
  private done = false;
  private lastPong = 0;
  private pingT?: number;
  private ackT?: number;
  private acked = 0;
  private retryT?: number;

  constructor(private token: string, private h: Handlers, private size: () => TermSize) {}

  open(t: { id: string } | { spec: BackendSpec }): void {
    if ("id" in t) this.id = t.id; else this.spec = t.spec;
    this.connect();
  }
  input(b: Uint8Array): void { this.ws?.send(pack("Input", b)); }
  resize(s: TermSize): void { this.ws?.send(pack("Resize", s)); }
  trust(accept: boolean): void { this.ws?.send(pack("HostKeyDecision", { accept })); }
  answer(answers: string[]): void { this.ws?.send(pack("AuthResponse", { answers })); }
  kill(): void { this.ws?.send(pack("Kill")); }
  detach(): void {
    this.done = true;
    this.ws?.send(pack("Detach"));
    this.ws?.close();
    this.stop();
  }

  private stop(): void { clearInterval(this.pingT); clearTimeout(this.retryT); clearTimeout(this.ackT); }

  private connect(): void {
    this.h.link("retry");
    const ws = (this.ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`));
    ws.binaryType = "arraybuffer";
    ws.onopen = () => ws.send(pack("Hello", { protocol_version: PROTOCOL_VERSION, token: this.token }));
    ws.onmessage = (e: MessageEvent<ArrayBuffer>) => this.on(parse(e.data));
    ws.onclose = () => { clearInterval(this.pingT); if (!this.done) this.retry(); };
  }

  private retry(): void {
    this.attempt += 1;
    this.h.link("retry");
    this.h.error(`Connection lost. Retrying (attempt ${this.attempt}).`);
    const ceiling = Math.min(30_000, 1000 * 2 ** this.attempt);
    this.retryT = window.setTimeout(() => this.connect(), Math.max(250, Math.random() * ceiling));
  }

  private attach(): void {
    this.ws?.send(pack("Attach", { session: this.id ? idBytes(this.id) : null, last_offset: this.id ? this.offset || null : null, mode: "Writer" }));
  }

  private ackTo(n: number): void {
    this.acked = n;
    if (this.ackT) return;
    this.ackT = window.setTimeout(() => { this.ackT = undefined; this.ws?.send(pack("Ack", { offset: this.acked })); }, 30);
  }

  private on(m: Msg): void {
    switch (m.t) {
      case "Welcome":
        this.attempt = 0; this.lastPong = Date.now(); this.h.link("ok"); this.h.error("");
        this.pingT = window.setInterval(() => {
          if (Date.now() - this.lastPong > 35_000) { this.ws?.close(); return; }
          this.ws?.send(pack("Ping", { nonce: Date.now() }));
        }, 15_000);
        if (this.id) this.attach(); else this.ws?.send(pack("Create", { backend: this.spec, size: this.size() }));
        break;
      case "Created": this.id = idStr(m.session); this.h.created(this.id); this.attach(); break;
      case "Attached": if ((m.start as Record<string, unknown>)["Snapshot"]) this.h.reset(); break;
      case "Snapshot": { const b = bytesOf(m.bytes); this.offset = m.offset as number; this.h.output(b, () => this.ackTo(this.offset)); break; }
      case "Output": {
        const b = bytesOf(m.bytes), end = (m.offset as number) + b.length;
        this.offset = end;
        this.h.output(b, () => this.ackTo(end));
        if (b.includes(0x1b) && new TextDecoder().decode(b).includes("\x1b[6n")) {
          this.input(new TextEncoder().encode("\x1b[1;1R"));
        }
        break;
      }
      case "State": this.h.state(m.state as SessionState); break;
      case "HostKeyPrompt": this.h.prompt("hostkey", m); break;
      case "AuthPrompt": this.h.prompt("auth", m); break;
      case "Title": this.h.title(m.v as string); break;
      case "Pong": this.lastPong = Date.now(); break;
      case "Exit": this.done = true; this.stop(); this.h.exit(m.code as number | null); break;
      case "Error": this.h.error(String(m.message)); break;
    }
  }
}
