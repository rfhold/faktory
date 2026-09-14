import { create } from "@bufbuild/protobuf";
import { QueryClient } from "@tanstack/solid-query";
import { describe, expect, it } from "vitest";
import { ModelSchema } from "../../../proto/gen/ts/faktory/v1/faktory_pb";
import { modelKeys, replaceModels, upsertModel } from "./queries";

const makeModel = (id: string, name: string) => create(ModelSchema, { id, name });

describe("model watch cache updates", () => {
  it("replaces an initial collection and primes detail entries", () => {
    const client = new QueryClient();
    const models = [makeModel("one", "One"), makeModel("two", "Two")];
    replaceModels(client, models);
    expect(client.getQueryData(modelKeys.all)).toEqual(models);
    expect(client.getQueryData(modelKeys.detail("two"))).toEqual(models[1]);
  });

  it("upserts a complete model without dropping other cached records", () => {
    const client = new QueryClient();
    replaceModels(client, [makeModel("one", "Old"), makeModel("two", "Two")]);
    upsertModel(client, makeModel("one", "New"));
    expect(client.getQueryData<ReturnType<typeof makeModel>[]>(modelKeys.all)?.map((item) => item.name)).toEqual([
      "New",
      "Two",
    ]);
  });
});
