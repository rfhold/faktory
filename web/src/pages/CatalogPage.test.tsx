import { create } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { createSignal, type JSX } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  ModelGeometryFactsSchema,
  ModelSchema,
  RenderState,
  Vector3Schema,
} from "../../../proto/gen/ts/faktory/v1/faktory_pb";

vi.mock("@solidjs/router", () => ({
  A: (props: JSX.AnchorHTMLAttributes<HTMLAnchorElement>) => <a {...props} />,
}));

import { CatalogModelRow } from "./CatalogPage";

const disposers: Array<() => void> = [];

afterEach(() => {
  disposers.splice(0).forEach((dispose) => dispose());
  document.body.replaceChildren();
});

describe("CatalogModelRow", () => {
  it("renders one linked row with lazy preview and semantic last-good facts", () => {
    const facts = create(ModelGeometryFactsSchema, {
      volumeCubicMillimeters: 12_500,
      sizeMillimeters: create(Vector3Schema, { x: 10, y: 20, z: 30 }),
    });
    const model = create(ModelSchema, {
      id: "bracket/a",
      name: "Bracket",
      renderState: RenderState.FAILED,
      currentSuccessfulSourceRevision: "last-good-revision",
      currentSuccessfulFacts: facts,
      updatedAt: timestampFromDate(new Date("2026-09-13T15:45:00.000Z")),
    });
    const host = document.createElement("div");
    document.body.append(host);
    disposers.push(render(() => <CatalogModelRow model={model} index={0} />, host));

    const link = host.querySelector("a");
    const image = host.querySelector("img");
    expect(link?.getAttribute("href")).toBe("/models/bracket%2Fa");
    expect(host.querySelectorAll("a")).toHaveLength(1);
    expect(image?.getAttribute("src")).toBe("/artifacts/bracket%2Fa/last-good-revision/preview.svg");
    expect(image?.getAttribute("alt")).toBe("Bracket model preview");
    expect(image?.getAttribute("loading")).toBe("lazy");
    expect(image?.getAttribute("decoding")).toBe("async");
    expect(host.querySelector("dl")?.textContent).toContain("12.5 cm³");
    expect(host.querySelector("dl")?.textContent).toContain("10 × 20 × 30 mm");
    expect(host.querySelector("time")?.getAttribute("datetime")).toBe("2026-09-13T15:45:00.000Z");
    expect(link?.textContent).toContain("showing the last successful revision");
    expect(link?.textContent).toContain("last-good-re");
    const status = host.querySelector(".status");
    expect(status?.getAttribute("role")).toBe("status");
    expect(status?.getAttribute("aria-live")).toBe("polite");
    expect(status?.getAttribute("aria-atomic")).toBe("true");
    expect(status?.textContent).toContain("Failed");
  });

  it("falls back quietly when no preview or facts are available", () => {
    const model = create(ModelSchema, { id: "new model", name: "New model" });
    const host = document.createElement("div");
    document.body.append(host);
    disposers.push(render(() => <CatalogModelRow model={model} index={1} />, host));

    expect(host.querySelector("img")).toBeNull();
    expect(host.querySelector(".model-preview-placeholder")?.getAttribute("aria-hidden")).toBeNull();
    expect(host.querySelector(".model-preview-placeholder")?.textContent).toBe("Preview unavailable");
    expect(host.querySelector("dl")?.textContent?.match(/—/g)).toHaveLength(3);
    expect(host.querySelector("time")).toBeNull();
  });

  it("replaces a failed image with the placeholder", () => {
    const model = create(ModelSchema, {
      id: "model",
      currentSuccessfulSourceRevision: "revision",
    });
    const host = document.createElement("div");
    document.body.append(host);
    disposers.push(render(() => <CatalogModelRow model={model} index={0} />, host));

    host.querySelector("img")?.dispatchEvent(new Event("error"));
    expect(host.querySelector("img")).toBeNull();
    const placeholder = host.querySelector(".model-preview-placeholder");
    expect(placeholder?.textContent).toBe("Preview unavailable");
    expect(placeholder?.getAttribute("aria-hidden")).toBeNull();
  });

  it("loads a new preview after a model revision changes", () => {
    const [model, setModel] = createSignal(create(ModelSchema, {
      id: "model",
      currentSuccessfulSourceRevision: "revision-1",
    }));
    const host = document.createElement("div");
    document.body.append(host);
    disposers.push(render(() => <CatalogModelRow model={model()} index={0} />, host));

    expect(host.querySelector("img")?.getAttribute("alt")).toBe("model model preview");
    host.querySelector("img")?.dispatchEvent(new Event("error"));
    expect(host.querySelector("img")).toBeNull();

    setModel(create(ModelSchema, {
      id: "model",
      currentSuccessfulSourceRevision: "revision-2",
    }));
    expect(host.querySelector("img")?.getAttribute("src")).toBe(
      "/artifacts/model/revision-2/preview.svg",
    );
  });

  it("reactively replaces status, facts, and updated timestamp", () => {
    const initialFacts = create(ModelGeometryFactsSchema, {
      volumeCubicMillimeters: 500,
      sizeMillimeters: create(Vector3Schema, { x: 1, y: 2, z: 3 }),
    });
    const replacementFacts = create(ModelGeometryFactsSchema, {
      volumeCubicMillimeters: 2_000,
      sizeMillimeters: create(Vector3Schema, { x: 4, y: 5, z: 6 }),
    });
    const [model, setModel] = createSignal(create(ModelSchema, {
      id: "model",
      renderState: RenderState.PENDING,
      currentSuccessfulSourceRevision: "revision-1",
      currentSuccessfulFacts: initialFacts,
      updatedAt: timestampFromDate(new Date("2026-09-13T15:45:00.000Z")),
    }));
    const host = document.createElement("div");
    document.body.append(host);
    disposers.push(render(() => <CatalogModelRow model={model()} index={0} />, host));

    expect(host.querySelector(".status")?.textContent).toContain("Pending");
    expect(host.querySelector("dl")?.textContent).toContain("500 mm³");
    expect(host.querySelector("dl")?.textContent).toContain("1 × 2 × 3 mm");

    setModel(create(ModelSchema, {
      id: "model",
      renderState: RenderState.READY,
      currentSuccessfulSourceRevision: "revision-2",
      currentSuccessfulFacts: replacementFacts,
      updatedAt: timestampFromDate(new Date("2026-09-14T16:30:00.000Z")),
    }));

    expect(host.querySelector(".status")?.textContent).toContain("Ready");
    expect(host.querySelector("dl")?.textContent).toContain("2 cm³");
    expect(host.querySelector("dl")?.textContent).toContain("4 × 5 × 6 mm");
    expect(host.querySelector("time")?.getAttribute("datetime")).toBe("2026-09-14T16:30:00.000Z");
  });

  it("keeps the full row natively keyboard-focusable", () => {
    const model = create(ModelSchema, { id: "keyboard/model", name: "Keyboard model" });
    const host = document.createElement("div");
    document.body.append(host);
    disposers.push(render(() => <CatalogModelRow model={model} index={0} />, host));

    const anchors = host.querySelectorAll("a");
    expect(anchors).toHaveLength(1);
    anchors[0].focus();
    expect(document.activeElement).toBe(anchors[0]);
    expect(anchors[0].getAttribute("href")).toBe("/models/keyboard%2Fmodel");
  });
});
