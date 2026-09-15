import { Buffer } from "node:buffer";
import { MAX_SPEC_BYTES, RECIPE } from "./constants.js";
import { ServiceError } from "./errors.js";
import { validateSpecByteLength } from "./limits.js";

export interface CanonicalRenderSpec {
  kind: "canonical";
  recipe: typeof RECIPE;
}

export interface ViewCameraSpec {
  target: [number, number, number];
  rotation: [number, number, number, number];
  projection: "perspective" | "orthographic";
  distance: number;
  field_of_view_degrees: number;
  orthographic_scale: number;
}

export interface ViewRenderSpec {
  kind: "view";
  recipe: typeof RECIPE;
  camera: ViewCameraSpec;
}

export type RenderSpec = CanonicalRenderSpec | ViewRenderSpec;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function exactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const actual = Object.keys(value).sort();
  return actual.length === keys.length && actual.every((key, index) => key === [...keys].sort()[index]);
}

function validNumber(value: unknown, positive = false): value is number {
  return typeof value === "number"
    && Number.isFinite(value)
    && Math.abs(value) <= 1_000_000_000
    && (!positive || value > 0);
}

function tuple(value: unknown, length: number): value is number[] {
  return Array.isArray(value) && value.length === length && value.every((item) => validNumber(item));
}

function parseCamera(value: unknown): ViewCameraSpec {
  if (!isRecord(value) || !exactKeys(value, [
    "target", "rotation", "projection", "distance", "field_of_view_degrees", "orthographic_scale",
  ])) throw new ServiceError(400, "invalid_render_spec");
  if (!tuple(value.target, 3) || !tuple(value.rotation, 4)) {
    throw new ServiceError(400, "invalid_render_spec");
  }
  const quaternionLength = Math.hypot(...value.rotation);
  if (quaternionLength < 1e-12 || quaternionLength > 1_000_000) {
    throw new ServiceError(400, "invalid_render_spec");
  }
  if (value.projection !== "perspective" && value.projection !== "orthographic") {
    throw new ServiceError(400, "invalid_render_spec");
  }
  if (!validNumber(value.distance, true)
    || !validNumber(value.field_of_view_degrees, true)
    || value.field_of_view_degrees >= 179
    || !validNumber(value.orthographic_scale, true)) {
    throw new ServiceError(400, "invalid_render_spec");
  }
  return {
    target: value.target as [number, number, number],
    rotation: value.rotation as [number, number, number, number],
    projection: value.projection,
    distance: value.distance,
    field_of_view_degrees: value.field_of_view_degrees,
    orthographic_scale: value.orthographic_scale,
  };
}

export function parseRenderSpec(encoded: string | undefined): RenderSpec {
  if (!encoded || encoded.length > Math.ceil(MAX_SPEC_BYTES * 4 / 3) || !/^[A-Za-z0-9_-]+$/.test(encoded)) {
    throw new ServiceError(400, "invalid_render_spec");
  }
  const bytes = Buffer.from(encoded, "base64url");
  try {
    validateSpecByteLength(bytes.length);
  } catch {
    throw new ServiceError(400, "invalid_render_spec");
  }
  if (bytes.toString("base64url") !== encoded) {
    throw new ServiceError(400, "invalid_render_spec");
  }
  let value: unknown;
  try {
    value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch {
    throw new ServiceError(400, "invalid_render_spec");
  }
  if (!isRecord(value) || value.recipe !== RECIPE) {
    throw new ServiceError(400, "unsupported_render_spec");
  }
  if (value.kind === "canonical" && exactKeys(value, ["kind", "recipe"])) {
    return { kind: "canonical", recipe: RECIPE };
  }
  if (value.kind === "view" && exactKeys(value, ["kind", "recipe", "camera"])) {
    return { kind: "view", recipe: RECIPE, camera: parseCamera(value.camera) };
  }
  throw new ServiceError(400, "unsupported_render_spec");
}
