import { PNG } from "pngjs";
import { RENDER_HEIGHT, RENDER_WIDTH } from "./constants.js";
import { ServiceError } from "./errors.js";
import { validatePngByteLength } from "./limits.js";

export function validatePng(bytes: Buffer): void {
  validatePngByteLength(bytes.length);
  let png: PNG;
  try {
    png = PNG.sync.read(bytes, { checkCRC: true });
  } catch {
    throw new ServiceError(500, "invalid_render_output");
  }
  if (png.width !== RENDER_WIDTH || png.height !== RENDER_HEIGHT) {
    throw new ServiceError(500, "invalid_render_output");
  }
  for (let index = 3; index < png.data.length; index += 4) {
    if (png.data[index] !== 255) throw new ServiceError(500, "invalid_render_output");
  }
}
