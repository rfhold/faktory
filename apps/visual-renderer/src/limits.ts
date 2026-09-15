import {
  MAX_GLB_BYTES, MAX_PNG_BYTES, MAX_RESPONSE_BYTES, MAX_SPEC_BYTES,
} from "./constants.js";
import { ServiceError } from "./errors.js";

export function validateGlbByteLength(length: number): void {
  if (!Number.isSafeInteger(length) || length < 0 || length > MAX_GLB_BYTES) {
    throw new ServiceError(413, "glb_too_large");
  }
}

export function validateSpecByteLength(length: number): void {
  if (!Number.isSafeInteger(length) || length < 0 || length > MAX_SPEC_BYTES) {
    throw new ServiceError(400, "invalid_render_spec");
  }
}

export function validatePngByteLength(length: number): void {
  if (!Number.isSafeInteger(length) || length < 0 || length > MAX_PNG_BYTES) {
    throw new ServiceError(500, "invalid_render_output");
  }
}

export function validateResponseByteLength(length: number): void {
  if (!Number.isSafeInteger(length) || length < 0 || length > MAX_RESPONSE_BYTES) {
    throw new ServiceError(500, "render_response_too_large");
  }
}
