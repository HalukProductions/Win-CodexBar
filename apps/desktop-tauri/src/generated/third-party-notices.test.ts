import { describe, expect, it } from "vitest";
import generated from "./third-party-notices.json?raw";
import {
  STANDARD_TEMPLATES,
  buildDocument,
  compareVersions,
  copyrightLine,
  finalizeComponents,
  licenseIds,
  loadTemplate,
  mergeComponent,
  normalizeText,
  renderStandardText,
  sortNotices,
  standardNotices,
  textIdOf,
} from "../../scripts/third-party-notices-lib.mjs";
import type { NoticeComponent } from "../../scripts/third-party-notices-lib.mjs";

function component(overrides: Partial<NoticeComponent> & { name: string }): NoticeComponent {
  return {
    ecosystem: "cargo",
    versions: ["1.0.0"],
    authors: [],
    url: `https://example.com/${overrides.name}`,
    license: "MIT",
    notices: [],
    ...overrides,
  };
}

describe("third-party notices generator", () => {
  it("normalizes line endings, BOM, and trailing blanks before interning", () => {
    expect(normalizeText("\uFEFFa \r\nb\t\r\n\r\n")).toBe("a\nb");
    expect(textIdOf(normalizeText("a \r\nb"))).toBe(textIdOf("a\nb"));
  });

  it("splits license expressions into unique ids", () => {
    expect(licenseIds("MIT OR Apache-2.0")).toEqual(["MIT", "Apache-2.0"]);
    expect(licenseIds("(MIT OR Apache-2.0) AND MIT")).toEqual(["MIT", "Apache-2.0"]);
    expect(licenseIds("Apache-2.0 WITH LLVM-exception")).toEqual(["Apache-2.0", "LLVM-exception"]);
    expect(licenseIds("See license file")).toEqual([]);
  });

  it("builds the copyright line from authors, or a generic holder when there are none", () => {
    expect(copyrightLine("x", ["A <a@example.com>", "B"])).toBe("Copyright (c) A <a@example.com>, B");
    expect(copyrightLine("webview2-com", [])).toBe("Copyright (c) webview2-com contributors");
  });

  it("uses the documented holder override ahead of authors metadata", () => {
    expect(copyrightLine("alloc-stdlib", ["Daniel Reiter Horn <danielrh@dropbox.com>"])).toBe(
      "Copyright (c) 2016 Dropbox, Inc.",
    );
    expect(copyrightLine("alloc-no-stdlib", ["Daniel Reiter Horn <danielrh@dropbox.com>"])).toBe(
      "Copyright (c) Daniel Reiter Horn <danielrh@dropbox.com>",
    );
  });

  it("keeps dollar signs in author names literal when rendering", () => {
    expect(renderStandardText("{{copyright}}\n", "Copyright (c) $& Co")).toBe(
      "Copyright (c) $& Co\n",
    );
  });

  it("selects a standard text for each SPDX id that has a template", () => {
    const templates: Record<string, string> = {
      MIT: "{{copyright}}\n\nPermission text",
    };
    const readTemplate = (id: string) => templates[id];
    expect(standardNotices("MIT OR Apache-2.0", "x", ["A"], readTemplate)).toEqual([
      { id: "MIT", text: "Copyright (c) A\n\nPermission text" },
    ]);
    expect(standardNotices("GPL-2.0+", "x", [], readTemplate)).toEqual([]);
    expect(standardNotices("See license file", "x", [], readTemplate)).toEqual([]);
  });

  it("never treats inherited object keys as templates", () => {
    expect(loadTemplate("constructor")).toBeUndefined();
    expect(loadTemplate("toString")).toBeUndefined();
    expect(standardNotices("constructor", "x", [])).toEqual([]);
  });

  it("ships a template file for every standard id, with the MIT and BSD placeholder", () => {
    for (const id of Object.keys(STANDARD_TEMPLATES)) {
      expect(loadTemplate(id), id).toBeTypeOf("string");
    }
    expect(loadTemplate("MIT")).toContain("{{copyright}}");
    expect(loadTemplate("BSD-3-Clause")).toContain("{{copyright}}");
    expect(loadTemplate("Apache-2.0")).not.toContain("{{copyright}}");
    const [mit] = standardNotices("MIT", "x", ["A"]);
    expect(mit.text.startsWith("Copyright (c) A\n\nPermission is hereby granted")).toBe(true);
  });

  it("merges duplicate components without repeating versions, authors, or notices", () => {
    const map = new Map<string, NoticeComponent>();
    const notice = { file: "LICENSE", textId: "t1" };
    mergeComponent(map, "cargo:a", component({ name: "a", authors: ["A"], notices: [notice] }));
    mergeComponent(
      map,
      "cargo:a",
      component({ name: "a", versions: ["2.0.0", "1.0.0"], authors: ["A", "B"], notices: [notice] }),
    );
    const merged = map.get("cargo:a");
    expect(merged?.versions).toEqual(["1.0.0", "2.0.0"]);
    expect(merged?.authors).toEqual(["A", "B"]);
    expect(merged?.notices).toEqual([notice]);
  });

  it("sorts versions numerically and notices by file then text id", () => {
    expect(["1.10.0", "1.2.0", "1.9.9"].sort(compareVersions)).toEqual(["1.2.0", "1.9.9", "1.10.0"]);
    expect(
      sortNotices([
        { file: "NOTICE", textId: "b" },
        { file: "LICENSE", textId: "z" },
        { file: "LICENSE", textId: "a" },
      ]),
    ).toEqual([
      { file: "LICENSE", textId: "a" },
      { file: "LICENSE", textId: "z" },
      { file: "NOTICE", textId: "b" },
    ]);
  });

  it("sorts components case-insensitively by name, then ecosystem, stably", () => {
    const sorted = finalizeComponents([
      component({ name: "Beta", ecosystem: "npm" }),
      component({ name: "alpha", ecosystem: "npm" }),
      component({ name: "alpha", ecosystem: "cargo" }),
    ]).map((c) => `${c.name}:${c.ecosystem}`);
    expect(sorted).toEqual(["alpha:cargo", "alpha:npm", "Beta:npm"]);
  });

  it("writes only the texts that a component references, with stable output", () => {
    const texts = new Map([
      ["b", "second"],
      ["a", "first"],
      ["unused", "not written"],
    ]);
    const list = finalizeComponents([
      component({ name: "one", notices: [{ file: "LICENSE", textId: "a" }] }),
    ]);
    const output = buildDocument(list, texts);
    expect(output.endsWith("\n")).toBe(true);
    expect(JSON.parse(output)).toEqual({
      schemaVersion: 1,
      components: [
        {
          name: "one",
          ecosystem: "cargo",
          versions: ["1.0.0"],
          authors: [],
          url: "https://example.com/one",
          license: "MIT",
          notices: [{ file: "LICENSE", textId: "a" }],
        },
      ],
      texts: { a: "first" },
    });
    expect(buildDocument(list, texts)).toBe(output);
  });

  it("gives identical standard texts the same text id across components", () => {
    const first = standardNotices("MIT", "one", ["A"])[0].text;
    const second = standardNotices("MIT/Apache-2.0", "two", ["A"])[0].text;
    expect(textIdOf(normalizeText(first))).toBe(textIdOf(normalizeText(second)));
  });

  it("ships a generated file where every component carries license text", () => {
    const raw = JSON.parse(generated) as {
      components: Array<{ name: string; notices: Array<{ textId: string; standard?: string }> }>;
      texts: Record<string, string>;
    };
    expect(raw.components.length).toBeGreaterThan(100);
    for (const item of raw.components) {
      expect(item.notices.length, item.name).toBeGreaterThan(0);
      for (const notice of item.notices) {
        expect(raw.texts[notice.textId], item.name).toBeTypeOf("string");
        if (notice.standard !== undefined) {
          expect(Object.keys(STANDARD_TEMPLATES), item.name).toContain(notice.standard);
        }
      }
    }
  });
});
