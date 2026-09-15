import { PNG } from "pngjs";
import { describe, expect, it } from "vitest";
import { validateGlb } from "../src/glb.js";
import {
  MAX_GLB_BYTES, MAX_PNG_BYTES, MAX_RESPONSE_BYTES,
} from "../src/constants.js";
import {
  validateGlbByteLength, validatePngByteLength, validateResponseByteLength,
} from "../src/limits.js";
import { validatePng } from "../src/png.js";
import { coloredGlb } from "./fixture.js";

describe("bounded artifact validation", () => {
  it("accepts a complete GLB and rejects malformed framing", () => {
    expect(() => validateGlb(coloredGlb())).not.toThrow();
    expect(() => validateGlb(Buffer.from("glTF"))).toThrow("malformed_glb");
    const wrongLength = coloredGlb();
    wrongLength.writeUInt32LE(12, 8);
    expect(() => validateGlb(wrongLength)).toThrow("malformed_glb");
  });

  it("decodes exact opaque PNGs instead of trusting their headers", () => {
    const opaque = new PNG({ width: 640, height: 480, fill: true });
    opaque.data.fill(255);
    expect(() => validatePng(PNG.sync.write(opaque))).not.toThrow();
    opaque.data[3] = 254;
    expect(() => validatePng(PNG.sync.write(opaque))).toThrow("invalid_render_output");
    expect(() => validatePng(Buffer.from("not png"))).toThrow("invalid_render_output");
  });

  it("enforces exact and adjacent GLB, PNG, and response limits", () => {
    expect(() => validateGlbByteLength(MAX_GLB_BYTES)).not.toThrow();
    expect(() => validateGlbByteLength(MAX_GLB_BYTES + 1)).toThrow("glb_too_large");
    expect(() => validatePngByteLength(MAX_PNG_BYTES)).not.toThrow();
    expect(() => validatePngByteLength(MAX_PNG_BYTES + 1)).toThrow("invalid_render_output");
    expect(() => validateResponseByteLength(MAX_RESPONSE_BYTES)).not.toThrow();
    expect(() => validateResponseByteLength(MAX_RESPONSE_BYTES + 1))
      .toThrow("render_response_too_large");
  });
});
