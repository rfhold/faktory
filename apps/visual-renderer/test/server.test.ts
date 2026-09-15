import { afterEach, describe, expect, it, vi } from "vitest";
import { ServerResponse } from "node:http";
import { ServiceError } from "../src/errors.js";
import type { RenderResponse } from "../src/renderEngine.js";
import { VisualRendererServer, type Renderer } from "../src/server.js";
import { coloredGlb } from "./fixture.js";

function specHeader(value: unknown): string {
  return Buffer.from(JSON.stringify(value)).toString("base64url");
}

describe("render HTTP protocol", () => {
  let server: VisualRendererServer | undefined;
  afterEach(async () => server?.close());

  const result: RenderResponse = {
    recipe: "three-v2", width: 640, height: 480,
    images: [{ name: "view", mime_type: "image/png", data: "cG5n" }],
  };

  function renderOptions(signal?: AbortSignal): RequestInit {
    return {
      method: "POST",
      headers: {
        "content-type": "application/octet-stream",
        "x-faktory-render-spec": specHeader({ kind: "canonical", recipe: "three-v2" }),
      },
      body: new Uint8Array(coloredGlb()),
      signal,
    };
  }

  it("bounds the fixed deadline below the client timeout", () => {
    const engine: Renderer = { ready: true, render: async () => result };
    expect(() => new VisualRendererServer(engine, 0)).toThrow();
    expect(() => new VisualRendererServer(engine, 29_999)).not.toThrow();
    expect(() => new VisualRendererServer(engine, 30_000)).toThrow();
  });

  async function start(engine: Renderer = {
      ready: true,
      render: async () => result,
    }, deadlineMs = 25_000): Promise<string> {
    server = new VisualRendererServer(engine, deadlineMs);
    await server.start(0);
    return `http://127.0.0.1:${server.port}`;
  }

  it("serves bounded health and exact success JSON", async () => {
    const origin = await start();
    expect(await (await fetch(`${origin}/health/live`)).json()).toEqual({ status: "live" });
    expect(await (await fetch(`${origin}/health/ready`)).json()).toEqual({ status: "ready" });
    const response = await fetch(`${origin}/v1/render`, {
      method: "POST",
      headers: {
        "content-type": "application/octet-stream",
        "x-faktory-render-spec": specHeader({
          kind: "view", recipe: "three-v2", camera: {
            target: [0, 0, 0], rotation: [0, 0, 0, 1], projection: "perspective",
            distance: 5, field_of_view_degrees: 42, orthographic_scale: 4,
          },
        }),
      },
      body: new Uint8Array(coloredGlb()),
    });
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({
      recipe: "three-v2", width: 640, height: 480,
      images: [{ name: "view", mime_type: "image/png", data: "cG5n" }],
    });
  });

  it("rejects methods, content types, malformed GLBs, and unknown routes safely", async () => {
    const origin = await start();
    const method = await fetch(`${origin}/v1/render`);
    expect(method.status).toBe(405);
    expect(method.headers.get("allow")).toBe("POST");
    expect(await method.json()).toEqual({ error: "method_not_allowed" });

    const healthMethod = await fetch(`${origin}/health/live`, { method: "POST" });
    expect(healthMethod.status).toBe(405);
    expect(healthMethod.headers.get("allow")).toBe("GET");

    const contentType = await fetch(`${origin}/v1/render`, { method: "POST", body: "bad" });
    expect(contentType.status).toBe(415);
    expect(await contentType.json()).toEqual({ error: "unsupported_content_type" });

    const malformed = await fetch(`${origin}/v1/render`, {
      method: "POST",
      headers: {
        "content-type": "application/octet-stream",
        "x-faktory-render-spec": specHeader({ kind: "canonical", recipe: "three-v2" }),
      },
      body: Buffer.from("not a glb"),
    });
    expect(malformed.status).toBe(422);
    expect(await malformed.json()).toEqual({ error: "malformed_glb" });

    expect((await fetch(`${origin}/private`)).status).toBe(404);
  });

  it("returns a stable complete-job timeout and recovers the queue", async () => {
    let calls = 0;
    let cleaned = false;
    const origin = await start({
      ready: true,
      render: async (_glb, _spec, signal) => {
        calls += 1;
        if (calls > 1) return result;
        return new Promise<RenderResponse>((_resolve, reject) => {
          signal.addEventListener("abort", () => {
            cleaned = true;
            reject(signal.reason);
          }, { once: true });
        });
      },
    }, 20);
    const options = {
      method: "POST",
      headers: {
        "content-type": "application/octet-stream",
        "x-faktory-render-spec": specHeader({ kind: "canonical", recipe: "three-v2" }),
      },
      body: new Uint8Array(coloredGlb()),
    };
    const timeout = await fetch(`${origin}/v1/render`, options);
    expect(timeout.status).toBe(504);
    expect(await timeout.json()).toEqual({ error: "render_timeout" });
    expect(cleaned).toBe(true);
    expect((await fetch(`${origin}/v1/render`, options)).status).toBe(200);
  });

  it("cancels active work after a disconnect and admits the next request", async () => {
    let started!: () => void;
    const activeStarted = new Promise<void>((resolve) => { started = resolve; });
    let calls = 0;
    let cleaned = false;
    const origin = await start({
      ready: true,
      render: async (_glb, _spec, signal) => {
        calls += 1;
        if (calls > 1) return result;
        started();
        return new Promise<RenderResponse>((_resolve, reject) => {
          signal.addEventListener("abort", () => {
            cleaned = true;
            reject(signal.reason);
          }, { once: true });
        });
      },
    });
    const controller = new AbortController();
    const options = {
      method: "POST",
      headers: {
        "content-type": "application/octet-stream",
        "x-faktory-render-spec": specHeader({ kind: "canonical", recipe: "three-v2" }),
      },
      body: new Uint8Array(coloredGlb()),
      signal: controller.signal,
    };
    const abandoned = fetch(`${origin}/v1/render`, options);
    await activeStarted;
    controller.abort();
    await expect(abandoned).rejects.toThrow();
    await vi.waitFor(() => expect(cleaned).toBe(true));
    expect((await fetch(`${origin}/v1/render`, { ...options, signal: undefined })).status).toBe(200);
  });

  it("removes a disconnected pending HTTP request before it can render", async () => {
    let releaseActive!: (value: RenderResponse) => void;
    let activeStarted!: () => void;
    const started = new Promise<void>((resolve) => { activeStarted = resolve; });
    let calls = 0;
    const origin = await start({
      ready: true,
      render: async () => {
        calls += 1;
        if (calls > 1) return result;
        activeStarted();
        return new Promise<RenderResponse>((resolve) => { releaseActive = resolve; });
      },
    });

    const active = fetch(`${origin}/v1/render`, renderOptions());
    await started;
    const pendingController = new AbortController();
    const abandoned = fetch(`${origin}/v1/render`, renderOptions(pendingController.signal));
    await vi.waitFor(() => expect(server!.pendingRenderCount).toBe(1));
    pendingController.abort();
    await expect(abandoned).rejects.toThrow();
    await vi.waitFor(() => expect(server!.pendingRenderCount).toBe(0));

    const replacement = fetch(`${origin}/v1/render`, renderOptions());
    await vi.waitFor(() => expect(server!.pendingRenderCount).toBe(1));
    releaseActive(result);
    expect((await active).status).toBe(200);
    expect((await replacement).status).toBe(200);
    expect(calls).toBe(2);
  });

  it("observes late render rejection after disconnect without a second response", async () => {
    const unhandled: unknown[] = [];
    const onUnhandled = (reason: unknown) => { unhandled.push(reason); };
    const responseEnd = vi.spyOn(ServerResponse.prototype, "end");
    process.on("unhandledRejection", onUnhandled);
    try {
      let rejectLate!: (reason: unknown) => void;
      let markStarted!: () => void;
      let markAborted!: () => void;
      const started = new Promise<void>((resolve) => { markStarted = resolve; });
      const aborted = new Promise<void>((resolve) => { markAborted = resolve; });
      let calls = 0;
      const origin = await start({
        ready: true,
        render: async (_glb, _spec, signal) => {
          calls += 1;
          if (calls > 1) return result;
          markStarted();
          signal.addEventListener("abort", markAborted, { once: true });
          return new Promise<RenderResponse>((_resolve, reject) => { rejectLate = reject; });
        },
      });
      const controller = new AbortController();
      const abandoned = fetch(`${origin}/v1/render`, renderOptions(controller.signal));
      await started;
      controller.abort();
      await expect(abandoned).rejects.toThrow();
      await aborted;
      rejectLate(new Error("late render rejection"));

      expect((await fetch(`${origin}/v1/render`, renderOptions())).status).toBe(200);
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(unhandled).toEqual([]);
      expect(responseEnd).toHaveBeenCalledTimes(1);
      expect(calls).toBe(2);
    } finally {
      process.removeListener("unhandledRejection", onUnhandled);
      responseEnd.mockRestore();
    }
  });

  it("recovers after an unconditional safe render rejection", async () => {
    let calls = 0;
    const origin = await start({
      ready: true,
      render: async () => {
        calls += 1;
        if (calls === 1) throw new ServiceError(422, "render_failed");
        return result;
      },
    });
    const rejected = await fetch(`${origin}/v1/render`, renderOptions());
    expect(rejected.status).toBe(422);
    expect(await rejected.json()).toEqual({ error: "render_failed" });
    expect((await fetch(`${origin}/v1/render`, renderOptions())).status).toBe(200);
    expect(calls).toBe(2);
  });
});
