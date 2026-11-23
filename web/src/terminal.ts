import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";

/** Slab palette: deep malachite ground, oxidized-copper ANSI colors. */
const theme = {
  background: "#0d2a2c", foreground: "#d8e6da", cursor: "#f2c14e", selectionBackground: "#2f6f8f99",
  black: "#0d2a2c", red: "#e0644f", green: "#6fcf97", yellow: "#e3b341", blue: "#5b9dff", magenta: "#c490e4", cyan: "#58d6c9", white: "#d8e6da",
  brightBlack: "#52716b", brightRed: "#ff8a73", brightGreen: "#9be8b8", brightYellow: "#f6d57a", brightBlue: "#8ab8ff", brightMagenta: "#dcb4f2", brightCyan: "#8ff0e4", brightWhite: "#f4faf2",
};

export function makeTerminal(host: HTMLElement): { term: Terminal; fit: FitAddon } {
  const term = new Terminal({ fontFamily: '"Sometype Mono", ui-monospace, monospace', fontSize: 14, cursorBlink: true, scrollback: 5000, theme });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(host);
  try {
    const gl = new WebglAddon();
    gl.onContextLoss(() => gl.dispose());
    term.loadAddon(gl);
  } catch { /* falls back to the default renderer */ }
  fit.fit();
  return { term, fit };
}
