import { describe, expect, it } from "vitest";
import { boardLayouts, sheetLayouts, validateLayout, layoutSvg, type CutInput } from "./cuts";
const input: CutInput = { units: "mm", kerf: 3, stock: [{ id: "sheet", width: 1200, height: 2400, count: 4, grain: "y" }], parts: [{ id: "top", width: 600, height: 900, count: 2, grain: "y", rotate: false }, { id: "shelf", width: 300, height: 500, count: 4, grain: "none", rotate: true }] };
describe("bounded deterministic cut tools", () => {
  it("compares layouts and validates containment/counts/kerf", () => {
    const a = sheetLayouts(input);
    expect(a).toEqual(sheetLayouts(input));
    for (const l of a) {
      validateLayout(input, l);
      expect(l.waste).toBeGreaterThanOrEqual(0);
      expect(layoutSvg(l)).toContain("<svg");
    }
  });
  it("fits explicit outside-point board lengths", () => {
    const [l] = boardLayouts({ units: "mm", kerf: 3, stock: [{ id: "board", length: 2400, count: 2 }], parts: [{ id: "leg", length: 700, count: 4 }] });
    expect(l.sheets).toHaveLength(2);
    expect(l.sheets.flatMap(s => s.placements)).toHaveLength(4);
  });
  it("rejects units, counts, nonfinite inputs and shortages", () => {
    expect(() => sheetLayouts({ ...input, units: "in" as "mm" })).toThrow();
    expect(() => sheetLayouts({ ...input, kerf: NaN })).toThrow();
    expect(() => sheetLayouts({ ...input, stock: [{ ...input.stock[0], count: 0 }] })).toThrow();
    expect(() => sheetLayouts({ ...input, stock: [{ ...input.stock[0], width: 1 }] })).toThrow();
  });
  it("requires typed stock/part IDs and explicit sheet-part rotation", () => {
    const invalid: unknown[] = [null, 123, { ...input, stock: [null] }, { ...input, parts: [null] },
      { ...input, stock: [{ ...input.stock[0], id: 123 }] },
      { ...input, parts: [{ ...input.parts[0], id: 123 }] },
      { ...input, parts: [{ id: "missing", width: 1, height: 1, count: 1, grain: "none" }] },
      { ...input, parts: [{ ...input.parts[0], rotate: "false" }] },
    ];
    for (const value of invalid) {
      try { sheetLayouts(value as CutInput); throw new Error("Unexpected valid input"); }
      catch (error) { expect(error).toBeInstanceOf(Error); expect(error).not.toBeInstanceOf(TypeError); expect((error as Error).message).toMatch(/^Invalid/); }
    }
    const board = { units: "mm" as const, kerf: 0, stock: [{ id: "s", length: 2, count: 1 }], parts: [{ id: "p", length: 1, count: 1 }] };
    for (const value of [null, { ...board, stock: [null] }, { ...board, parts: [null] }, { ...board, stock: [{ ...board.stock[0], id: 123 }] }, { ...board, parts: [{ ...board.parts[0], id: 123 }] }]) {
      expect(() => boardLayouts(value as unknown as Parameters<typeof boardLayouts>[0])).toThrow(/^Invalid/);
    }
    expect(sheetLayouts(input)).not.toHaveLength(0);
  });
  it("rejects overlapping output and grain-breaking rotation", () => {
    const l = structuredClone(sheetLayouts(input)[0]);
    const s = l.sheets.find(s => s.placements.length > 1)!;
    s.placements[1].x = s.placements[0].x;
    s.placements[1].y = s.placements[0].y;
    expect(() => validateLayout(input, l)).toThrow();
  });
  it("respects grain and disallowed rotation", () => {
    const small: CutInput = { units: "mm", kerf: 0, stock: [{ id: "s", width: 100, height: 50, count: 1, grain: "x" }], parts: [{ id: "p", width: 50, height: 100, count: 1, grain: "x", rotate: true }] };
    expect(() => sheetLayouts(small)).toThrow();
    small.parts[0].grain = "none";
    expect(sheetLayouts(small)[0].sheets[0].placements[0].rotated).toBe(true);
  });
});
