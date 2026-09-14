import { A } from "@solidjs/router";
import { createQuery } from "@tanstack/solid-query";
import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import type { Model } from "../../../proto/gen/ts/faktory/v1/faktory_pb";
import { faktoryClient } from "../api/client";
import { modelKeys } from "../api/queries";
import { StatusBadge } from "../components/StatusBadge";
import {
  formatDimensions,
  formatTimestamp,
  formatVolume,
  modelAvailability,
  previewUrl,
  timestampDateTime,
} from "../model";

export function CatalogModelRow(props: { model: Model; index: number }) {
  const [previewFailed, setPreviewFailed] = createSignal(false);
  const preview = createMemo(() => previewUrl(props.model));
  const updatedDateTime = createMemo(() => timestampDateTime(props.model.updatedAt));

  createEffect(() => {
    preview();
    setPreviewFailed(false);
  });

  return (
    <A class="model-card" href={`/models/${encodeURIComponent(props.model.id)}`}>
      <div class="model-card-index">{String(props.index + 1).padStart(2, "0")}</div>
      <div class="model-preview">
        <Show
          when={preview() && !previewFailed() ? preview() : undefined}
          fallback={<span class="model-preview-placeholder">Preview unavailable</span>}
        >
          {(source) => (
            <img
              src={source()}
              alt={`${props.model.name || props.model.id} model preview`}
              loading="lazy"
              decoding="async"
              onError={() => setPreviewFailed(true)}
            />
          )}
        </Show>
      </div>
      <div class="model-card-body">
        <div class="card-title-row">
          <h2>{props.model.name || props.model.id}</h2>
          <StatusBadge state={props.model.renderState} />
        </div>
        <p>{modelAvailability(props.model)}</p>
        <div class="revision">
          <span>Artifact</span>
          <code>{props.model.currentSuccessfulSourceRevision.slice(0, 12) || "none"}</code>
        </div>
      </div>
      <dl class="model-facts">
        <div class="model-fact">
          <dt>Volume</dt>
          <dd>{formatVolume(props.model)}</dd>
        </div>
        <div class="model-fact">
          <dt>Size</dt>
          <dd>{formatDimensions(props.model)}</dd>
        </div>
        <div class="model-fact">
          <dt>Updated</dt>
          <dd>
            <Show when={updatedDateTime()} fallback={formatTimestamp(props.model.updatedAt)}>
              {(dateTime) => <time datetime={dateTime()}>{formatTimestamp(props.model.updatedAt)}</time>}
            </Show>
          </dd>
        </div>
      </dl>
      <span class="card-arrow" aria-hidden="true">&#8599;</span>
    </A>
  );
}

export function CatalogPage() {
  const models = createQuery(() => ({
    queryKey: modelKeys.all,
    queryFn: async () => (await faktoryClient.listModels({})).models,
    staleTime: 30_000,
  }));

  return (
    <main class="page catalog-page">
      <header class="page-heading">
        <div>
          <p class="eyebrow">Model registry / live</p>
          <h1>FAKTORY</h1>
        </div>
        <p class="heading-note">Forge Any Kind of Thing; Our Resources Yield.</p>
      </header>

      <Show when={models.isPending}>
        <div class="panel state-panel" aria-live="polite">Loading model catalog...</div>
      </Show>
      <Show when={models.isError}>
        <div class="panel state-panel error" role="alert">
          <strong>Catalog unavailable.</strong>
          <span>The model catalog could not be loaded.</span>
          <button onClick={() => void models.refetch()}>Try again</button>
        </div>
      </Show>
      <Show when={models.data?.length === 0}>
        <div class="panel empty-state">
          <span class="empty-mark">00</span>
          <h2>No models yet</h2>
          <p>Models created through the Faktory MCP workflow will appear here.</p>
        </div>
      </Show>

      <div class="model-grid">
        <For each={models.data}>
          {(model: Model, index) => (
            <CatalogModelRow model={model} index={index()} />
          )}
        </For>
      </div>
    </main>
  );
}
