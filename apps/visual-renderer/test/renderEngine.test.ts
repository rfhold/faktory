import { existsSync } from "node:fs";
import { chromium } from "playwright-core";
import { PNG } from "pngjs";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { CANONICAL_NAMES } from "../src/constants.js";
import { ServiceError } from "../src/errors.js";
import { CHROMIUM_LAUNCH_OPTIONS, RenderEngine, waitForAbort } from "../src/renderEngine.js";
import { coloredGlb, externalDependencyGlb } from "./fixture.js";

const browserAvailable = existsSync(chromium.executablePath());
const neverAbort = () => new AbortController().signal;
const BACKGROUND_RGB = [0x11, 0x15, 0x0f] as const;

function luminance(red: number, green: number, blue: number): number {
  return 0.2126 * red + 0.7152 * green + 0.0722 * blue;
}

function visibleModelStats(png: PNG): { coloredPixels: number; meanLuminance: number } {
  let coloredPixels = 0;
  let totalLuminance = 0;
  for (let index = 0; index < png.data.length; index += 4) {
    const red = png.data[index];
    const green = png.data[index + 1];
    const blue = png.data[index + 2];
    const contrast = Math.hypot(
      red - BACKGROUND_RGB[0], green - BACKGROUND_RGB[1], blue - BACKGROUND_RGB[2],
    );
    if (contrast > 30 && Math.max(red, green, blue) - Math.min(red, green, blue) > 20) {
      coloredPixels += 1;
      totalLuminance += luminance(red, green, blue);
    }
    expect(png.data[index + 3]).toBe(255);
  }
  return {
    coloredPixels,
    meanLuminance: coloredPixels === 0 ? 0 : totalLuminance / coloredPixels,
  };
}

describe("Chromium security configuration", () => {
  it("requires the Chromium sandbox and never supplies a no-sandbox argument", () => {
    expect(CHROMIUM_LAUNCH_OPTIONS.chromiumSandbox).toBe(true);
    expect(CHROMIUM_LAUNCH_OPTIONS.args ?? []).not.toContain("--no-sandbox");
  });

  it("rejects once on cancellation and observes a late operation rejection", async () => {
    const controller = new AbortController();
    let rejectLate!: (reason: unknown) => void;
    const operation = new Promise<void>((_resolve, reject) => { rejectLate = reject; });
    const result = waitForAbort(operation, controller.signal);
    controller.abort(new ServiceError(504, "render_timeout"));
    await expect(result).rejects.toMatchObject({ status: 504, code: "render_timeout" });
    rejectLate(new Error("late rejection"));
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
});

describe.skipIf(!browserAvailable)("visual renderer browser integration", () => {
  const engine = new RenderEngine();
  beforeAll(async () => engine.start(), 30_000);
  afterAll(async () => engine.close());

  it("renders colored canonical and saved perspective/orthographic views", async () => {
    const canonical = await engine.render(
      coloredGlb(), { kind: "canonical", recipe: "three-v2" }, neverAbort(),
    );
    expect(canonical).toMatchObject({ recipe: "three-v2", width: 640, height: 480 });
    expect(canonical.images.map(({ name }) => name)).toEqual(CANONICAL_NAMES);
    const stats = new Map<string, ReturnType<typeof visibleModelStats>>();
    for (const image of canonical.images) {
      const png = PNG.sync.read(Buffer.from(image.data, "base64"));
      expect([png.width, png.height]).toEqual([640, 480]);
      expect(image.mime_type).toBe("image/png");
      const imageStats = visibleModelStats(png);
      expect(imageStats.coloredPixels).toBeGreaterThan(500);
      stats.set(image.name, imageStats);
    }
    const pixels = PNG.sync.read(Buffer.from(canonical.images[0].data, "base64")).data;
    const colorCounts = [0, 0, 0];
    for (let index = 0; index < pixels.length; index += 4) {
      const channels = [pixels[index], pixels[index + 1], pixels[index + 2]];
      const strongest = channels.indexOf(Math.max(...channels));
      if (channels[strongest] > 60 && channels[strongest] > channels[(strongest + 1) % 3] * 1.4) {
        colorCounts[strongest] += 1;
      }
    }
    expect(colorCounts.every((count) => count > 100)).toBe(true);
    const bottom = stats.get("bottom")!;
    expect(bottom.coloredPixels).toBeGreaterThan(1_000);
    expect(bottom.meanLuminance).toBeGreaterThan(
      luminance(...BACKGROUND_RGB) + 25,
    );

    for (const projection of ["perspective", "orthographic"] as const) {
      const rendered = await engine.render(coloredGlb(), {
        kind: "view", recipe: "three-v2", camera: {
          target: [0, 0, 0], rotation: [0, 0, 0, 1], projection,
          distance: 8, field_of_view_degrees: 42, orthographic_scale: 5,
        },
      }, neverAbort());
      expect(rendered.images).toHaveLength(1);
      expect(rendered.images[0].name).toBe("view");
    }
  }, 60_000);

  it("rejects external GLB dependencies and recovers for the next render", async () => {
    await expect(engine.render(
      externalDependencyGlb(), { kind: "canonical", recipe: "three-v2" }, neverAbort(),
    ))
      .rejects.toMatchObject({ status: 422, code: "render_failed" });
    expect(engine.ready).toBe(true);
    await expect(engine.render(
      coloredGlb(), { kind: "canonical", recipe: "three-v2" }, neverAbort(),
    )).resolves.toMatchObject({ recipe: "three-v2", width: 640, height: 480 });
  });

  it("closes active browser work after cancellation and accepts the next render", async () => {
    const controller = new AbortController();
    const cancelled = engine.render(
      coloredGlb(), { kind: "canonical", recipe: "three-v2" }, controller.signal,
    );
    await vi.waitFor(() => expect(engine.activeJobCount).toBe(1));
    controller.abort(new ServiceError(499, "request_cancelled"));
    await expect(cancelled).rejects.toMatchObject({ status: 499, code: "request_cancelled" });
    await vi.waitFor(() => expect(engine.activeJobCount).toBe(0));
    expect(engine.activeHarnessJobCount).toBe(0);
    const recovered = await engine.render(
      coloredGlb(), { kind: "canonical", recipe: "three-v2" }, neverAbort(),
    );
    expect(recovered.images).toHaveLength(7);
  });
});
