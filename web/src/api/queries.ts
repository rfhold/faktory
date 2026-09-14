import type { QueryClient } from "@tanstack/solid-query";
import type {
  ListViewsResponse,
  Model,
} from "../../../proto/gen/ts/faktory/v1/faktory_pb";

export const modelKeys = {
  all: ["models"] as const,
  detail: (id: string) => ["models", id] as const,
  views: (id: string) => ["models", id, "views"] as const,
};

export function replaceModels(queryClient: QueryClient, models: Model[]) {
  queryClient.setQueryData(modelKeys.all, models);
  for (const model of models) {
    queryClient.setQueryData(modelKeys.detail(model.id), model);
  }
}

export function upsertModel(queryClient: QueryClient, model: Model) {
  queryClient.setQueryData<Model[]>(modelKeys.all, (current) => {
    if (!current) return [model];
    const index = current.findIndex((item) => item.id === model.id);
    if (index < 0) return [...current, model];
    return current.map((item, itemIndex) => (itemIndex === index ? model : item));
  });
  queryClient.setQueryData(modelKeys.detail(model.id), model);
}

export function setViewsDefault(
  queryClient: QueryClient,
  modelId: string,
  defaultViewId: string,
) {
  queryClient.setQueryData<ListViewsResponse>(modelKeys.views(modelId), (current) =>
    current ? { ...current, defaultViewId } : current,
  );
}
