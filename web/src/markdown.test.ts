import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { isMarkdownFileName, renderMarkdownToFragment } from "./markdown";

describe("markdown", () => {
  let prevDocument: unknown;

  beforeEach(() => {
    prevDocument = (globalThis as any).document;
    const mk = (tag: string) => {
      const el: any = {
        tagName: tag.toUpperCase(),
        children: [] as any[],
        textContent: "",
        href: "",
        type: "",
        disabled: false,
        checked: false,
        className: "",
        append(...nodes: any[]) {
          for (const n of nodes) {
            if (typeof n === "string") el.children.push({ text: n });
            else el.children.push(n);
          }
        },
      };
      return el;
    };
    (globalThis as any).document = {
      createDocumentFragment: () => mk("fragment"),
      createElement: (tag: string) => mk(tag),
    };
  });

  afterEach(() => {
    (globalThis as any).document = prevDocument;
  });

  test("identifies markdown filenames cleanly", () => {
    expect(isMarkdownFileName("README.md")).toBe(true);
    expect(isMarkdownFileName("implementation_plan.markdown")).toBe(true);
    expect(isMarkdownFileName("Cargo.toml")).toBe(false);
  });

  test("renders headings, task lists, code blocks, and strips unsafe javascript: links", () => {
    const md = [
      "# Title",
      "",
      "- [x] Done with **bold** and `code`",
      "- [ ] [Evil](javascript:alert(1)) vs [Safe](https://example.com)",
      "",
      "```rs",
      "fn main() {}",
      "```",
    ].join("\n");

    const frag = renderMarkdownToFragment(md) as any;
    expect(frag.children[0].tagName).toBe("H1");
    expect(frag.children[1].tagName).toBe("UL");
    expect(frag.children[2].tagName).toBe("PRE");

    // Evil javascript: link must NOT create an <a> element
    const secondItemChildren = frag.children[1].children[1].children;
    const anchors = JSON.stringify(secondItemChildren);
    expect(anchors).not.toContain("javascript:");
    expect(anchors).toContain("https://example.com");
  });
});
