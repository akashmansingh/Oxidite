/**
 * Safe DOM-only Markdown renderer (zero `innerHTML`).
 * Supports headings, paragraphs, bold, inline code, fenced code blocks,
 * bullet/numbered lists, task checkboxes, blockquotes, horizontal rules, and http(s) links.
 */
export function renderMarkdownToFragment(src: string): DocumentFragment {
  const frag = document.createDocumentFragment();
  const lines = src.replace(/\r\n/g, "\n").split("\n");
  let i = 0;

  while (i < lines.length) {
    const line = lines[i] ?? "";

    // Fenced code block
    if (/^```/.test(line.trim())) {
      const codeLines: string[] = [];
      i += 1;
      while (i < lines.length && !/^```/.test((lines[i] ?? "").trim())) {
        codeLines.push(lines[i] ?? "");
        i += 1;
      }
      if (i < lines.length) i += 1;
      const pre = document.createElement("pre");
      pre.className = "md-pre";
      const code = document.createElement("code");
      code.textContent = codeLines.join("\n");
      pre.append(code);
      frag.append(pre);
      continue;
    }

    // Blank line
    if (!line.trim()) {
      i += 1;
      continue;
    }

    // Horizontal rule
    if (/^\s*(?:---+|___+|\*\*\*+)\s*$/.test(line)) {
      frag.append(document.createElement("hr"));
      i += 1;
      continue;
    }

    // Heading (#..####)
    const hm = /^(#{1,4})\s+(.+)$/.exec(line);
    if (hm && hm[1] && hm[2]) {
      const level = Math.min(4, hm[1].length) as 1 | 2 | 3 | 4;
      const hEl = document.createElement(`h${level}`);
      hEl.append(renderInline(hm[2]));
      frag.append(hEl);
      i += 1;
      continue;
    }

    // Blockquote
    if (/^\s*>\s?/.test(line)) {
      const qLines: string[] = [];
      while (i < lines.length && /^\s*>\s?/.test(lines[i] ?? "")) {
        qLines.push((lines[i] ?? "").replace(/^\s*>\s?/, ""));
        i += 1;
      }
      const bq = document.createElement("blockquote");
      bq.append(renderInline(qLines.join(" ")));
      frag.append(bq);
      continue;
    }

    // Unordered / Task list
    if (/^\s*[-*+]\s+/.test(line)) {
      const ul = document.createElement("ul");
      while (i < lines.length && /^\s*[-*+]\s+/.test(lines[i] ?? "")) {
        const rawItem = (lines[i] ?? "").replace(/^\s*[-*+]\s+/, "");
        const li = document.createElement("li");
        const taskMatch = /^\[([ xX])\]\s+(.*)$/.exec(rawItem);
        if (taskMatch && taskMatch[1] !== undefined && taskMatch[2] !== undefined) {
          const cb = document.createElement("input");
          cb.type = "checkbox";
          cb.disabled = true;
          cb.checked = taskMatch[1].toLowerCase() === "x";
          li.className = "md-task";
          li.append(cb, " ", renderInline(taskMatch[2]));
        } else {
          li.append(renderInline(rawItem));
        }
        ul.append(li);
        i += 1;
      }
      frag.append(ul);
      continue;
    }

    // Ordered list
    if (/^\s*\d+\.\s+/.test(line)) {
      const ol = document.createElement("ol");
      while (i < lines.length && /^\s*\d+\.\s+/.test(lines[i] ?? "")) {
        const rawItem = (lines[i] ?? "").replace(/^\s*\d+\.\s+/, "");
        const li = document.createElement("li");
        li.append(renderInline(rawItem));
        ol.append(li);
        i += 1;
      }
      frag.append(ol);
      continue;
    }

    // Paragraph (join contiguous plain lines)
    const pLines: string[] = [];
    while (
      i < lines.length &&
      (lines[i] ?? "").trim() !== "" &&
      !/^(#{1,4}\s+|```|\s*[-*+]\s+|\s*\d+\.\s+|\s*>\s?|\s*(?:---+|___+|\*\*\*+)\s*$)/.test(
        lines[i] ?? ""
      )
    ) {
      pLines.push((lines[i] ?? "").trim());
      i += 1;
    }
    const p = document.createElement("p");
    p.append(renderInline(pLines.join(" ")));
    frag.append(p);
  }

  return frag;
}

const INLINE_TOK = /(`[^`]+`|\*\*[^*]+\*\*|\[[^\]]+\]\([^)]+\))/g;

function renderInline(text: string): DocumentFragment {
  const frag = document.createDocumentFragment();
  let lastIdx = 0;
  for (const m of text.matchAll(INLINE_TOK)) {
    const at = m.index ?? 0;
    const tok = m[0];
    if (at > lastIdx) frag.append(text.slice(lastIdx, at));
    if (tok.startsWith("`") && tok.endsWith("`")) {
      const code = document.createElement("code");
      code.textContent = tok.slice(1, -1);
      frag.append(code);
    } else if (tok.startsWith("**") && tok.endsWith("**")) {
      const strong = document.createElement("strong");
      strong.textContent = tok.slice(2, -2);
      frag.append(strong);
    } else if (tok.startsWith("[")) {
      const linkMatch = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(tok);
      if (linkMatch && linkMatch[1] !== undefined && linkMatch[2] !== undefined) {
        const href = linkMatch[2].trim();
        if (/^https?:\/\//i.test(href)) {
          const a = document.createElement("a");
          a.href = href;
          a.target = "_blank";
          a.rel = "noopener noreferrer";
          a.textContent = linkMatch[1];
          frag.append(a);
        } else {
          frag.append(linkMatch[1]);
        }
      } else {
        frag.append(tok);
      }
    }
    lastIdx = at + tok.length;
  }
  if (lastIdx < text.length) frag.append(text.slice(lastIdx));
  return frag;
}

export function isMarkdownFileName(name: string): boolean {
  return /\.(?:md|markdown|mdx)$/i.test(name.trim());
}
