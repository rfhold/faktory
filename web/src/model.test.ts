import { create } from "@bufbuild/protobuf";
import { timestampFromDate, TimestampSchema } from "@bufbuild/protobuf/wkt";
import { describe, expect, it } from "vitest";
import {
  ModelGeometryFactsSchema,
  ModelOutputSummarySchema,
  ModelSchema,
  OutputRole,
  RenderState,
  Vector3Schema,
} from "../../proto/gen/ts/faktory/v1/faktory_pb";
import {
  artifactUrl,
  formatDimensions,
  formatTimestamp,
  formatVolume,
  modelAvailability,
  outputArtifactUrl,
  outputRoleLabel,
  primaryOutput,
  previewUrl,
  resolveOutput,
  timestampDateTime,
} from "./model";

function model(overrides: Parameters<typeof create<typeof ModelSchema>>[1] = {}) {
  return create(ModelSchema, { id: "bracket/a", name: "Bracket", ...overrides });
}

describe("artifactUrl", () => {
  it("uses an immutable, path-safe successful revision URL", () => {
    expect(artifactUrl(model({ currentSuccessfulSourceRevision: "abc123" }))).toBe(
      "/artifacts/bracket%2Fa/abc123/model.glb",
    );
  });

  it("does not produce a URL without a successful revision", () => {
    expect(artifactUrl(model())).toBeUndefined();
  });
});

describe("outputArtifactUrl", () => {
  it("encodes model, revision, and output identity as path segments", () => {
    expect(outputArtifactUrl(
      model({ id: "bracket/a", currentSuccessfulSourceRevision: "rev/#1" }),
      "fixture / clamp",
    )).toBe("/artifacts/bracket%2Fa/rev%2F%231/outputs/fixture%20%2F%20clamp/model.glb");
  });

  it("does not produce a URL without a successful revision or output identity", () => {
    expect(outputArtifactUrl(model(), "part")).toBeUndefined();
    expect(outputArtifactUrl(model({ currentSuccessfulSourceRevision: "rev" }), "")).toBeUndefined();
  });
});

describe("output selection", () => {
  const output = (outputId: string, role: OutputRole, primary = false) =>
    create(ModelOutputSummarySchema, { outputId, role, primary });
  const outputs = [
    output("bracket", OutputRole.PART),
    output("assembly", OutputRole.ASSEMBLY, true),
    output("fixture", OutputRole.TOOL),
  ];

  it("defaults to the declared primary and preserves explicit selection", () => {
    expect(primaryOutput(outputs)?.outputId).toBe("assembly");
    expect(resolveOutput(outputs)?.outputId).toBe("assembly");
    expect(resolveOutput(outputs, "fixture")?.outputId).toBe("fixture");
    expect(resolveOutput(outputs, "removed")?.outputId).toBe("assembly");
  });

  it("labels generated output roles without numeric assumptions", () => {
    expect(outputRoleLabel(OutputRole.ASSEMBLY)).toBe("Assembly");
    expect(outputRoleLabel(OutputRole.PART)).toBe("Part");
    expect(outputRoleLabel(OutputRole.TOOL)).toBe("Tool");
    expect(outputRoleLabel(OutputRole.UNSPECIFIED)).toBe("Output");
  });
});

describe("previewUrl", () => {
  it("encodes both immutable path segments", () => {
    expect(previewUrl(model({ id: "bracket/a b", currentSuccessfulSourceRevision: "rev/#1" }))).toBe(
      "/artifacts/bracket%2Fa%20b/rev%2F%231/preview.svg",
    );
  });

  it("does not produce a URL without a successful revision", () => {
    expect(previewUrl(model())).toBeUndefined();
  });
});

describe("geometry fact formatting", () => {
  const facts = (volumeCubicMillimeters: number, size?: { x: number; y: number; z: number }) =>
    create(ModelGeometryFactsSchema, {
      volumeCubicMillimeters,
      sizeMillimeters: size ? create(Vector3Schema, size) : undefined,
    });

  it("selects compact volume units at their thresholds", () => {
    expect(formatVolume(model({ currentSuccessfulFacts: facts(999) }))).toBe("999 mm³");
    expect(formatVolume(model({ currentSuccessfulFacts: facts(1_000) }))).toBe("1 cm³");
    expect(formatVolume(model({ currentSuccessfulFacts: facts(1_000_000_000) }))).toBe("1 m³");
  });

  it("formats axis-aligned dimensions", () => {
    expect(
      formatDimensions(model({ currentSuccessfulFacts: facts(1, { x: 12.5, y: 20, z: 3.25 }) })),
    ).toBe("12.5 × 20 × 3.25 mm");
  });

  it("uses an em dash for missing or invalid values", () => {
    expect(formatVolume(model())).toBe("—");
    expect(formatVolume(model({ currentSuccessfulFacts: facts(-1) }))).toBe("—");
    expect(formatDimensions(model({ currentSuccessfulFacts: facts(1) }))).toBe("—");
    expect(
      formatDimensions(model({ currentSuccessfulFacts: facts(1, { x: Number.NaN, y: 2, z: 3 }) })),
    ).toBe("—");
  });
});

describe("timestamp formatting", () => {
  it("provides an absolute local label and machine-readable datetime", () => {
    const timestamp = timestampFromDate(new Date("2026-09-13T15:45:00.000Z"));
    expect(timestampDateTime(timestamp)).toBe("2026-09-13T15:45:00.000Z");
    expect(formatTimestamp(timestamp)).not.toBe("—");
  });

  it("uses an em dash for absent or invalid timestamps", () => {
    const invalid = create(TimestampSchema, { seconds: 253_402_300_800n });
    expect(formatTimestamp()).toBe("—");
    expect(timestampDateTime(invalid)).toBeUndefined();
    expect(formatTimestamp(invalid)).toBe("—");
  });
});

describe("modelAvailability", () => {
  it("reports failed replacement renders while preserving last-good geometry", () => {
    expect(
      modelAvailability(
        model({ renderState: RenderState.FAILED, currentSuccessfulSourceRevision: "last-good" }),
      ),
    ).toContain("showing the last successful revision");
  });

  it("distinguishes a first render in progress", () => {
    expect(modelAvailability(model({ renderState: RenderState.RENDERING }))).toBe(
      "Geometry is rendering.",
    );
  });
});
