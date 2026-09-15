import { describe, expect, it } from "vitest";
import { ServiceError } from "../src/errors.js";
import { MAX_SPEC_BYTES } from "../src/constants.js";
import { parseRenderSpec } from "../src/spec.js";

function encode(value: unknown): string {
  return Buffer.from(JSON.stringify(value)).toString("base64url");
}

const camera = {
  target: [1, 2, 3], rotation: [0, 0, 0, 1], projection: "perspective",
  distance: 20, field_of_view_degrees: 42, orthographic_scale: 10,
};

describe("strict render spec", () => {
  it("accepts only the canonical and complete view shapes", () => {
    expect(parseRenderSpec(encode({ kind: "canonical", recipe: "three-v2" }))).toEqual({
      kind: "canonical", recipe: "three-v2",
    });
    expect(parseRenderSpec(encode({ kind: "view", recipe: "three-v2", camera }))).toMatchObject({
      kind: "view", camera,
    });
  });

  it.each([
    undefined,
    "e30=",
    encode({ kind: "canonical", recipe: "three-v1" }),
    encode({ kind: "canonical", recipe: "three-v2", extra: true }),
    encode({ kind: "view", recipe: "three-v2", camera: { ...camera, distance: 0 } }),
    encode({ kind: "view", recipe: "three-v2", camera: { ...camera, field_of_view_degrees: 179 } }),
    encode({ kind: "view", recipe: "three-v2", camera: { ...camera, rotation: [0, 0, 0, 0] } }),
    encode({ kind: "view", recipe: "three-v2", camera: { ...camera, unknown: 1 } }),
  ])("rejects malformed, unsupported, or invalid input", (encoded) => {
    expect(() => parseRenderSpec(encoded)).toThrow(ServiceError);
  });

  it("accepts the exact decoded header bound and rejects one adjacent byte", () => {
    const json = JSON.stringify({ kind: "canonical", recipe: "three-v2" });
    const exact = Buffer.from(json.padEnd(MAX_SPEC_BYTES, " ")).toString("base64url");
    const over = Buffer.from(json.padEnd(MAX_SPEC_BYTES + 1, " ")).toString("base64url");
    expect(parseRenderSpec(exact)).toEqual({ kind: "canonical", recipe: "three-v2" });
    expect(() => parseRenderSpec(over)).toThrow("invalid_render_spec");
  });
});
