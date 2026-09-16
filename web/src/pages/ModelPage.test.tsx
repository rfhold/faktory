import { create } from "@bufbuild/protobuf";
import { type JSX, createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  ListViewsResponseSchema,
  ModelGeometryFactsSchema,
  ModelOutputSummarySchema,
  ModelSchema,
  OutputRole,
  Vector3Schema,
} from "../../../proto/gen/ts/faktory/v1/faktory_pb";

const mocks = vi.hoisted(() => ({
  model: undefined as unknown,
}));

vi.mock("@solidjs/router", () => ({
  A: (props: JSX.AnchorHTMLAttributes<HTMLAnchorElement>) => <a {...props} />,
  useParams: () => ({ id: "model/a" }),
}));

vi.mock("@tanstack/solid-query", () => ({
  createQuery: (factory: () => { queryKey: readonly unknown[] }) => {
    const key = factory().queryKey;
    if (key.includes("views")) {
      return { data: create(ListViewsResponseSchema), isError: false, isPending: false, refetch: vi.fn() };
    }
    return {
      get data() { return (mocks.model as () => unknown)(); },
      isError: false,
      isPending: false,
      refetch: vi.fn(),
    };
  },
  createMutation: () => ({ isPending: false, mutate: vi.fn() }),
  useQueryClient: () => ({ invalidateQueries: vi.fn() }),
}));

vi.mock("../viewer/ModelViewer", () => ({
  ModelViewer: (props: { url: string }) => <div data-testid="viewer" data-url={props.url}>Model viewer</div>,
}));

import { ModelPage } from "./ModelPage";

const disposers: Array<() => void> = [];

afterEach(() => {
  disposers.splice(0).forEach((dispose) => dispose());
  document.body.replaceChildren();
});

function output(outputId: string, role: OutputRole, primary: boolean, volume: number) {
  return create(ModelOutputSummarySchema, {
    outputId,
    role,
    primary,
    facts: create(ModelGeometryFactsSchema, {
      volumeCubicMillimeters: volume,
      sizeMillimeters: create(Vector3Schema, { x: volume, y: 2, z: 3 }),
    }),
  });
}

describe("ModelPage outputs", () => {
  it("selects outputs in declared order and resets to primary for a replacement revision", () => {
    const [model, setModel] = createSignal(create(ModelSchema, {
      id: "model/a",
      name: "Multipart model",
      currentSuccessfulSourceRevision: "revision/1",
      currentSuccessfulOutputs: [
        output("part one", OutputRole.PART, false, 500),
        output("main assembly", OutputRole.ASSEMBLY, true, 2_000),
        output("fixture", OutputRole.TOOL, false, 3_000),
      ],
    }));
    mocks.model = model;
    const host = document.createElement("div");
    document.body.append(host);
    disposers.push(render(() => <ModelPage />, host));

    const options = Array.from(host.querySelectorAll<HTMLButtonElement>(".output-option"));
    expect(options.map((button) => button.textContent)).toEqual([
      "part onePart",
      "main assemblyAssembly / Primary",
      "fixtureTool",
    ]);
    expect(options[1].getAttribute("aria-pressed")).toBe("true");
    expect(host.querySelector('[data-testid="viewer"]')?.getAttribute("data-url")).toBe(
      "/artifacts/model%2Fa/revision%2F1/outputs/main%20assembly/model.glb",
    );
    expect(host.querySelector(".output-facts")?.textContent).toContain("Assembly");
    expect(host.querySelector(".output-facts")?.textContent).toContain("2 cm³");

    options[2].click();
    expect(options[2].getAttribute("aria-pressed")).toBe("true");
    expect(host.querySelector('[data-testid="viewer"]')?.getAttribute("data-url")).toContain(
      "/outputs/fixture/model.glb",
    );
    expect(host.querySelector(".output-facts")?.textContent).toContain("Tool");
    expect(host.querySelector(".output-facts")?.textContent).toContain("3 cm³");

    setModel(create(ModelSchema, {
      id: "model/a",
      name: "Multipart model",
      currentSuccessfulSourceRevision: "revision-2",
      currentSuccessfulOutputs: [
        output("fixture", OutputRole.TOOL, false, 4_000),
        output("replacement", OutputRole.PART, true, 5_000),
      ],
    }));

    expect(host.querySelector<HTMLButtonElement>('.output-option[aria-pressed="true"]')?.textContent)
      .toBe("replacementPart / Primary");
    expect(host.querySelector('[data-testid="viewer"]')?.getAttribute("data-url")).toContain(
      "/revision-2/outputs/replacement/model.glb",
    );
  });

  it("shows the no-success state without an output selector or viewer", () => {
    const [model] = createSignal(create(ModelSchema, { id: "model/a", name: "New model" }));
    mocks.model = model;
    const host = document.createElement("div");
    document.body.append(host);
    disposers.push(render(() => <ModelPage />, host));

    expect(host.querySelector(".output-panel")).toBeNull();
    expect(host.querySelector('[data-testid="viewer"]')).toBeNull();
    expect(host.textContent).toContain("No successful geometry");
  });
});
