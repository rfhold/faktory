import { RenderState, type Model } from "../../proto/gen/ts/faktory/v1/faktory_pb";
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

export function previewUrl(model: Pick<Model, "id" | "currentSuccessfulSourceRevision">) {
  if (!model.currentSuccessfulSourceRevision) return undefined;
  return `/artifacts/${encodeURIComponent(model.id)}/${encodeURIComponent(model.currentSuccessfulSourceRevision)}/preview.svg`;
}

export function formatDimensions(model: Pick<Model, "currentSuccessfulFacts">) {
  const size = model.currentSuccessfulFacts?.sizeMillimeters;
  if (!size || [size.x, size.y, size.z].some((value) => !Number.isFinite(value) || value < 0)) {
    return missingValue;
  }
  return `${conciseNumber.format(size.x)} × ${conciseNumber.format(size.y)} × ${conciseNumber.format(size.z)} mm`;
}

export function formatVolume(model: Pick<Model, "currentSuccessfulFacts">) {
  const volume = model.currentSuccessfulFacts?.volumeCubicMillimeters;
  if (volume === undefined || !Number.isFinite(volume) || volume < 0) return missingValue;
  if (volume >= 1_000_000_000) return `${conciseNumber.format(volume / 1_000_000_000)} m³`;
  if (volume >= 1_000) return `${conciseNumber.format(volume / 1_000)} cm³`;
  return `${conciseNumber.format(volume)} mm³`;
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
