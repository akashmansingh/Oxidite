const GLYPHS: Record<string, string[]> = {
  A: ["01110", "10001", "11111", "10001", "10001"],
  B: ["11110", "10001", "11110", "10001", "11110"],
  C: ["01111", "10000", "10000", "10000", "01111"],
  D: ["11110", "10001", "10001", "10001", "11110"],
  E: ["11111", "10000", "11110", "10000", "11111"],
  F: ["11111", "10000", "11110", "10000", "10000"],
  G: ["01111", "10000", "10111", "10001", "01111"],
  H: ["10001", "10001", "11111", "10001", "10001"],
  I: ["11111", "00100", "00100", "00100", "11111"],
  J: ["00111", "00010", "00010", "10010", "01100"],
  K: ["10001", "10010", "11100", "10010", "10001"],
  L: ["10000", "10000", "10000", "10000", "11111"],
  M: ["10001", "11011", "10101", "10001", "10001"],
  N: ["10001", "11001", "10101", "10011", "10001"],
  O: ["01110", "10001", "10001", "10001", "01110"],
  P: ["11110", "10001", "11110", "10000", "10000"],
  Q: ["01110", "10001", "10001", "10010", "01101"],
  R: ["11110", "10001", "11110", "10010", "10001"],
  S: ["01111", "10000", "01110", "00001", "11110"],
  T: ["11111", "00100", "00100", "00100", "00100"],
  U: ["10001", "10001", "10001", "10001", "01110"],
  V: ["10001", "10001", "10001", "01010", "00100"],
  W: ["10001", "10001", "10101", "11011", "10001"],
  X: ["10001", "01010", "00100", "01010", "10001"],
  Y: ["10001", "01010", "00100", "00100", "00100"],
  Z: ["11111", "00010", "00100", "01000", "11111"],
  "0": ["01110", "10011", "10101", "11001", "01110"],
  "1": ["00100", "01100", "00100", "00100", "01110"],
  "2": ["01110", "10001", "00110", "01000", "11111"],
  "3": ["11110", "00001", "01110", "00001", "11110"],
  "4": ["10010", "10010", "11111", "00010", "00010"],
  "5": ["11111", "10000", "11110", "00001", "11110"],
  "6": ["01110", "10000", "11110", "10001", "01110"],
  "7": ["11111", "00010", "00100", "01000", "01000"],
  "8": ["01110", "10001", "01110", "10001", "01110"],
  "9": ["01110", "10001", "01111", "00001", "01110"],
  "-": ["00000", "00000", "11110", "00000", "00000"],
  "_": ["00000", "00000", "00000", "00000", "11111"],
  ".": ["00000", "00000", "00000", "00000", "00100"],
};

export interface WordmarkResult {
  text: string;
  isArt: boolean;
  tier: "wide" | "compact" | "truncated" | "fallback";
}

/**
 * Renders `rawName` as an uppercase outlined-block wordmark (`█` body + `╔═╗║╚╝` offset outline)
 * guaranteed to fit strictly within `maxCols` display columns without wrapping mid-glyph.
 */
export function renderWordmark(rawName: string, maxCols: number): WordmarkResult {
  const trimmed = rawName.trim();
  if (!trimmed) {
    return { text: "W O R K S P A C E", isArt: false, tier: "fallback" };
  }

  const upper = trimmed.toUpperCase();
  const normalized = upper.replace(/\s+/g, "-");
  const isAsciiSupported = /^[A-Z0-9\-_.]+$/.test(normalized);

  if (!isAsciiSupported) {
    return { text: upper, isArt: false, tier: "fallback" };
  }

  // Tier 1: Wide outlined block (10 cols/char + 2-col gap + 1-col right shadow)
  const wideWidth = normalized.length * 10 + Math.max(0, normalized.length - 1) * 2 + 1;
  if (wideWidth <= maxCols) {
    return { text: buildShadowArt(normalized, 2, 2), isArt: true, tier: "wide" };
  }

  // Tier 2: Compact outlined block (5 cols/char + 1-col gap + 1-col right shadow)
  const compactWidth = normalized.length * 5 + Math.max(0, normalized.length - 1) + 1;
  if (compactWidth <= maxCols) {
    return { text: buildShadowArt(normalized, 1, 1), isArt: true, tier: "compact" };
  }

  // Tier 2b: Truncated compact block if >= 4 glyphs fit cleanly
  const maxCharsCompact = Math.floor(maxCols / 6);
  if (maxCharsCompact >= 4 && maxCols >= 24) {
    const clipped = normalized.slice(0, maxCharsCompact - 1) + ".";
    return { text: buildShadowArt(clipped, 1, 1), isArt: true, tier: "truncated" };
  }

  // Tier 3: Bold wide-tracked uppercase fallback for very narrow columns
  return { text: normalized.split("").join(" "), isArt: false, tier: "fallback" };
}

function buildShadowArt(word: string, scaleX: number, gap: number): string {
  const H = 5;
  const rows = Array.from({ length: H }, () => "");
  const fallbackBmp = GLYPHS["-"]!;

  for (let i = 0; i < word.length; i++) {
    const ch = word.charAt(i);
    const bmp = GLYPHS[ch] ?? fallbackBmp;
    for (let r = 0; r < H; r++) {
      if (i > 0) rows[r] = (rows[r] ?? "") + "0".repeat(gap);
      const rowBits = bmp[r] ?? "00000";
      rows[r] =
        (rows[r] ?? "") +
        rowBits
          .split("")
          .map((c) => c.repeat(scaleX))
          .join("");
    }
  }

  const W = (rows[0] ?? "").length;
  const solid = (r: number, c: number) =>
    r >= 0 && r < H && c >= 0 && c < W && (rows[r]?.charAt(c) ?? "0") === "1";

  const shadow = (r: number, c: number) =>
    !solid(r, c) && (solid(r - 1, c) || solid(r, c - 1) || solid(r - 1, c - 1));

  const out: string[] = [];
  for (let r = 0; r <= H; r++) {
    let line = "";
    for (let c = 0; c <= W; c++) {
      if (solid(r, c)) {
        line += "█";
      } else if (shadow(r, c)) {
        const n = shadow(r - 1, c);
        const s = shadow(r + 1, c);
        const w = shadow(r, c - 1);
        const e = shadow(r, c + 1);
        const fromTop = solid(r - 1, c);
        const fromLeft = solid(r, c - 1);
        const fromDiag = solid(r - 1, c - 1);

        if (fromTop && fromLeft) {
          line += "╔";
        } else if (fromTop) {
          if (!w && !fromDiag) line += "╚";
          else if (!e && s) line += "╗";
          else line += "═";
        } else if (fromLeft) {
          if (!n && !fromDiag) line += "╗";
          else if (!s && e) line += "╚";
          else line += "║";
        } else {
          line += "╝";
        }
      } else {
        line += " ";
      }
    }
    out.push(line.trimEnd());
  }
  return out.join("\n");
}
