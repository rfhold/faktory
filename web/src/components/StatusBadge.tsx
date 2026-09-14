import type { JSX } from "solid-js";
import { renderStateLabel } from "../model";
import type { RenderState } from "../../../proto/gen/ts/faktory/v1/faktory_pb";

export function StatusBadge(props: { state: RenderState }): JSX.Element {
  return (
    <span
      class={`status status-${renderStateLabel(props.state).toLowerCase()}`}
      role="status"
      aria-live="polite"
      aria-atomic="true"
    >
      <span class="status-dot" aria-hidden="true" />
      {renderStateLabel(props.state)}
    </span>
  );
}
