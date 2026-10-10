// All dimensions are explicit millimetres. Miter lengths must be supplied between outside points.
export type Grain = "none" | "x" | "y";
export type Stock = {
  id: string;
  width: number;
  height: number;
  count: number;
  grain: Grain;
};
export type Part = {
  id: string;
  width: number;
  height: number;
  count: number;
  grain: Grain;
  rotate: boolean;
};
export type CutInput = {
  units: "mm";
  kerf: number;
  stock: Stock[];
  parts: Part[];
};
export type Placement = {
  partId: string;
  x: number;
  y: number;
  width: number;
  height: number;
  rotated: boolean;
};
export type Sheet = {
  stockId: string;
  width: number;
  height: number;
  placements: Placement[];
  cuts: number;
};
export type Layout = {
  algorithm: string;
  units: "mm";
  kerf: number;
  sheets: Sheet[];
  waste: number;
  stockArea: number;
  cutComplexity: number;
  notes: string[];
};
type Rect = {
  x: number;
  y: number;
  width: number;
  height: number;
};
const positive = (n: number) => Number.isFinite(n) && n > 0 && n <= 1e6;
function validateInput(input: CutInput) {
  if (!input || typeof input !== "object" || input.units !== "mm" || !Number.isFinite(input.kerf) || input.kerf < 0 || input.kerf > 1000 || !Array.isArray(input.stock) || !Array.isArray(input.parts) || !input.stock.length || !input.parts.length || input.stock.length > 64 || input.parts.length > 128)
    throw new Error("Invalid cut input; use explicit mm units");
  for (const list of [input.stock, input.parts]) {
    const ids = new Set<string>();
    let total = 0;
    for (const p of list) {
      if (!p || typeof p !== "object" || typeof p.id !== "string" || !/^[a-zA-Z0-9_-]{1,64}$/.test(p.id) || ids.has(p.id) || !positive(p.width) || !positive(p.height) || !Number.isInteger(p.count) || p.count < 1 || p.count > 512 || !["none", "x", "y"].includes(p.grain))
        throw new Error("Invalid stock or part");
      if (list === input.parts && typeof (p as Part).rotate !== "boolean")
        throw new Error("Invalid rotation");
      ids.add(p.id);
      total += p.count;
    }
    if (total > 512)
      throw new Error("Too many stock or part instances");
  }
}
function grainFits(part: Part, stock: Stock, rotated: boolean) {
  const grain = rotated ? part.grain === "x" ? "y" : part.grain === "y" ? "x" : "none" : part.grain;
  return grain === "none" || stock.grain === grain;
}
export function validateLayout(input: CutInput, layout: Layout): void {
  validateInput(input);
  if (layout.units !== "mm" || layout.kerf !== input.kerf || !Array.isArray(layout.sheets) || layout.sheets.length > 512)
    throw new Error("Invalid layout");
  const stockCounts = new Map<string, number>();
  const counts = new Map<string, number>();
  for (const sheet of layout.sheets) {
    const stock = input.stock.find(s => s.id === sheet.stockId);
    if (!stock || sheet.width !== stock.width || sheet.height !== stock.height || !Array.isArray(sheet.placements) || sheet.placements.length > 512 || !Number.isInteger(sheet.cuts) || sheet.cuts < 0 || sheet.cuts > sheet.placements.length * 2)
      throw new Error("Invalid stock output");
    stockCounts.set(stock.id, (stockCounts.get(stock.id) ?? 0) + 1);
    if (stockCounts.get(stock.id)! > stock.count)
      throw new Error("Stock exceeded");
    for (const p of sheet.placements) {
      const part = input.parts.find(part => part.id === p.partId);
      if (!part || typeof p.rotated !== "boolean" || (p.rotated && !part.rotate) || !grainFits(part, stock, p.rotated) || p.width !== (p.rotated ? part.height : part.width) || p.height !== (p.rotated ? part.width : part.height) || !Number.isFinite(p.x) || !Number.isFinite(p.y) || p.x < 0 || p.y < 0 || p.x + p.width > sheet.width + 1e-9 || p.y + p.height > sheet.height + 1e-9)
        throw new Error("Invalid placement");
      counts.set(part.id, (counts.get(part.id) ?? 0) + 1);
    }
    const k = input.kerf;
    for (let i = 0; i < sheet.placements.length; i++)
      for (let j = i + 1; j < sheet.placements.length; j++) {
        const a = sheet.placements[i], b = sheet.placements[j];
        if (!(a.x + a.width + k <= b.x + 1e-9 || b.x + b.width + k <= a.x + 1e-9 || a.y + a.height + k <= b.y + 1e-9 || b.y + b.height + k <= a.y + 1e-9))
          throw new Error("Overlapping placements or missing kerf");
      }
  }
  for (const part of input.parts)
    if (counts.get(part.id) !== part.count)
      throw new Error("Part count mismatch");
  const stockArea = layout.sheets.reduce((sum, s) => sum + s.width * s.height, 0);
  const usedArea = input.parts.reduce((sum, p) => sum + p.width * p.height * p.count, 0);
  const complexity = layout.sheets.reduce((sum, s) => sum + s.cuts, 0);
  if (layout.stockArea !== stockArea || Math.abs(layout.waste - (stockArea - usedArea)) > 1e-6 || !Number.isFinite(layout.waste) || layout.cutComplexity !== complexity)
    throw new Error("Invalid layout metrics");
}
function guillotine(input: CutInput, horizontal: boolean): Layout {
  validateInput(input);
  const byId = (a: {
    id: string;
  }, b: {
    id: string;
  }) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  const available = input.stock.flatMap(stock => Array.from({ length: stock.count }, () => stock)).sort((a, b) => a.width * a.height - b.width * b.height || byId(a, b));
  const parts = input.parts.flatMap(part => Array.from({ length: part.count }, () => part)).sort((a, b) => b.width * b.height - a.width * a.height || byId(a, b));
  const bins: {
    stock: Stock;
    sheet: Sheet;
    free: Rect[];
  }[] = [];
  for (const part of parts) {
    let choice: {
      bin: typeof bins[number];
      index: number;
      rotated: boolean;
      width: number;
      height: number;
    } | undefined;
    const find = (bin: typeof bins[number]) => {
      for (let index = 0; index < bin.free.length; index++)
        for (const rotated of part.rotate ? [false, true] : [false]) {
          const width = rotated ? part.height : part.width, height = rotated ? part.width : part.height;
          const r = bin.free[index];
          if (width <= r.width && height <= r.height && grainFits(part, bin.stock, rotated))
            return { bin, index, rotated, width, height };
        }
    };
    for (const bin of bins) {
      choice = find(bin);
      if (choice)
        break;
    }
    if (!choice) {
      for (let i = 0; i < available.length; i++) {
        const stock = available[i];
        const bin = { stock, sheet: { stockId: stock.id, width: stock.width, height: stock.height, placements: [] as Placement[], cuts: 0 }, free: [{ x: 0, y: 0, width: stock.width, height: stock.height }] };
        choice = find(bin);
        if (choice) {
          bins.push(bin);
          available.splice(i, 1);
          break;
        }
      }
    }
    if (!choice)
      throw new Error("Insufficient stock for this bounded heuristic");
    const { bin, index, rotated, width, height } = choice;
    const r = bin.free.splice(index, 1)[0], k = input.kerf;
    bin.sheet.placements.push({ partId: part.id, x: r.x, y: r.y, width, height, rotated });
    const dw = r.width - width - k, dh = r.height - height - k;
    const remainder = horizontal
      ? [{ x: r.x + width + k, y: r.y, width: dw, height }, { x: r.x, y: r.y + height + k, width: r.width, height: dh }]
      : [{ x: r.x + width + k, y: r.y, width: dw, height: r.height }, { x: r.x, y: r.y + height + k, width, height: dh }];
    bin.free.push(...remainder.filter(r => r.width > 0 && r.height > 0));
    bin.sheet.cuts += Number(width < r.width) + Number(height < r.height);
  }
  const sheets = bins.map(b => b.sheet);
  const stockArea = sheets.reduce((sum, s) => sum + s.width * s.height, 0);
  const used = input.parts.reduce((sum, p) => sum + p.width * p.height * p.count, 0);
  const layout: Layout = { algorithm: horizontal ? "guillotine-horizontal" : "guillotine-vertical", units: "mm", kerf: input.kerf, sheets, stockArea, waste: stockArea - used, cutComplexity: sheets.reduce((sum, s) => sum + s.cuts, 0), notes: ["Deterministic heuristic, not globally optimal.", "Waste includes kerf. Confirm tool safety and cut order independently.", "Miter lengths use outside points; no cut list is inferred from geometry bounds."] };
  validateLayout(input, layout);
  return layout;
}
export function sheetLayouts(input: CutInput): Layout[] {
  validateInput(input);
  const layouts: Layout[] = [];
  for (const direction of [false, true]) {
    try {
      layouts.push(guillotine(input, direction));
    }
    catch (error) {
      if (!(error instanceof Error) || !error.message.startsWith("Insufficient stock"))
        throw error;
    }
  }
  if (!layouts.length)
    throw new Error("Insufficient stock for bounded heuristics");
  return layouts.sort((a, b) => a.sheets.length - b.sheets.length || a.waste - b.waste || a.cutComplexity - b.cutComplexity);
}
export type BoardInput = {
  units: "mm";
  kerf: number;
  stock: {
    id: string;
    length: number;
    count: number;
  }[];
  parts: {
    id: string;
    length: number;
    count: number;
  }[];
};
export function boardLayouts(input: BoardInput): Layout[] {
  if (!input || typeof input !== "object" || !Array.isArray(input.stock) || !Array.isArray(input.parts) || input.stock.length > 64 || input.parts.length > 128 || input.stock.some(s => !s || typeof s !== "object") || input.parts.some(p => !p || typeof p !== "object"))
    throw new Error("Invalid cut input; use explicit mm units");
  return sheetLayouts({ units: input.units, kerf: input.kerf, stock: input.stock.map(s => ({ id: s.id, width: s.length, height: 1, count: s.count, grain: "x" })), parts: input.parts.map(p => ({ id: p.id, width: p.length, height: 1, count: p.count, grain: "x", rotate: false })) }).map(l => ({ ...l, algorithm: "board-first-fit-decreasing", notes: [...l.notes, "1D board lengths in mm; cross-section and material compatibility are caller inputs."] })).slice(0, 1);
}
export function layoutSvg(layout: Layout, sheetIndex = 0): string {
  const s = layout.sheets[sheetIndex];
  if (!s || !positive(s.width) || !positive(s.height) || s.placements.length > 512)
    throw new Error("Invalid SVG layout");
  const rects = s.placements.map(p => {
    if (![p.x, p.y, p.width, p.height].every(Number.isFinite) || !/^[a-zA-Z0-9_-]{1,64}$/.test(p.partId))
      throw new Error("Invalid SVG placement");
    return `<g><rect x="${p.x}" y="${p.y}" width="${p.width}" height="${p.height}" fill="#d9e9df" stroke="#42695c" stroke-width="1"/><title>${p.partId}: ${p.width} x ${p.height} mm</title></g>`;
  }).join("");
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${s.width} ${s.height}" role="img"><title>${layout.algorithm.replace(/[^a-z-]/g, "")} in mm</title><rect width="${s.width}" height="${s.height}" fill="#f4efe2"/>${rects}</svg>`;
}
