import { A, useParams } from "@solidjs/router";
import { createMutation, createQuery, useQueryClient } from "@tanstack/solid-query";
import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import {
  Projection,
  type NamedView,
} from "../../../proto/gen/ts/faktory/v1/faktory_pb";
import { faktoryClient } from "../api/client";
import { modelKeys, setViewsDefault, upsertModel } from "../api/queries";
import { StatusBadge } from "../components/StatusBadge";
import { artifactUrl, modelAvailability } from "../model";
import { ModelViewer, type CameraSnapshot } from "../viewer/ModelViewer";
import {
  initializeDefaultView,
  skipDefaultViewInitialization,
  type DefaultViewInitialization,
} from "./defaultView";

function cameraFromView(view: NamedView): CameraSnapshot {
  return {
    target: view.target ?? { x: 0, y: 0, z: 0 },
    rotation: view.rotation ?? { x: 0, y: 0, z: 0, w: 1 },
    projection: view.projection,
    distance: view.distance,
    fieldOfViewDegrees: view.fieldOfViewDegrees,
    orthographicScale: view.orthographicScale,
  };
}

export function ModelPage() {
  const params = useParams<{ id: string }>();
  const modelId = () => params.id;
  const queryClient = useQueryClient();
  const [camera, setCamera] = createSignal<CameraSnapshot>();
  const [appliedView, setAppliedView] = createSignal<CameraSnapshot>();
  const [editingId, setEditingId] = createSignal<string>();
  const [viewName, setViewName] = createSignal("");
  const [viewerState, setViewerState] = createSignal<"loading" | "ready" | "error">("loading");
  const [loadedGeometryUrl, setLoadedGeometryUrl] = createSignal<string>();
  const [actionError, setActionError] = createSignal("");
  let defaultViewInitialization: DefaultViewInitialization | undefined;
  let activeModelId = modelId();

  const model = createQuery(() => ({
    queryKey: modelKeys.detail(modelId()),
    queryFn: async () => {
      const response = await faktoryClient.getModel({ modelId: modelId() });
      if (!response.model) throw new Error("The server returned no model");
      return response.model;
    },
    staleTime: 30_000,
  }));
  const views = createQuery(() => ({
    queryKey: modelKeys.views(modelId()),
    queryFn: () => faktoryClient.listViews({ modelId: modelId() }),
    staleTime: 30_000,
  }));
  const geometryUrl = () => model.data ? artifactUrl(model.data) : undefined;
  const loadKey = () => `${modelId()}:${geometryUrl() ?? ""}`;
  const editingView = createMemo(() => views.data?.views.find((view) => view.id === editingId()));

  createEffect(() => {
    const nextModelId = modelId();
    if (nextModelId === activeModelId) return;
    activeModelId = nextModelId;
    setCamera(undefined);
    setAppliedView(undefined);
    setEditingId(undefined);
    setViewName("");
    setViewerState("loading");
    setLoadedGeometryUrl(undefined);
    setActionError("");
  });

  createEffect(() => {
    const url = geometryUrl();
    const result = initializeDefaultView(
      defaultViewInitialization,
      loadKey(),
      Boolean(url) && loadedGeometryUrl() === url,
      views.data?.defaultViewId ?? "",
      views.data?.views,
    );
    defaultViewInitialization = result.state;
    if (!result.view) return;

    const nextCamera = cameraFromView(result.view);
    setAppliedView({ ...nextCamera });
    setCamera(nextCamera);
  });

  const saveView = createMutation(() => ({
    mutationFn: async () => {
      const currentCamera = camera();
      if (!currentCamera) throw new Error("Move or apply the camera before saving a view");
      const existing = editingView();
      return faktoryClient.putView({
        modelId: modelId(),
        view: {
          id: existing?.id ?? "",
          name: viewName().trim(),
          target: currentCamera.target,
          rotation: currentCamera.rotation,
          projection: currentCamera.projection,
          distance: currentCamera.distance,
          fieldOfViewDegrees: currentCamera.fieldOfViewDegrees,
          orthographicScale: currentCamera.orthographicScale,
          etag: existing?.etag ?? "",
        },
        expectedEtag: existing?.etag,
      });
    },
    onSuccess: async (response) => {
      setActionError("");
      if (response.view) {
        setEditingId(response.view.id);
        setViewName(response.view.name);
      }
      await queryClient.invalidateQueries({ queryKey: modelKeys.views(modelId()) });
    },
    onError: () => setActionError("View could not be saved. Try again."),
  }));

  const deleteView = createMutation(() => ({
    mutationFn: (view: NamedView) =>
      faktoryClient.deleteView({ modelId: modelId(), viewId: view.id, expectedEtag: view.etag }),
    onSuccess: async () => {
      setActionError("");
      setEditingId(undefined);
      setViewName("");
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: modelKeys.views(modelId()) }),
        queryClient.invalidateQueries({ queryKey: modelKeys.detail(modelId()) }),
        queryClient.invalidateQueries({ queryKey: modelKeys.all, exact: true }),
      ]);
    },
    onError: () => setActionError("View could not be deleted. Try again."),
  }));

  const setDefault = createMutation(() => ({
    mutationFn: (viewId: string) => faktoryClient.setDefaultView({ modelId: modelId(), viewId }),
    onSuccess: (response) => {
      setActionError("");
      if (response.model) {
        upsertModel(queryClient, response.model);
        setViewsDefault(queryClient, modelId(), response.model.defaultViewId);
      }
    },
    onError: () => setActionError("Default view could not be changed. Try again."),
  }));

  const beginEditing = (view: NamedView) => {
    defaultViewInitialization = skipDefaultViewInitialization(loadKey());
    setEditingId(view.id);
    setViewName(view.name);
    const nextCamera = cameraFromView(view);
    setAppliedView({ ...nextCamera });
    setCamera(nextCamera);
  };

  const beginNew = () => {
    setEditingId(undefined);
    setViewName("");
    setActionError("");
  };

  return (
    <main class="page detail-page">
      <A class="back-link" href="/">&larr; Model catalog</A>
      <Show when={model.isPending}>
        <div class="panel state-panel">Loading model...</div>
      </Show>
      <Show when={model.isError}>
        <div class="panel state-panel error" role="alert">
          <strong>Model unavailable.</strong>
          <span>The model details could not be loaded.</span>
          <button onClick={() => void model.refetch()}>Try again</button>
        </div>
      </Show>

      <Show when={model.data}>
        {(record) => {
          return (
            <>
              <header class="detail-heading">
                <div>
                  <p class="eyebrow">Model / {record().id}</p>
                  <h1>{record().name || record().id}</h1>
                </div>
                <StatusBadge state={record().renderState} />
              </header>

              <div class="availability panel">
                <div>
                  <span class="availability-label">Render report</span>
                  <strong>{modelAvailability(record())}</strong>
                </div>
                <Show when={record().renderError}>
                  <p role="alert">{record().renderError}</p>
                </Show>
              </div>

              <div class="detail-grid">
                <section class="viewer-panel panel" aria-label="Model geometry">
                  <Show
                    when={artifactUrl(record())}
                    fallback={
                      <div class="viewer-placeholder">
                        <span class="empty-mark">GLB</span>
                        <h2>No successful geometry</h2>
                        <p>The viewer will become available after a render succeeds.</p>
                      </div>
                    }
                  >
                    {(url) => (
                      <>
                        <ModelViewer
                          url={url()}
                          appliedView={appliedView()}
                          onCameraChange={(nextCamera) => {
                            setCamera(nextCamera);
                            if (viewerState() === "ready") {
                              defaultViewInitialization = skipDefaultViewInitialization(loadKey());
                            }
                          }}
                          onLoadState={(state) => {
                            setViewerState(state);
                            setLoadedGeometryUrl(state === "ready" ? url() : undefined);
                          }}
                        />
                        <Show when={viewerState() === "loading"}>
                          <div class="viewer-message" aria-live="polite">Loading geometry...</div>
                        </Show>
                        <Show when={viewerState() === "error"}>
                          <div class="viewer-message error" role="alert">Geometry could not be loaded.</div>
                        </Show>
                      </>
                    )}
                  </Show>
                  <div class="viewer-caption">
                    <span>Orbit / drag</span><span>Pan / secondary drag</span><span>Zoom / scroll</span>
                  </div>
                </section>

                <aside class="views-panel panel">
                  <div class="section-heading">
                    <div>
                      <p class="eyebrow">Shared camera</p>
                      <h2>Named views</h2>
                    </div>
                    <button class="button-secondary" type="button" onClick={beginNew}>New</button>
                  </div>

                  <Show when={views.isPending}><p class="muted">Loading views...</p></Show>
                  <Show when={views.isError}>
                    <div class="inline-error" role="alert">Views unavailable. <button onClick={() => void views.refetch()}>Retry</button></div>
                  </Show>
                  <Show when={views.data?.views.length === 0}>
                    <p class="muted">No shared views have been saved.</p>
                  </Show>
                  <div class="view-list">
                    <For each={views.data?.views}>
                      {(view) => (
                        <article classList={{ "view-row": true, selected: editingId() === view.id }}>
                          <button class="view-apply" type="button" onClick={() => beginEditing(view)}>
                            <span>{view.name}</span>
                            <small>{view.projection === Projection.ORTHOGRAPHIC ? "Orthographic" : "Perspective"}</small>
                          </button>
                          <Show when={views.data?.defaultViewId === view.id}>
                            <span class="default-label">Default</span>
                          </Show>
                          <div class="view-actions">
                            <button type="button" title={`Set ${view.name} as default`} onClick={() => setDefault.mutate(view.id)}>Set default</button>
                            <button class="danger-link" type="button" title={`Delete ${view.name}`} onClick={() => deleteView.mutate(view)}>Delete</button>
                          </div>
                        </article>
                      )}
                    </For>
                  </div>

                  <form
                    class="view-editor"
                    onSubmit={(event) => {
                      event.preventDefault();
                      setActionError("");
                      saveView.mutate();
                    }}
                  >
                    <label for="view-name">{editingView() ? "Update selected view" : "Save current camera"}</label>
                    <div class="field-row">
                      <input
                        id="view-name"
                        value={viewName()}
                        onInput={(event) => setViewName(event.currentTarget.value)}
                        placeholder="View name"
                        required
                      />
                      <button type="submit" disabled={!camera() || saveView.isPending}>
                        {saveView.isPending ? "Saving..." : editingView() ? "Update" : "Save"}
                      </button>
                    </div>
                    <div class="projection-toggle" role="group" aria-label="Camera projection">
                      <button
                        type="button"
                        classList={{ active: camera()?.projection !== Projection.ORTHOGRAPHIC }}
                        disabled={!camera()}
                        onClick={() => {
                          const current = camera();
                          if (!current) return;
                          const next = { ...current, projection: Projection.PERSPECTIVE };
                          setCamera(next);
                          setAppliedView(next);
                        }}
                      >Perspective</button>
                      <button
                        type="button"
                        classList={{ active: camera()?.projection === Projection.ORTHOGRAPHIC }}
                        disabled={!camera()}
                        onClick={() => {
                          const current = camera();
                          if (!current) return;
                          const next = { ...current, projection: Projection.ORTHOGRAPHIC };
                          setCamera(next);
                          setAppliedView(next);
                        }}
                      >Orthographic</button>
                    </div>
                    <p>Camera movement stays local until you save.</p>
                  </form>
                  <Show when={actionError()}>
                    <div class="inline-error" role="alert">{actionError()}</div>
                  </Show>
                </aside>
              </div>
            </>
          );
        }}
      </Show>
    </main>
  );
}
