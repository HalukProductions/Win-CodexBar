import { describe, expect, it } from "vitest";
import { buildAccountOrdinals } from "./claudeAccountDisplay";

describe("buildAccountOrdinals", () => {
  it("numbers accounts in list order, not by sorted id", () => {
    expect(buildAccountOrdinals([{ id: "z" }, { id: "a" }, { id: "m" }])).toEqual({
      z: 1,
      a: 2,
      m: 3,
    });
  });

  it("keeps the first position of a repeated id", () => {
    expect(buildAccountOrdinals([{ id: "b" }, { id: "a" }, { id: "b" }])).toEqual({ b: 1, a: 2 });
  });
});
