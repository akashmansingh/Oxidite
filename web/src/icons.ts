// Inline SVG icons built with the DOM API. The gateway's CSP blocks data: images, and inline SVG elements need no network or style attributes.
export type IconName =
  | "plus"
  | "split"
  | "diff"
  | "expand"
  | "devices"
  | "sessions"
  | "changes"
  | "folder"
  | "branch"
  | "file"
  | "prompt"
  | "refresh"
  | "chevronDown"
  | "resizeDiag"
  | "play"
  | "pause";

const NS = "http://www.w3.org/2000/svg";

// Each entry is a list of [tag, attributes] drawn on a 16x16 grid with a 1.5px round stroke.
const SHAPES: Record<IconName, [string, Record<string, string>][]> = {
  plus: [["path", { d: "M8 3v10M3 8h10" }]],
  split: [
    ["rect", { x: "2", y: "3", width: "12", height: "10", rx: "2" }],
    ["path", { d: "M8 3v10" }],
  ],
  diff: [
    ["rect", { x: "2", y: "3", width: "12", height: "10", rx: "2" }],
    ["path", { d: "M10 3v10" }],
  ],
  expand: [["path", { d: "M9.5 2.5h4v4M6.5 13.5h-4v-4M13.5 2.5L9 7M2.5 13.5L7 9" }]],
  devices: [
    ["rect", { x: "2", y: "3", width: "12", height: "8", rx: "1.5" }],
    ["path", { d: "M5.5 14h5M8 11v3" }],
  ],
  sessions: [["path", { d: "M2.5 4h11M2.5 8h11M2.5 12h7" }]],
  changes: [
    ["path", { d: "M4 2h5l3 3v9H4z" }],
    ["path", { d: "M9 2v3h3M6.5 9h3M8 7.5v3" }],
  ],
  folder: [["path", { d: "M2 4.5a1 1 0 0 1 1-1h3l1.5 1.5H13a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z" }]],
  branch: [
    ["circle", { cx: "4.5", cy: "3.5", r: "1.5" }],
    ["circle", { cx: "4.5", cy: "12.5", r: "1.5" }],
    ["circle", { cx: "11.5", cy: "6", r: "1.5" }],
    ["path", { d: "M4.5 5v6M11.5 7.5c0 2.5-3 2.5-6.5 3.5" }],
  ],
  file: [
    ["path", { d: "M4 2h5l3 3v9H4z" }],
    ["path", { d: "M9 2v3h3" }],
  ],
  prompt: [["path", { d: "M4 4.5 8.5 8 4 11.5M9.5 12h3" }]],
  refresh: [
    ["path", { d: "M13.5 8a5.5 5.5 0 1 1-1.4-3.7" }],
    ["path", { d: "M13.5 2.5v3h-3" }],
  ],
  chevronDown: [["path", { d: "M4 6l4 4 4-4" }]],
  resizeDiag: [["path", { d: "M9.5 3.5h3v3M6.5 12.5h-3v-3M12.5 3.5L9 7M3.5 12.5L7 9" }]],
  play: [["path", { d: "M5 3.5l8 4.5-8 4.5z" }]],
  pause: [["path", { d: "M5 3.5v9M11 3.5v9" }]],
};

export function icon(name: IconName): SVGSVGElement {
  const svg = document.createElementNS(NS, "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("width", "16");
  svg.setAttribute("height", "16");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.5");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  svg.classList.add("ico");
  for (const [tag, attrs] of SHAPES[name]) {
    const node = document.createElementNS(NS, tag);
    for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, v);
    svg.append(node);
  }
  return svg;
}
