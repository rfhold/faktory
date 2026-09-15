import { chromium, type Browser, type BrowserContext, type LaunchOptions } from "playwright-core";
import {
  CANONICAL_NAMES,
  CONTEXT_CLOSE_GRACE_MS,
  RECIPE,
  RENDER_HEIGHT,
  RENDER_WIDTH,
} from "./constants.js";
import { ServiceError } from "./errors.js";
import { HarnessServer } from "./harnessServer.js";
import { validatePng } from "./png.js";
import { validateResponseByteLength } from "./limits.js";
import type { RenderSpec } from "./spec.js";

export const CHROMIUM_LAUNCH_OPTIONS: LaunchOptions = Object.freeze({
  headless: true,
  chromiumSandbox: true,
});

export interface RenderImage {
  name: (typeof CANONICAL_NAMES)[number] | "view";
  mime_type: "image/png";
  data: string;
}

export interface RenderResponse {
  recipe: typeof RECIPE;
  width: typeof RENDER_WIDTH;
  height: typeof RENDER_HEIGHT;
  images: RenderImage[];
}

function abortReason(signal: AbortSignal): unknown {
  return signal.reason ?? new ServiceError(499, "request_cancelled");
}

export function waitForAbort<T>(operation: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) return Promise.reject(abortReason(signal));
  return new Promise<T>((resolve, reject) => {
    const onAbort = () => {
      signal.removeEventListener("abort", onAbort);
      reject(abortReason(signal));
    };
    signal.addEventListener("abort", onAbort, { once: true });
    void operation.then(
      (value) => {
        signal.removeEventListener("abort", onAbort);
        resolve(value);
      },
      (error: unknown) => {
        signal.removeEventListener("abort", onAbort);
        reject(error);
      },
    );
  });
}

async function closesWithinGrace(close: Promise<void>): Promise<boolean> {
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      close.then(() => true),
      new Promise<false>((resolve) => {
        timer = setTimeout(() => resolve(false), CONTEXT_CLOSE_GRACE_MS);
      }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

export class RenderEngine {
  private browser: Browser | undefined;
  private readonly harness = new HarnessServer();
  private readonly activeContexts = new Set<BrowserContext>();

  get ready(): boolean {
    return this.browser?.isConnected() === true;
  }

  get activeJobCount(): number {
    return this.activeContexts.size;
  }

  get activeHarnessJobCount(): number {
    return this.harness.activeJobCount;
  }

  async start(): Promise<void> {
    try {
      await this.harness.start();
      this.browser = await chromium.launch(CHROMIUM_LAUNCH_OPTIONS);
    } catch (error) {
      await this.harness.close().catch(() => undefined);
      throw error;
    }
  }

  async render(glb: Buffer, spec: RenderSpec, signal: AbortSignal): Promise<RenderResponse> {
    if (!this.browser?.isConnected()) throw new ServiceError(503, "renderer_not_ready");
    if (signal.aborted) throw abortReason(signal);
    const job = this.harness.createJob(glb);
    let context: BrowserContext | undefined;
    let cancellationClose: Promise<void> | undefined;
    let blockedRequests = 0;
    const cancel = () => {
      job.dispose();
      if (context) {
        cancellationClose ??= context.close().catch(() => undefined);
        void cancellationClose;
      }
    };
    signal.addEventListener("abort", cancel, { once: true });
    try {
      const pendingContext = this.browser.newContext({
        viewport: { width: RENDER_WIDTH, height: RENDER_HEIGHT },
        deviceScaleFactor: 1,
        colorScheme: "light",
        serviceWorkers: "block",
      }).then(async (created) => {
        if (signal.aborted) {
          await created.close().catch(() => undefined);
          throw abortReason(signal);
        }
        return created;
      });
      context = await waitForAbort(pendingContext, signal);
      this.activeContexts.add(context);
      await context.route("**/*", async (route) => {
        const url = new URL(route.request().url());
        if (url.origin === new URL(job.pageUrl).origin && job.allowedPaths.has(url.pathname)) {
          await route.continue();
        } else {
          blockedRequests += 1;
          await route.abort("blockedbyclient");
        }
      });
      const page = await context.newPage();
      page.setDefaultTimeout(30_000);
      await page.goto(job.pageUrl, { waitUntil: "load" });
      await page.evaluate(async (modelUrl) => {
        const harness = (window as unknown as {
          faktoryHarness: { load(url: string): Promise<void> };
        }).faktoryHarness;
        await harness.load(modelUrl);
      }, job.modelUrl);
      if (blockedRequests > 0) throw new ServiceError(422, "external_glb_dependency");

      const canvas = page.locator("canvas");
      const images: RenderImage[] = [];
      if (spec.kind === "canonical") {
        for (const name of CANONICAL_NAMES) {
          await page.evaluate(async (viewName) => {
            const harness = (window as unknown as {
              faktoryHarness: { renderCanonical(value: string): Promise<void> };
            }).faktoryHarness;
            await harness.renderCanonical(viewName);
          }, name);
          images.push(await this.capture(canvas, name));
        }
      } else {
        await page.evaluate(async (camera) => {
          const harness = (window as unknown as {
            faktoryHarness: { renderView(value: typeof camera): Promise<void> };
          }).faktoryHarness;
          await harness.renderView(camera);
        }, spec.camera);
        images.push(await this.capture(canvas, "view"));
      }
      const response: RenderResponse = {
        recipe: RECIPE,
        width: RENDER_WIDTH,
        height: RENDER_HEIGHT,
        images,
      };
      validateResponseByteLength(Buffer.byteLength(JSON.stringify(response)));
      return response;
    } catch (error) {
      if (signal.aborted) throw abortReason(signal);
      if (blockedRequests > 0) throw new ServiceError(422, "external_glb_dependency");
      if (error instanceof ServiceError) throw error;
      throw new ServiceError(422, "render_failed");
    } finally {
      signal.removeEventListener("abort", cancel);
      if (context) {
        const close = cancellationClose ?? context.close().catch(() => undefined);
        if (!await closesWithinGrace(close)) {
          const browser = this.browser;
          this.browser = undefined;
          if (browser) void browser.close().catch(() => undefined);
        }
        this.activeContexts.delete(context);
      }
      job.dispose();
    }
  }

  private async capture(
    canvas: import("playwright-core").Locator,
    name: RenderImage["name"],
  ): Promise<RenderImage> {
    const png = await canvas.screenshot({ type: "png", animations: "disabled" });
    validatePng(png);
    return { name, mime_type: "image/png", data: png.toString("base64") };
  }

  async close(): Promise<void> {
    const browser = this.browser;
    this.browser = undefined;
    if (browser) await browser.close().catch(() => undefined);
    await this.harness.close();
  }
}
