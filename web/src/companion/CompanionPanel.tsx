import { createQuery } from "@tanstack/solid-query";
import { Show, createEffect, createSignal, onCleanup } from "solid-js";
import { fetchCompanionOrigin, fetchCompanion, mountCompanion } from "./protocol";
export function CompanionPanel(props: {
  modelId: string;
  revision: string;
  stale?: boolean;
}) {
  const config = createQuery(() => ({ queryKey: ["runtime-config"], queryFn: ({ signal }) => fetchCompanionOrigin(signal), retry: false, staleTime: Infinity }));
  const origin = () => config.data;
  const query = createQuery(() => ({ queryKey: ["companion", props.modelId, props.revision], enabled: Boolean(origin() && props.revision), queryFn: ({ signal }) => fetchCompanion(props.modelId, props.revision, signal), retry: false, staleTime: Infinity }));
  const [state, setState] = createSignal<"loading" | "ready" | "error">("loading");
  let connection: ReturnType<typeof mountCompanion> | undefined;
  const reset = () => { connection?.destroy(); connection = undefined; setState("loading"); };
  createEffect(() => { props.revision; props.modelId; reset(); });
  onCleanup(reset);
  return <section class="panel companion-panel" aria-labelledby="companion-heading">
    <div class="section-heading"><div><p class="eyebrow">Revision companion</p><h2 id="companion-heading">Guide &amp; reference</h2></div><button type="button" class="button-secondary" disabled={state() !== "ready"} onClick={() => connection?.print()}>Print guide</button></div>
    <Show when={props.stale && props.revision}><p class="muted" role="status">This guide uses the last successful revision. Newer model changes are not included.</p></Show>
    <Show when={origin()} fallback={<p class="muted">{config.isPending ? "Loading companion configuration..." : "Companion viewer is not configured."}</p>}>
      <Show when={props.revision} fallback={<p class="muted">A successful revision is required.</p>}>
        <Show when={query.isPending}><p class="muted">Loading companion...</p></Show>
        <Show when={query.isError || state() === "error"}><p class="inline-error" role="alert">Companion unavailable. The model remains available.</p></Show>
        <Show when={query.data === null}><p class="muted">No companion for this revision.</p></Show>
        <Show when={query.data} keyed>{companion => <iframe title="Model companion" class="companion-frame" src={`${origin()}/frame`} sandbox="allow-scripts allow-modals" referrerPolicy="no-referrer" credentialless onLoad={event => { reset(); connection = mountCompanion(event.currentTarget, companion, setState); }} />}</Show>
      </Show>
    </Show>
  </section>;
}
