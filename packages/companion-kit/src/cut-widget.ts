import { boardLayouts, sheetLayouts, layoutSvg, type BoardInput, type CutInput } from "./cuts";
import { sanitize } from "./index";
export function renderCutWidget(document: Document, value: unknown, maxNodes = 16384): DocumentFragment {
  if (!value || typeof value !== "object")
    throw new Error("Invalid cut asset");
  const request = value as {
    kind: string;
    input: CutInput | BoardInput;
  };
  if (!["board", "sheet"].includes(request.kind) || !request.input)
    throw new Error("Invalid cut asset");
  const layouts = request.kind === "board" ? boardLayouts(request.input as BoardInput) : sheetLayouts(request.input as CutInput);
  const board = request.kind === "board";
  const selected = layouts[0];
  const estimatedNodes = 20 + 11 * layouts.length + selected.sheets.reduce(
    (total, sheet) => total + (board ? 17 : 23) + sheet.placements.length * (board ? 11 : 17), 0,
  );
  if (estimatedNodes > maxNodes) throw new CutRenderBudgetError();
  const root = document.createDocumentFragment();
  const heading = document.createElement("h3");
  heading.textContent = "Cut layout comparison / mm";
  root.append(heading);
  const summary = document.createElement("table");
  const caption = document.createElement("caption");
  caption.textContent = "Deterministic heuristics, not globally optimal. Waste includes kerf.";
  summary.append(caption);
  const row = (values: string[], header = false) => {
    const tr = document.createElement("tr");
    for (const text of values) {
      const cell = document.createElement(header ? "th" : "td");
      cell.textContent = text;
      if (header)
        cell.setAttribute("scope", "col");
      tr.append(cell);
    }
    return tr;
  };
  const thead = document.createElement("thead");
  thead.append(row(["Strategy", "Stock pieces", board ? "Stock length mm" : "Stock area mm^2", board ? "Waste mm" : "Waste mm^2", "Cut complexity"], true));
  summary.append(thead);
  const tbody = document.createElement("tbody");
  for (const l of layouts)
    tbody.append(row([l.algorithm, String(l.sheets.length), String(l.stockArea), String(l.waste), String(l.cutComplexity)]));
  summary.append(tbody);
  root.append(summary);
  for (let i = 0; i < selected.sheets.length; i++) {
    const sheet = selected.sheets[i];
    const figure = document.createElement("figure");
    figure.append(sanitize(document, layoutSvg(selected, i), true));
    if (board) {
      const svg = figure.querySelector("svg")!;
      svg.setAttribute("preserveAspectRatio", "none");
      svg.setAttribute("style", "width:100%;height:80px");
    }
    const label = document.createElement("figcaption");
    label.textContent = board ? `${sheet.stockId} / ${i + 1}: ${sheet.width} mm length (normalized strip, not cross-section)` : `${sheet.stockId} / ${i + 1}: ${sheet.width} x ${sheet.height} mm`;
    figure.append(label);
    root.append(figure);
    const table = document.createElement("table");
    const head = document.createElement("thead");
    head.append(row(board ? ["Part", "X mm", "Length mm"] : ["Part", "X mm", "Y mm", "Width mm", "Height mm", "Rotated"], true));
    table.append(head);
    const body = document.createElement("tbody");
    for (const p of sheet.placements)
      body.append(row(board ? [p.partId, String(p.x), String(p.width)] : [p.partId, String(p.x), String(p.y), String(p.width), String(p.height), String(p.rotated)]));
    table.append(body);
    root.append(table);
  }
  const notes = document.createElement("p");
  notes.textContent = selected.notes.join(" ");
  root.append(notes);
  return root;
}
export class CutRenderBudgetError extends Error {
  constructor() { super("Companion is too complex"); }
}
