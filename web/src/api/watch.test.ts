import { create } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { QueryClient } from "@tanstack/solid-query";
import { describe, expect, it, vi } from "vitest";
import {
  ModelGeometryFactsSchema,
  ModelSchema,
  RenderState,
  Vector3Schema,
  WatchModelsResponseSchema,
} from "../../../proto/gen/ts/faktory/v1/faktory_pb";
import type { FaktoryClient } from "./client";
import { modelKeys } from "./queries";
import { consumeModelWatch } from "./watch";

describe("consumeModelWatch", () => {
  it("keeps the snapshot cached when the server stream fails", async () => {
    const model = create(ModelSchema, { id: "one", name: "One" });
    const client = {
      async *watchModels() {
        yield create(WatchModelsResponseSchema, {
          event: { case: "initialSnapshot", value: { models: [model] } },
        });
        throw new Error("connection lost");
      },
    } as unknown as FaktoryClient;
    const queryClient = new QueryClient();
    const controller = new AbortController();

    const watch = consumeModelWatch(client, queryClient, controller.signal);
    await vi.waitFor(() => expect(queryClient.getQueryData(modelKeys.all)).toEqual([model]));
    controller.abort();
    await watch;

    expect(queryClient.getQueryData(modelKeys.all)).toEqual([model]);
  });

  it("preserves enriched records through snapshot replacement and changed-event upsert", async () => {
    const initial = create(ModelSchema, {
      id: "one",
      name: "One",
      desiredSourceRevision: "desired-1",
      currentSuccessfulSourceRevision: "successful-1",
      renderState: RenderState.PENDING,
      renderError: "",
      defaultViewId: "front",
      currentSuccessfulFacts: create(ModelGeometryFactsSchema, {
        volumeCubicMillimeters: 1_500,
        sizeMillimeters: create(Vector3Schema, { x: 1, y: 2, z: 3 }),
      }),
      updatedAt: timestampFromDate(new Date("2026-09-13T15:45:00.000Z")),
    });
    const changed = create(ModelSchema, {
      id: "one",
      name: "One renamed",
      desiredSourceRevision: "desired-2",
      currentSuccessfulSourceRevision: "successful-2",
      renderState: RenderState.READY,
      renderError: "",
      defaultViewId: "isometric",
      currentSuccessfulFacts: create(ModelGeometryFactsSchema, {
        volumeCubicMillimeters: 2_500,
        sizeMillimeters: create(Vector3Schema, { x: 4, y: 5, z: 6 }),
      }),
      updatedAt: timestampFromDate(new Date("2026-09-14T16:30:00.000Z")),
    });
    const queryClient = new QueryClient();
    let snapshotCatalog: unknown;
    let snapshotDetail: unknown;
    const client = {
      async *watchModels() {
        yield create(WatchModelsResponseSchema, {
          event: { case: "initialSnapshot", value: { models: [initial] } },
        });
        snapshotCatalog = queryClient.getQueryData(modelKeys.all);
        snapshotDetail = queryClient.getQueryData(modelKeys.detail(initial.id));
        yield create(WatchModelsResponseSchema, {
          event: { case: "modelChanged", value: { model: changed } },
        });
        throw new Error("connection lost");
      },
    } as unknown as FaktoryClient;
    const controller = new AbortController();

    const watch = consumeModelWatch(client, queryClient, controller.signal);
    await vi.waitFor(() => expect(queryClient.getQueryData(modelKeys.all)).toEqual([changed]));
    expect(snapshotCatalog).toEqual([initial]);
    expect(snapshotDetail).toEqual(initial);
    expect(queryClient.getQueryData(modelKeys.detail(changed.id))).toEqual(changed);
    controller.abort();
    await watch;
  });

  it("replaces stale catalog state with the authoritative snapshot after reconnect", async () => {
    vi.useFakeTimers();
    const stale = create(ModelSchema, { id: "stale", name: "Stale" });
    const replacement = create(ModelSchema, { id: "replacement", name: "Replacement" });
    let connections = 0;
    const client = {
      async *watchModels(_request: unknown, options: { signal: AbortSignal }) {
        connections += 1;
        if (connections === 1) {
          yield create(WatchModelsResponseSchema, {
            event: { case: "initialSnapshot", value: { models: [stale] } },
          });
          throw new Error("connection lost");
        }

        yield create(WatchModelsResponseSchema, {
          event: { case: "initialSnapshot", value: { models: [replacement] } },
        });
        await new Promise<void>((resolve) => {
          options.signal.addEventListener("abort", () => resolve(), { once: true });
        });
      },
    } as unknown as FaktoryClient;
    const queryClient = new QueryClient();
    const controller = new AbortController();
    const watch = consumeModelWatch(client, queryClient, controller.signal);

    try {
      await vi.waitFor(() => expect(queryClient.getQueryData(modelKeys.all)).toEqual([stale]));
      await vi.advanceTimersByTimeAsync(1_000);
      await vi.waitFor(() => expect(queryClient.getQueryData(modelKeys.all)).toEqual([replacement]));

      const catalog = queryClient.getQueryData<Array<{ id: string }>>(modelKeys.all);
      expect(catalog?.some((model) => model.id === stale.id)).toBe(false);
      expect(catalog?.some((model) => model.id === replacement.id)).toBe(true);
    } finally {
      controller.abort();
      await watch;
      queryClient.clear();
      const remainingTimers = vi.getTimerCount();
      vi.useRealTimers();
      expect(remainingTimers).toBe(0);
    }
  });
});
