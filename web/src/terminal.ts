import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";

export type ThemeName = "onyx" | "verdigris" | "cuprite" | "patina";

export interface ThemeDef {
  label: string;
  acc: string;
  acc2: string;
  term: ITheme;
}

export const THEMES: Record<ThemeName, ThemeDef> = {
  onyx: {
    label: "Onyx",
    acc: "#5b8cff",
    acc2: "#5ad7ff",
    term: {
      background: "#000000",
      foreground: "#e8ebf5",
      cursor: "#5b8cff",
      selectionBackground: "#5b8cff44",
      black: "#1b1e2b",
      red: "#ff5470",
      green: "#34c47c",
      yellow: "#ffcb6b",
      blue: "#5b8cff",
      magenta: "#b69cff",
      cyan: "#5ad7ff",
      white: "#e8ebf5",
      brightBlack: "#7c8296",
      brightRed: "#ff758c",
      brightGreen: "#5fe29f",
      brightYellow: "#ffe08f",
      brightBlue: "#8ab0ff",
      brightMagenta: "#d0bfff",
      brightCyan: "#8aecff",
      brightWhite: "#ffffff",
    },
  },
  verdigris: {
    label: "Verdigris",
    acc: "#2fe0c0",
    acc2: "#e8b04b",
    term: {
      background: "#030a09",
      foreground: "#d9efe8",
      cursor: "#2fe0c0",
      selectionBackground: "#2fe0c044",
      black: "#12302b",
      red: "#e8743b",
      green: "#2fe0a0",
      yellow: "#e8b04b",
      blue: "#2fe0c0",
      magenta: "#9ad0ff",
      cyan: "#7ee0c4",
      white: "#d9efe8",
      brightBlack: "#6c9088",
      brightRed: "#f0905e",
      brightGreen: "#5ff0be",
      brightYellow: "#f0c474",
      brightBlue: "#60f0d6",
      brightMagenta: "#bedfff",
      brightCyan: "#a6f0db",
      brightWhite: "#ffffff",
    },
  },
  cuprite: {
    label: "Cuprite",
    acc: "#ff5a3c",
    acc2: "#ffb347",
    term: {
      background: "#0b0605",
      foreground: "#f2e3da",
      cursor: "#ff5a3c",
      selectionBackground: "#ff5a3c44",
      black: "#2b1913",
      red: "#ff5a3c",
      green: "#6fcf5a",
      yellow: "#ffb347",
      blue: "#ff7a59",
      magenta: "#f2a6c0",
      cyan: "#ffd27a",
      white: "#f2e3da",
      brightBlack: "#98796d",
      brightRed: "#ff7d66",
      brightGreen: "#8de07b",
      brightYellow: "#ffc770",
      brightBlue: "#ff9c82",
      brightMagenta: "#f7c2d4",
      brightCyan: "#ffe0a0",
      brightWhite: "#ffffff",
    },
  },
  patina: {
    label: "Patina",
    acc: "#1651c9",
    acc2: "#0f7c78",
    term: {
      background: "#cbdacf",
      foreground: "#0b2a24",
      cursor: "#1651c9",
      selectionBackground: "#1651c933",
      black: "#0b2a24",
      red: "#b3261e",
      green: "#1b8a4b",
      yellow: "#9a6a00",
      blue: "#1651c9",
      magenta: "#7a3fb0",
      cyan: "#0f7c78",
      white: "#d8e3da",
      brightBlack: "#46665d",
      brightRed: "#d9382f",
      brightGreen: "#25ad5e",
      brightYellow: "#be8400",
      brightBlue: "#2a6be0",
      brightMagenta: "#9859d4",
      brightCyan: "#16a19c",
      brightWhite: "#ffffff",
    },
  },
};

export function makeTerminal(host: HTMLElement, themeName: ThemeName = "onyx"): { term: Terminal; fit: FitAddon } {
  const selected = THEMES[themeName]?.term ?? THEMES.onyx.term;
  const term = new Terminal({
    fontFamily: '"Sometype Mono", ui-monospace, monospace',
    fontSize: 14,
    cursorBlink: true,
    scrollback: 5000,
    theme: selected,
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(host);
  try {
    const gl = new WebglAddon();
    gl.onContextLoss(() => gl.dispose());
    term.loadAddon(gl);
    // xterm throws from a pending frame when a terminal is disposed before its WebGL renderer.
    const dispose = term.dispose.bind(term);
    term.dispose = () => { try { gl.dispose(); } catch { /* already gone */ } dispose(); };
  } catch {
    /* falls back to default canvas renderer */
  }
  fit.fit();
  return { term, fit };
}
