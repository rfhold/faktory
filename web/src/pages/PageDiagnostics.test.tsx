import { create } from "@bufbuild/protobuf";
import { QueryClientProvider, QueryClient } from "@tanstack/solid-query";
import { type JSX } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  ListViewsResponseSchema,
  ModelSchema,
  NamedViewSchema,
} from "../../../proto/gen/ts/faktory/v1/faktory_pb";

const mocks = vi.hoisted(() => ({
  queryResults: [] as Array<Record<string, unknown>>,
  mutationOptions: [] as Array<{ onError: (error: Error) => void }>,
}));

vi.mock("@solidjs/router", () => ({
  A: (props: JSX.AnchorHTMLAttributes<HTMLAnchorElement>) => <a {...props} />,
  useParams: () => ({ id: "model-1" }),
}));

vi.mock("@tanstack/solid-query", async (importOriginal) => {
  const original = await importOriginal<typeof import("@tanstack/solid-query")>();
  return {
    ...original,
    createQuery: () => mocks.queryResults.shift(),
    createMutation: (factory: () => { onError: (error: Error) => void }) => {
      const options = factory();
      mocks.mutationOptions.push(options);
      return { isPending: false, mutate: vi.fn() };
    },
    useQueryClient: () => ({ invalidateQueries: vi.fn() }),
  };
});

vi.mock("../viewer/ModelViewer", () => ({
  ModelViewer: () => <div>Model viewer</div>,
}));

import { CatalogPage } from "./CatalogPage";
import { ModelPage } from "./ModelPage";

const disposers: Array<() => void> = [];

function mount(view: () => JSX.Element) {
  const host = document.createElement("div");
  document.body.append(host);
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  disposers.push(render(() => (
    <QueryClientProvider client={queryClient}>{view()}</QueryClientProvider>
  ), host));
  return host;
}

beforeEach(() => {
  mocks.queryResults.length = 0;
  mocks.mutationOptions.length = 0;
});

afterEach(() => {
  disposers.splice(0).forEach((dispose) => dispose());
  document.body.replaceChildren();
});

describe("page diagnostics", () => {
  it("keeps catalog query errors out of the rendered retry alert", () => {
    const sentinel = "SENTINEL-catalog-credential";
    mocks.queryResults.push({
      data: undefined,
      error: new Error(sentinel),
      isError: true,
      isPending: false,
      refetch: vi.fn(),
    });

    const host = mount(() => <CatalogPage />);
    const alert = host.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("Catalog unavailable.");
    expect(alert?.textContent).toContain("The model catalog could not be loaded.");
    expect(alert?.textContent).not.toContain(sentinel);
    expect(alert?.querySelector("button")?.textContent).toBe("Try again");
  });

  it("keeps model query errors out of the rendered retry alert", () => {
    const sentinel = "SENTINEL-model-token";
    mocks.queryResults.push(
      {
        data: undefined,
        error: new Error(sentinel),
        isError: true,
        isPending: false,
        refetch: vi.fn(),
      },
      { data: undefined, isError: false, isPending: true, refetch: vi.fn() },
    );

    const host = mount(() => <ModelPage />);
    const alert = host.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("Model unavailable.");
    expect(alert?.textContent).toContain("The model details could not be loaded.");
    expect(alert?.textContent).not.toContain(sentinel);
    expect(alert?.querySelector("button")?.textContent).toBe("Try again");
  });

  it("renders fixed operation-specific messages for named-view failures", () => {
    const sentinel = "SENTINEL-view-session";
    const model = create(ModelSchema, { id: "model-1", name: "Model one" });
    const view = create(NamedViewSchema, { id: "view-1", name: "Front" });
    mocks.queryResults.push(
      { data: model, error: undefined, isError: false, isPending: false, refetch: vi.fn() },
      {
        data: create(ListViewsResponseSchema, { views: [view] }),
        error: undefined,
        isError: false,
        isPending: false,
        refetch: vi.fn(),
      },
    );

    const host = mount(() => <ModelPage />);
    expect(mocks.mutationOptions).toHaveLength(3);
    const actions = Array.from(host.querySelectorAll("button"), (button) => button.textContent);
    expect(actions).toEqual(expect.arrayContaining(["Set default", "Delete", "Save"]));

    const expectedMessages = [
      "View could not be saved. Try again.",
      "View could not be deleted. Try again.",
      "Default view could not be changed. Try again.",
    ];
    expectedMessages.forEach((message, index) => {
      mocks.mutationOptions[index].onError(new Error(`${sentinel}-${index}`));
      const alert = host.querySelector('.inline-error[role="alert"]');
      expect(alert?.textContent).toBe(message);
      expect(host.textContent).not.toContain(sentinel);
    });
  });
});
