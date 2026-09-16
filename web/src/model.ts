import {
  OutputRole,
  RenderState,
  type Model,
  type ModelGeometryFacts,
  type ModelOutputSummary,
} from "../../proto/gen/ts/faktory/v1/faktory_pb";
import { timestampDate, type Timestamp } from "@bufbuild/protobuf/wkt";

const missingValue = "—";
const conciseNumber = new Intl.NumberFormat(undefined, { maximumSignificantDigits: 4 });
const localDateTime = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

export function renderStateLabel(state: RenderState) {
  switch (state) {
    case RenderState.PENDING:
      return "Pending";
    case RenderState.RENDERING:
      return "Rendering";
    case RenderState.READY:
      return "Ready";
    case RenderState.FAILED:
      return "Failed";
    default:
      return "Unknown";
  }
}

export function artifactUrl(model: Pick<Model, "id" | "currentSuccessfulSourceRevision">) {
  if (!model.currentSuccessfulSourceRevision) return undefined;
  return `/artifacts/${encodeURIComponent(model.id)}/${encodeURIComponent(model.currentSuccessfulSourceRevision)}/model.glb`;
}

export function outputArtifactUrl(
  model: Pick<Model, "id" | "currentSuccessfulSourceRevision">,
  outputId: string,
) {
  if (!model.currentSuccessfulSourceRevision || !outputId) return undefined;
  return `/artifacts/${encodeURIComponent(model.id)}/${encodeURIComponent(model.currentSuccessfulSourceRevision)}/outputs/${encodeURIComponent(outputId)}/model.glb`;
}

export function previewUrl(model: Pick<Model, "id" | "currentSuccessfulSourceRevision">) {
  if (!model.currentSuccessfulSourceRevision) return undefined;
  return `/artifacts/${encodeURIComponent(model.id)}/${encodeURIComponent(model.currentSuccessfulSourceRevision)}/preview.svg`;
}

export function formatFactsDimensions(facts?: ModelGeometryFacts) {
  const size = facts?.sizeMillimeters;
  if (!size || [size.x, size.y, size.z].some((value) => !Number.isFinite(value) || value < 0)) {
    return missingValue;
  }
  return `${conciseNumber.format(size.x)} × ${conciseNumber.format(size.y)} × ${conciseNumber.format(size.z)} mm`;
}

export function formatDimensions(model: Pick<Model, "currentSuccessfulFacts">) {
  return formatFactsDimensions(model.currentSuccessfulFacts);
}

export function formatFactsVolume(facts?: ModelGeometryFacts) {
  const volume = facts?.volumeCubicMillimeters;
  if (volume === undefined || !Number.isFinite(volume) || volume < 0) return missingValue;
  if (volume >= 1_000_000_000) return `${conciseNumber.format(volume / 1_000_000_000)} m³`;
  if (volume >= 1_000) return `${conciseNumber.format(volume / 1_000)} cm³`;
  return `${conciseNumber.format(volume)} mm³`;
}

export function formatVolume(model: Pick<Model, "currentSuccessfulFacts">) {
  return formatFactsVolume(model.currentSuccessfulFacts);
}

export function outputRoleLabel(role: OutputRole) {
  switch (role) {
    case OutputRole.ASSEMBLY:
      return "Assembly";
    case OutputRole.PART:
      return "Part";
    case OutputRole.TOOL:
      return "Tool";
    default:
      return "Output";
  }
}

export function primaryOutput(outputs: readonly ModelOutputSummary[]) {
  return outputs.find((output) => output.primary) ?? outputs[0];
}

export function resolveOutput(
  outputs: readonly ModelOutputSummary[],
  selectedOutputId?: string,
) {
  return outputs.find((output) => output.outputId === selectedOutputId) ?? primaryOutput(outputs);
}

export function timestampDateTime(timestamp?: Timestamp) {
  if (!timestamp) return undefined;
  if (
    timestamp.seconds < -62_135_596_800n ||
    timestamp.seconds > 253_402_300_799n ||
    timestamp.nanos < 0 ||
    timestamp.nanos > 999_999_999
  ) {
    return undefined;
  }
  try {
    const date = timestampDate(timestamp);
    return Number.isFinite(date.getTime()) ? date.toISOString() : undefined;
  } catch {
    return undefined;
  }
}

export function formatTimestamp(timestamp?: Timestamp) {
  const dateTime = timestampDateTime(timestamp);
  return dateTime ? localDateTime.format(new Date(dateTime)) : missingValue;
}

export function modelAvailability(model: Model) {
  const hasArtifact = Boolean(model.currentSuccessfulSourceRevision);
  if (model.renderState === RenderState.FAILED) {
    return hasArtifact ? "Latest render failed; showing the last successful revision." : "Render failed; no geometry is available.";
  }
  if (model.renderState === RenderState.PENDING) {
    return hasArtifact ? "Replacement queued; showing the last successful revision." : "First render is queued.";
  }
  if (model.renderState === RenderState.RENDERING) {
    return hasArtifact ? "Replacement rendering; showing the last successful revision." : "Geometry is rendering.";
  }
  if (!hasArtifact) return "No successful artifact is available.";
  return "Current successful revision.";
}
