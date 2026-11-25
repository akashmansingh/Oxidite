import "./style.css";
import type { Terminal } from "@xterm/xterm";
import type { FitAddon } from "@xterm/addon-fit";
import { makeTerminal, THEMES, type ThemeName } from "./terminal";
import { Connection, type Handlers } from "./connection";
import { stateName, type BackendSpec, type Msg, type SessionState, type ShellCapability } from "./protocol";

// Gateway endpoints this UI expects:
//   GET  /api/sessions                      -> Sess[]
//   GET  /api/runtime/shells                -> ShellCapability[]
//   GET  /api/git/changes?session=ID        -> { branch, files: FileChg[] }
//   GET  /api/git/diff?session=ID&path=P    -> unified diff text
//   GET  /api/git/log?session=ID            -> { hash, subject }[]
//   POST /api/git/stage  { session, paths, staged }
//   POST /api/git/commit { session, message }
//   GET  /api/git/branches?session=ID       -> { current, branches: string[] }
//   POST /api/git/checkout { session, branch }
//   POST /api/git/fetch    { session }
// Missing endpoints show an empty state; nothing is faked.
type Sess = { id: string; state: SessionState; label?: string; workspace?: string };
type FileChg = { path: string; add: number; del: number; staged: boolean };
interface Tab {
  id: string | null;
  label: string;
  host: string;
  el: HTMLElement;
  term: Terminal;
  fit: FitAddon;
  conn: Connection;
  a: string;
  b: string;
  msg: string;
  pair: Tab | null;
}

const $ = <T extends HTMLElement = HTMLElement>(s: string): T => document.querySelector<T>(s) as T;
const el = <K extends keyof HTMLElementTagNameMap>(tag: K, cls = "", text = ""): HTMLElementTagNameMap[K] => {
  const e = document.createElement(tag);
  e.className = cls;
  e.textContent = text;
  return e;
};
const note = (n: HTMLElement, text: string): void => n.replaceChildren(el("p", "note", text));
const enc = new TextEncoder();
let token = sessionStorage.getItem("oxidite.token") ?? "";
// The server labels every session "session-<id>". Remember what this browser called the ones it created.
const names: Record<string, string> = (() => { try { return JSON.parse(localStorage.getItem("oxidite.names") ?? "{}"); } catch { return {}; } })();
function uniqueLabel(base: string): string {
  const used = new Set([...Object.values(names), ...tabs.map((t) => t.label)]);
  for (let i = 1; ; i++) { const l = i === 1 ? base : `${base} ${i}`; if (!used.has(l)) return l; }
}
const rememberName = (id: string, label: string): void => {
  names[id] = label;
  localStorage.setItem("oxidite.names", JSON.stringify(Object.fromEntries(Object.entries(names).slice(-100))));
};
let theme = (localStorage.getItem("oxidite.theme") as ThemeName | null) ?? "onyx";
const tabs: Tab[] = [];
let cur: Tab | null = null;
let raf = 0;
let splitDir: "h" | "v" = "h";
let splitNext = false;
let autoHid = false;
const small = (): boolean => matchMedia("(max-width: 1100px)").matches;

// Activity feed: real connection changes seen by this browser, newest first, capped at 20.
const events: { at: Date; tab: Tab; text: string; b: string }[] = [];
function logEvent(tab: Tab, text: string): void {
  if (events[0]?.tab === tab && events[0].text === text) return;
  events.unshift({ at: new Date(), tab, text, b: tab.b });
  events.length = Math.min(events.length, 20);
  renderActivity();
}
function renderActivity(): void {
  const ok = tabs.filter((t) => t.b === "ok").length;
  $("#actsum").textContent = !tabs.length ? "" : tabs.length > ok ? `${ok} ok, ${tabs.length - ok} flagged` : `${ok} ok`;
  const list = $("#actlist");
  if (!events.length) {
    note(list, "Connection changes show up here.");
    return;
  }
  list.replaceChildren(
    ...events.map((e) => {
      const b = el("button", "ev"),
        when = e.at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
      b.dataset.b = e.b;
      b.append(el("i", "dot"), el("b", "", e.tab.label), el("span", "", `${e.text}, ${when}`));
      b.onclick = () => {
        if (tabs.includes(e.tab)) select(e.tab);
      };
      return b;
    }),
  );
}
function fitVisible(): void {
  for (const t of tabs) if (!t.el.hidden) t.fit.fit();
}

async function api<T>(path: string, body?: unknown): Promise<T> {
  const r = await fetch(path, {
    method: body ? "POST" : "GET",
    headers: { Authorization: `Bearer ${token}`, ...(body ? { "Content-Type": "application/json" } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  });
  if (r.status === 401) {
    token = "";
    sessionStorage.removeItem("oxidite.token");
    $<HTMLDialogElement>("#gate").showModal();
    throw new Error("401");
  }
  if (!r.ok) throw new Error(String(r.status));
  return (r.headers.get("content-type")?.includes("json") ? r.json() : r.text()) as Promise<T>;
}

const stateText = (s: SessionState): string => {
  if (typeof s === "object" && "Reconnecting" in s) return `Reconnecting to the host (attempt ${s.Reconnecting.attempt})`;
  if (typeof s === "object" && "Failed" in s) return `Failed: ${s.Failed.reason}`;
  return stateName(s) === "Detached" ? "Running with no viewers" : stateName(s);
};

function paint(): void {
  const c = $("#chain");
  c.dataset.a = cur?.a ?? "retry";
  c.dataset.b = cur?.b ?? "ok";
  $("#node-t").textContent = cur?.host ?? "host";
  $("#msg").textContent = cur?.msg ?? "No session open";
  $("#off").textContent = cur ? `${(cur.conn.offset / 1024).toFixed(0)} KiB` : "";
}

function renderTabs(): void {
  const add = el("button", "ib addtab");
  add.setAttribute("aria-label", "New session");
  add.onclick = () => $("#new").click();
  $("#tabs").replaceChildren(
    ...tabs.map((t) => {
      const b = el("div", "tab"),
        main = el("button", "tabmain", t.label),
        x = el("button", "ib x");
      b.setAttribute("role", "tab");
      b.setAttribute("aria-selected", String(t === cur || t === cur?.pair));
      b.dataset.b = t.b;
      if (cur?.pair && (t === cur || t === cur.pair)) b.dataset.pair = "1";
      b.addEventListener("auxclick", (e) => {
        if (e.button === 1) closeTab(t);
      });
      main.onclick = () => select(t);
      x.setAttribute("aria-label", `Close ${t.label}. The session keeps running.`);
      x.onclick = () => closeTab(t);
      b.append(el("i", "dot"), main, x);
      return b;
    }),
    add,
  );
  $("#tabs").querySelector('[aria-selected="true"]')?.scrollIntoView({ inline: "nearest", block: "nearest" });
}

function select(t: Tab | null): void {
  cur = t;
  for (const x of tabs) x.el.hidden = !(x === t || (t !== null && x === t.pair));
  $("#panes").classList.toggle("has", tabs.length > 0);
  $("#panes").classList.toggle("split", !!t?.pair);
  $("#panes").classList.toggle("v", splitDir === "v");
  $("#sp").textContent = !t?.pair ? "Split" : splitDir === "h" ? "Stack" : "Unsplit";
  if (t)
    requestAnimationFrame(() => {
      fitVisible();
      t.term.focus();
    });
  renderTabs();
  paint();
  void loadChanges();
  void refresh();
}

function closeTab(t: Tab): void {
  t.conn.detach();
  t.term.dispose();
  t.el.remove();
  tabs.splice(tabs.indexOf(t), 1);
  for (const x of tabs) if (x.pair === t) x.pair = null;
  renderActivity();
  select(cur === t ? (tabs.at(-1) ?? null) : cur);
}

function openTab(t: { id: string } | { spec: BackendSpec }, label: string, host: string): void {
  if ("spec" in t) label = uniqueLabel(label);
  const pane = el("div", "pane");
  $("#panes").append(pane);
  const { term, fit } = makeTerminal(pane, theme);
  const tab = {
    id: "id" in t ? t.id : null,
    label,
    host,
    el: pane,
    term,
    fit,
    a: "retry",
    b: "ok",
    msg: "Connecting",
    pair: null,
  } as unknown as Tab;
  const h: Handlers = {
    output: (b, done) => {
      term.write(b, done);
      if (!raf)
        raf = requestAnimationFrame(() => {
          raf = 0;
          paint();
        });
    },
    reset: () => term.reset(),
    link: (l) => {
      if (l === "retry" && tab.a === "ok") logEvent(tab, "Lost the gateway");
      if (l === "ok" && tab.a === "retry" && tab.msg !== "Connecting") logEvent(tab, "Back on the gateway");
      tab.a = l;
      paint();
    },
    title: (s) => {
      tab.label = s;
      renderTabs();
    },
    error: (m) => {
      if (m) {
        tab.msg = m;
        paint();
      }
    },
    created: (id) => {
      tab.id = id;
      rememberName(id, tab.label);
      void refresh();
      void loadChanges();
    },
    exit: (c) => {
      tab.b = "down";
      tab.msg = c === null ? "Session ended." : `Session ended with exit code ${c}.`;
      renderTabs();
      paint();
      logEvent(tab, "Session ended");
      void refresh();
    },
    state: (s) => {
      const n = stateName(s);
      tab.b = n === "Degraded" ? "warn" : n === "Reconnecting" ? "retry" : n === "Failed" || n === "Exited" ? "down" : "ok";
      tab.msg = stateText(s);
      renderTabs();
      paint();
      logEvent(tab, stateText(s));
    },
    prompt: (k, m) => ask(k, m, tab),
  };
  tab.conn = new Connection(token, h, () => ({ rows: term.rows, cols: term.cols }));
  term.onData((d) => tab.conn.input(enc.encode(d)));
  term.onBinary((d) => tab.conn.input(Uint8Array.from(d, (c) => c.charCodeAt(0))));
  term.onResize(({ rows, cols }) => tab.conn.resize({ rows, cols }));
  tabs.push(tab);
  tab.conn.open(t);
  if (splitNext && cur) {
    splitNext = false;
    cur.pair = tab;
    if (!small() && !$("#app").classList.contains("nodiff")) {
      $("#app").classList.add("nodiff");
      autoHid = true;
    }
    select(cur);
    requestAnimationFrame(() => tab.term.focus());
  } else select(tab);
}

function ask(kind: "hostkey" | "auth", m: Msg, tab: Tab): void {
  const dlg = $<HTMLDialogElement>("#prompt"),
    body = $("#pbody"),
    btns = $("#pbtns");
  body.replaceChildren();
  btns.replaceChildren();
  const btn = (label: string, value: string, alt = false): void => {
    const b = el("button", alt ? "btn alt" : "btn", label);
    b.value = value;
    btns.append(b);
  };
  if (kind === "hostkey") {
    $("#prompt h2").textContent = "Trust this host?";
    body.append(
      el("p", "", `First connection to ${String(m.host)}. Check this fingerprint matches the host before you continue.`),
      el("code", "", String(m.fingerprint)),
    );
    btn("Cancel", "no", true);
    btn("Trust and connect", "yes");
  } else {
    $("#prompt h2").textContent = "The host wants more";
    for (const q of m.prompts as string[]) {
      const l = el("label", "", q),
        i = el("input");
      i.type = "password";
      i.autocomplete = "off";
      l.append(i);
      body.append(l);
    }
    btn("Continue", "yes");
  }
  dlg.onclose = () => {
    if (kind === "hostkey") tab.conn.trust(dlg.returnValue === "yes");
    else tab.conn.answer([...body.querySelectorAll("input")].map((i) => i.value));
  };
  dlg.showModal();
}

async function refresh(): Promise<void> {
  if (!token) return;
  try {
    const list = await api<Sess[]>("/api/sessions");
    for (const id of Object.keys(names)) if (!list.some((x) => x.id === id)) delete names[id];
    const groups = new Map<string, Sess[]>();
    for (const s of list) {
      const g = s.workspace ?? "Sessions";
      groups.set(g, [...(groups.get(g) ?? []), s]);
    }
    $("#ws").replaceChildren(
      ...[...groups].map(([name, items]) => {
        const g = el("section", "wsg");
        g.append(el("h3", "", name));
        for (const s of items) {
          const b = el("button", "sess"),
            n = stateName(s.state),
            nm = names[s.id] ?? s.label ?? s.id.slice(0, 8);
          b.dataset.s = n;
          b.setAttribute("aria-current", String(cur !== null && cur.id === s.id));
          const isOpen = tabs.some((t) => t.id === s.id);
          b.dataset.open = isOpen ? "1" : "0";
          b.append(el("i", "dot"), el("b", "", nm), el("span", "", isOpen ? `${n}, open` : n));
          b.onclick = () => {
            const open = tabs.find((t) => t.id === s.id);
            if (open) select(open);
            else openTab({ id: s.id }, nm, nm);
          };
          g.append(b);
        }
        return g;
      }),
    );
    $("#empty-ws").hidden = list.length > 0;

    // Check PowerShell capabilities
    try {
      const shellsRes = await fetch("/api/runtime/shells", { headers: { Authorization: `Bearer ${token}` } });
      if (shellsRes.ok) {
        const caps = (await shellsRes.json()) as ShellCapability[];
        const hasPwsh = caps.some((c) => c.id === "SystemPowerShell" && c.available);
        const pwshLabel = document.querySelector<HTMLElement>("#lbl-powershell");
        if (pwshLabel) pwshLabel.hidden = !hasPwsh;
      }
    } catch {
      // Non-fatal probe failure
    }
  } catch {
    $("#msg").textContent = "Could not load sessions. Check the token and that the gateway is running.";
  }
}

async function loadChanges(): Promise<void> {
  const list = $("#files");
  if (!cur?.id) {
    note(list, "Open a session to see its changes.");
    return;
  }
  try {
    const r = await api<{ branch: string; files: FileChg[] }>(`/api/git/changes?session=${cur.id}`);
    const add = r.files.reduce((a, f) => a + f.add, 0),
      del = r.files.reduce((a, f) => a + f.del, 0);
    $("#bname").textContent = r.branch;
    $("#sum").textContent = `+${add} −${del}`;
    $("#count").textContent = String(r.files.length);
    list.replaceChildren(...(r.files.length ? r.files.map(fileRow) : [el("p", "note", "No changes in this workspace.")]));
  } catch {
    note(list, "Git data is not available. The gateway needs its /api/git endpoints.");
  }
}

function fileRow(f: FileChg): HTMLElement {
  const r = el("div", "file"),
    name = el("button", "fname"),
    cb = el("input"),
    i = f.path.lastIndexOf("/");
  cb.type = "checkbox";
  cb.checked = f.staged;
  cb.setAttribute("aria-label", `Stage ${f.path}`);
  cb.onchange = () => void stage([f.path], cb.checked);
  name.append(el("b", "", f.path.slice(i + 1)), el("small", "", i > 0 ? f.path.slice(0, i) : ""));
  name.onclick = () => void loadDiff(f.path);
  r.append(name, el("span", "add", `+${f.add}`), el("span", "del", `−${f.del}`), cb);
  return r;
}

async function stage(paths: string[], staged: boolean): Promise<void> {
  try {
    await api("/api/git/stage", { session: cur?.id, paths, staged });
  } catch {
    $("#msg").textContent = "Could not change staging.";
  }
  await loadChanges();
}

async function loadHistory(): Promise<void> {
  const box = $("#hist");
  try {
    const log = await api<{ hash: string; subject: string }[]>(`/api/git/log?session=${cur?.id}`);
    box.replaceChildren(
      ...log.map((c) => {
        const r = el("div", "commit");
        r.append(el("b", "", c.subject), el("small", "", c.hash.slice(0, 7)));
        return r;
      }),
    );
  } catch {
    note(box, "History is not available. The gateway needs /api/git/log.");
  }
}

async function loadDiff(path: string): Promise<void> {
  $("#dtitle").textContent = path;
  $("#app").classList.remove("nodiff");
  if (small()) {
    $("#app").classList.remove("show-chg");
    $("#app").classList.add("show-diff");
  }
  try {
    renderDiff(await api<string>(`/api/git/diff?session=${cur?.id}&path=${encodeURIComponent(path)}`));
  } catch {
    note($("#diff"), "Could not load this diff.");
  }
}

const KW =
  /^(?:fn|let|mut|pub|use|mod|impl|struct|enum|trait|match|if|else|for|while|loop|return|const|static|async|await|import|from|export|default|function|class|new|try|catch|throw|def|self|None|True|False|true|false|null|undefined|type|interface|extends|as|in|of|yield|package|func|var|where|ref|move|dyn|crate|super)$/;
const TOK =
  /(\/\/.*|#(?!\[).*)|("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|`(?:[^`\\]|\\.)*`)|(\b\d[\w.]*\b)|(\b[A-Za-z_]\w*\b)/g;

/** Light, language-agnostic highlighting: comments, strings, numbers, keywords, Capitalized types. */
function hl(text: string): DocumentFragment {
  const f = document.createDocumentFragment();
  let i = 0;
  for (const m of text.matchAll(TOK)) {
    const at = m.index ?? 0,
      t = m[0];
    if (at > i) f.append(text.slice(i, at));
    const cls = m[1] ? "tc" : m[2] ? "ts" : m[3] ? "tn" : KW.test(t) ? "tk" : /^[A-Z]/.test(t) ? "tt" : "";
    f.append(cls ? el("span", cls, t) : t);
    i = at + t.length;
  }
  f.append(text.slice(i));
  return f;
}

/** Unified diff -> side-by-side rows. Deletions and additions in a hunk are paired line by line. */
function renderDiff(text: string): void {
  const out = $("#diff"),
    rows: HTMLElement[] = [];
  let l = 0,
    r = 0,
    dels: string[] = [],
    adds: string[] = [],
    hunk = false;
  const cell = (cls: string, t: string): HTMLElement => {
    if (!cls.startsWith("c")) return el("span", cls, t);
    const c = el("span", cls);
    c.append(hl(t));
    return c;
  };
  const row = (ln: number | "", lt: string | undefined, rn: number | "", rt: string | undefined, ctx = false): void => {
    const d = el("div", "drow");
    d.append(
      cell("n", String(ln)),
      cell(ctx ? "c" : lt === undefined ? "c nil" : "c del", lt ?? ""),
      cell("n", String(rn)),
      cell(ctx ? "c" : rt === undefined ? "c nil" : "c add", rt ?? ""),
    );
    rows.push(d);
  };
  const flush = (): void => {
    for (let i = 0; i < Math.max(dels.length, adds.length); i++) {
      const a = dels[i],
        b = adds[i];
      row(a !== undefined ? ++l : "", a, b !== undefined ? ++r : "", b);
    }
    dels = [];
    adds = [];
  };
  for (const line of text.split("\n")) {
    const m = /^@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(line);
    if (m) {
      flush();
      hunk = true;
      l = Number(m[1]) - 1;
      r = Number(m[2]) - 1;
      rows.push(el("div", "hunk", line));
      continue;
    }
    if (!hunk) continue;
    if (line.startsWith("-")) dels.push(line.slice(1));
    else if (line.startsWith("+")) adds.push(line.slice(1));
    else if (line.startsWith(" ")) {
      flush();
      row(++l, line.slice(1), ++r, line.slice(1), true);
    }
  }
  flush();
  if (rows.length) out.replaceChildren(...rows);
  else note(out, "No textual changes in this file.");
}

function showTab(changes: boolean): void {
  $("#tab-c").setAttribute("aria-selected", String(changes));
  $("#tab-h").setAttribute("aria-selected", String(!changes));
  $("#files").hidden = !changes;
  $("#commitbox").hidden = !changes;
  $("#hist").hidden = changes;
  if (changes) void loadChanges();
  else void loadHistory();
}

function setTheme(n: ThemeName): void {
  theme = n;
  document.documentElement.dataset.theme = n;
  localStorage.setItem("oxidite.theme", n);
  for (const t of tabs) t.term.options.theme = THEMES[n].term;
  for (const b of document.querySelectorAll<HTMLButtonElement>(".sw"))
    b.setAttribute("aria-pressed", String(b.dataset.t === n));
}

$("#themes").replaceChildren(
  ...(Object.keys(THEMES) as ThemeName[]).map((n) => {
    const b = el("button", "sw");
    b.dataset.t = n;
    b.title = THEMES[n].label;
    b.setAttribute("aria-label", `${THEMES[n].label} theme`);
    b.style.setProperty("--sw", THEMES[n].acc);
    b.style.setProperty("--sw2", THEMES[n].acc2);
    b.onclick = () => setTheme(n);
    return b;
  }),
);
setTheme(theme);

$("#tab-c").onclick = () => showTab(true);
$("#tab-h").onclick = () => showTab(false);
$("#all").onclick = () => void stage([], true);
$("#dt").onclick = () => {
  autoHid = false;
  $("#app").classList.toggle(small() ? "show-diff" : "nodiff");
};
$("#fs").onclick = () => $("#app").classList.toggle("focus");
$("#sp").onclick = () => {
  if (!cur) {
    $("#msg").textContent = "Open a session first, then split it.";
    return;
  }
  if (!cur.pair) {
    splitNext = true;
    $<HTMLDialogElement>("#newdlg").showModal();
    return;
  }
  if (splitDir === "h") splitDir = "v";
  else {
    splitDir = "h";
    cur.pair = null;
    if (autoHid) {
      $("#app").classList.remove("nodiff");
      autoHid = false;
    }
  }
  select(cur);
};

async function loadBranches(): Promise<void> {
  const m = $("#branches");
  try {
    const r = await api<{ current: string; branches: string[] }>(`/api/git/branches?session=${cur?.id}`);
    m.replaceChildren(
      ...r.branches.map((n) => {
        const b = el("button", "", n);
        b.setAttribute("role", "option");
        b.setAttribute("aria-selected", String(n === r.current));
        b.onclick = async () => {
          try {
            await api("/api/git/checkout", { session: cur?.id, branch: n });
            $("#msg").textContent = `Switched to ${n}.`;
          } catch {
            $("#msg").textContent = `Could not switch to ${n}. Commit or stash your changes first.`;
          }
          await loadChanges();
        };
        return b;
      }),
    );
  } catch {
    note(m, "Branches are not available. The gateway needs /api/git/branches.");
  }
}
$("#branch").onclick = (e) => {
  e.stopPropagation();
  const m = $("#branches");
  m.hidden = !m.hidden;
  $("#branch").setAttribute("aria-expanded", String(!m.hidden));
  if (!m.hidden) void loadBranches();
};
document.addEventListener("click", () => {
  $("#branches").hidden = true;
  $("#branch").setAttribute("aria-expanded", "false");
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") $("#branches").hidden = true;
});
$("#fetch").onclick = async () => {
  const b = $<HTMLButtonElement>("#fetch");
  b.disabled = true;
  b.textContent = "Fetching";
  try {
    await api("/api/git/fetch", { session: cur?.id });
    $("#msg").textContent = "Fetched from the remote.";
  } catch {
    $("#msg").textContent = "Fetch failed. Check the remote and its credentials on the gateway.";
  }
  b.disabled = false;
  b.textContent = "Fetch";
  await loadChanges();
};
$("#commit").onclick = async () => {
  const m = $<HTMLTextAreaElement>("#cmsg");
  if (!m.value.trim()) {
    m.focus();
    return;
  }
  try {
    await api("/api/git/commit", { session: cur?.id, message: m.value });
    m.value = "";
  } catch {
    $("#msg").textContent = "Commit failed. Stage at least one file first.";
  }
  await loadChanges();
};
new ResizeObserver(fitVisible).observe($("#panes"));

// Drawers on small screens, drag handles on wide ones.
$("#sb").onclick = () => $("#app").classList.toggle("show-side");
$("#cg").onclick = () => {
  $("#app").classList.remove("show-diff");
  $("#app").classList.toggle("show-chg");
};
$("#term-pane").addEventListener("pointerdown", () => $("#app").classList.remove("show-side", "show-diff", "show-chg"));
$("#ws").addEventListener("click", () => $("#app").classList.remove("show-side"));
const root = document.documentElement;
for (const [sel, v, dir] of [
  ["#side", "--w-side", 1],
  ["#term-pane", "--w-term", 1],
  ["#chg", "--w-chg", -1],
] as const) {
  const saved = localStorage.getItem(`oxidite.${v}`);
  if (saved) root.style.setProperty(v, saved);
  const g = el("div", "grip");
  g.dataset.edge = dir === 1 ? "r" : "l";
  g.setAttribute("role", "separator");
  g.setAttribute("aria-orientation", "vertical");
  $(sel).append(g);
  g.onpointerdown = (e) => {
    g.setPointerCapture(e.pointerId);
    const x0 = e.clientX,
      w0 = $(sel).getBoundingClientRect().width;
    g.onpointermove = (m) => root.style.setProperty(v, `${Math.max(180, Math.min(900, w0 + dir * (m.clientX - x0)))}px`);
    g.onpointerup = () => {
      g.onpointermove = null;
      localStorage.setItem(`oxidite.${v}`, root.style.getPropertyValue(v));
      fitVisible();
    };
  };
}

$("#new").onclick = () => $<HTMLDialogElement>("#newdlg").showModal();
$<HTMLDialogElement>("#newdlg").onclose = () => {
  const dlg = $<HTMLDialogElement>("#newdlg");
  if (dlg.returnValue !== "go") {
    splitNext = false;
    return;
  }
  const f = new FormData($<HTMLFormElement>("#newdlg form")),
    s = (k: string): string => String(f.get(k) ?? "");
  if (s("kind") === "ssh") {
    openTab(
      {
        spec: {
          Ssh: {
            host: s("host"),
            port: Number(s("port")) || 22,
            user: s("user"),
            auth: { Password: s("pw") },
            multiplexer: f.get("tmux") ? "Tmux" : "None",
          },
        },
      },
      s("host"),
      s("host"),
    );
  } else if (s("kind") === "powershell") {
    openTab({ spec: { LocalPty: { shell: "SystemPowerShell", cwd: null } } }, "powershell", "powershell");
  } else {
    openTab({ spec: { LocalPty: { shell: null, cwd: null } } }, "local shell", "local shell");
  }
};

const gate = $<HTMLDialogElement>("#gate");
gate.onclose = () => {
  token = $<HTMLInputElement>("#tok").value.trim();
  sessionStorage.setItem("oxidite.token", token);
  void refresh();
};
if (token) void refresh();
else gate.showModal();
renderActivity();
void loadChanges();

// Keep the sidebar live while the tab is visible.
setInterval(() => { if (token && document.visibilityState === "visible") void refresh(); }, 5000);
