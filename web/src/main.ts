import "./style.css";
import { makeTerminal } from "./terminal";
import { Connection, type Handlers } from "./connection";
import { stateName, type BackendSpec, type Msg, type SessionState } from "./protocol";

const $ = <T extends HTMLElement = HTMLElement>(s: string): T => document.querySelector<T>(s) as T;
const enc = new TextEncoder();
const { term, fit } = makeTerminal($("#term"));
let token = sessionStorage.getItem("oxidite.token") ?? "";
let conn: Connection | null = null;
let raf = 0;

const say = (s: string): void => { $("#msg").textContent = s; };
const setB = (v: string): void => { $("#chain").dataset.b = v; };

const stateText = (s: SessionState): string => {
  const n = stateName(s);
  if (typeof s === "object" && "Reconnecting" in s) return `Reconnecting to the host (attempt ${s.Reconnecting.attempt})`;
  if (typeof s === "object" && "Failed" in s) return `Failed: ${s.Failed.reason}`;
  return n === "Detached" ? "Running with no viewers" : n;
};

const handlers: Handlers = {
  output: (b, done) => {
    term.write(b, done);
    if (!raf) raf = requestAnimationFrame(() => { raf = 0; $("#off").textContent = `${((conn?.offset ?? 0) / 1024).toFixed(0)} KiB received`; });
  },
  reset: () => term.reset(),
  link: (l) => { $("#chain").dataset.a = l; },
  title: (t) => { document.title = `${t} | Oxidite`; },
  error: (m) => { if (m) say(m); },
  created: () => void refresh(),
  exit: (c) => { setB("down"); say(c === null ? "Session ended." : `Session ended with exit code ${c}.`); void refresh(); },
  state: (s) => {
    const n = stateName(s);
    setB(n === "Degraded" ? "warn" : n === "Reconnecting" ? "retry" : n === "Failed" || n === "Exited" ? "down" : "ok");
    say(stateText(s));
  },
  prompt: (kind, m) => ask(kind, m),
};

function ask(kind: "hostkey" | "auth", m: Msg): void {
  const dlg = $<HTMLDialogElement>("#prompt"), body = $("#pbody"), btns = $("#pbtns");
  body.replaceChildren(); btns.replaceChildren();
  const btn = (label: string, value: string, alt = false): void => {
    const b = document.createElement("button");
    b.className = alt ? "btn alt" : "btn"; b.value = value; b.textContent = label; btns.append(b);
  };
  if (kind === "hostkey") {
    $("#prompt h2").textContent = "Trust this host?";
    const p = document.createElement("p"), c = document.createElement("code");
    p.textContent = `First connection to ${String(m.host)}. Check this fingerprint matches the host before you continue.`;
    c.textContent = String(m.fingerprint); body.append(p, c);
    btn("Cancel", "no", true); btn("Trust and connect", "yes");
  } else {
    $("#prompt h2").textContent = "The host wants more";
    for (const q of m.prompts as string[]) {
      const l = document.createElement("label"), i = document.createElement("input");
      i.type = "password"; i.autocomplete = "off"; l.append(q, i); body.append(l);
    }
    btn("Continue", "yes");
  }
  dlg.onclose = () => {
    const ok = dlg.returnValue === "yes";
    if (kind === "hostkey") conn?.trust(ok);
    else conn?.answer([...body.querySelectorAll("input")].map((i) => i.value));
  };
  dlg.showModal();
}

function begin(t: { id: string } | { spec: BackendSpec }, label: string): void {
  conn?.detach();
  term.reset(); say("Connecting");
  $("#node-t").textContent = label;
  conn = new Connection(token, handlers, () => ({ rows: term.rows, cols: term.cols }));
  conn.open(t);
  term.focus();
}

async function refresh(): Promise<void> {
  try {
    const r = await fetch("/api/sessions", { headers: { Authorization: `Bearer ${token}` } });
    if (!r.ok) throw new Error(String(r.status));
    // Assumed shape: [{ id: string, state: SessionState, label?: string }]
    const list = (await r.json()) as { id: string; state: SessionState; label?: string }[];
    $("#empty").hidden = list.length > 0;
    $("#cores").replaceChildren(...list.map((s, i) => {
      const b = document.createElement("button"), n = stateName(s.state);
      b.className = "core"; b.dataset.s = n; b.style.setProperty("--i", String(i));
      b.setAttribute("aria-current", String(s.id === conn?.id));
      const name = document.createElement("b"), st = document.createElement("span");
      name.textContent = s.label ?? s.id.slice(0, 8); st.textContent = n;
      b.append(name, st);
      b.onclick = () => begin({ id: s.id }, s.label ?? "session");
      return b;
    }));
  } catch { say("Could not load sessions. Check the token and that the gateway is running."); }
}

term.onData((d) => conn?.input(enc.encode(d)));
term.onBinary((d) => conn?.input(Uint8Array.from(d, (c) => c.charCodeAt(0))));
term.onResize(({ rows, cols }) => conn?.resize({ rows, cols }));
new ResizeObserver(() => fit.fit()).observe($("#slab"));

$("#new").onclick = () => $<HTMLDialogElement>("#newdlg").showModal();
$<HTMLDialogElement>("#newdlg").onclose = (e) => {
  const dlg = e.currentTarget as HTMLDialogElement;
  if (dlg.returnValue !== "go") return;
  const f = new FormData($<HTMLFormElement>("#newdlg form"));
  const str = (k: string): string => String(f.get(k) ?? "");
  if (str("kind") === "ssh") {
    const spec: BackendSpec = { Ssh: { host: str("host"), port: Number(str("port")) || 22, user: str("user"), auth: { Password: str("pw") }, multiplexer: f.get("tmux") ? "Tmux" : "None" } };
    begin({ spec }, str("host"));
  } else begin({ spec: { LocalPty: { shell: null, cwd: null } } }, "local shell");
};

const gate = $<HTMLDialogElement>("#gate");
gate.onclose = () => {
  token = $<HTMLInputElement>("#tok").value.trim();
  sessionStorage.setItem("oxidite.token", token);
  void refresh();
};
if (token) void refresh(); else gate.showModal();
