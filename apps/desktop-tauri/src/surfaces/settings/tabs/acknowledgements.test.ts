import { describe, expect, it } from "vitest";
import generated from "../../../generated/third-party-notices.json?raw";
import { authorSummary, loadAcknowledgements, parseNotices, spdxIds, spdxUrl } from "./acknowledgements";

describe("acknowledgements", () => {
  it("resolves shared texts into each component's notices", () => {
    const list = parseNotices(
      JSON.stringify({
        schemaVersion: 1,
        components: [
          {
            name: "alpha",
            ecosystem: "cargo",
            versions: ["1.0.0"],
            authors: ["A"],
            url: "https://example.com/alpha",
            license: "MIT",
            notices: [{ file: "LICENSE", textId: "t1" }],
          },
          {
            name: "beta",
            ecosystem: "npm",
            versions: ["2.0.0"],
            authors: [],
            url: "https://example.com/beta",
            license: "MIT",
            notices: [{ file: "LICENSE", textId: "t1" }, { file: "NOTICE", textId: "missing" }],
          },
        ],
        texts: { t1: "MIT License text" },
      }),
    );
    expect(list.map((item) => item.notices)).toEqual([
      [{ file: "LICENSE", text: "MIT License text" }],
      [{ file: "LICENSE", text: "MIT License text" }],
    ]);
  });

  it("rejects an unknown schema", () => {
    expect(parseNotices(JSON.stringify({ schemaVersion: 2, components: [], texts: {} }))).toEqual([]);
  });

  it("names up to three authors and counts the rest after two", () => {
    expect(authorSummary([], "{} others")).toBe("");
    expect(authorSummary(["A", "B", "C"], "{} others")).toBe("A, B, C");
    expect(authorSummary(["A", "B", "C", "D", "E"], "{} others")).toBe("A, B, 3 others");
  });

  it("splits SPDX expressions into linkable ids", () => {
    expect(spdxIds("MIT OR Apache-2.0")).toEqual(["MIT", "Apache-2.0"]);
    expect(spdxIds("(MIT OR Apache-2.0) AND Unicode-3.0")).toEqual([
      "MIT",
      "Apache-2.0",
      "Unicode-3.0",
    ]);
    expect(spdxIds("MIT/Apache-2.0")).toEqual(["MIT", "Apache-2.0"]);
    expect(spdxUrl("GPL-2.0+")).toBe("https://spdx.org/licenses/GPL-2.0.html");
  });

  it("does not link free-text license fields", () => {
    expect(spdxIds("Unknown")).toEqual([]);
    expect(spdxIds("See license file")).toEqual([]);
    expect(spdxIds("NOASSERTION")).toEqual([]);
    expect(spdxIds("Custom Terms")).toEqual([]);
  });

  it("keeps the standard-text flag on notices", () => {
    const [item] = parseNotices(
      JSON.stringify({
        schemaVersion: 1,
        components: [
          {
            name: "gamma",
            ecosystem: "cargo",
            versions: ["1.0.0"],
            authors: [],
            url: "https://example.com/gamma",
            license: "MIT",
            notices: [{ file: "standard-MIT", textId: "t2", standard: "MIT" }],
          },
        ],
        texts: { t2: "Copyright (c) gamma contributors" },
      }),
    );
    expect(item.notices).toEqual([
      { file: "standard-MIT", text: "Copyright (c) gamma contributors", standard: "MIT" },
    ]);
  });

  it("parses the generated file once and shares the result between mounts", async () => {
    const first = loadAcknowledgements();
    expect(loadAcknowledgements()).toBe(first);
    expect((await first).length).toBeGreaterThan(100);
  });

  it("ships a well-formed, sorted generated notices file", () => {
    const raw = JSON.parse(generated) as {
      components: Array<{ name: string; notices: Array<{ textId: string }> }>;
      texts: Record<string, string>;
    };
    const list = parseNotices(generated);
    expect(list.length).toBeGreaterThan(100);
    for (const component of raw.components) {
      for (const notice of component.notices) {
        expect(raw.texts[notice.textId], component.name).toBeTypeOf("string");
      }
    }
    for (const item of list) {
      expect(item.notices.length > 0 || spdxIds(item.license).length > 0, item.name).toBe(true);
    }
    const names = list.map((item) => item.name.toLowerCase());
    expect(names).toEqual([...names].sort((a, b) => a.localeCompare(b, "en")));
    expect(names).toEqual(expect.arrayContaining(["tauri", "react", "codexcontrol"]));
  });
});
