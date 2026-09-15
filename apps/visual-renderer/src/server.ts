import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { RENDER_DEADLINE_MS } from "./constants.js";
import { errorMessage, ServiceError } from "./errors.js";
import { validateGlb } from "./glb.js";
import { validateGlbByteLength } from "./limits.js";
import { RenderQueue } from "./queue.js";
import type { RenderResponse } from "./renderEngine.js";
import { parseRenderSpec, type RenderSpec } from "./spec.js";

export interface Renderer {
  readonly ready: boolean;
  render(glb: Buffer, spec: RenderSpec, signal: AbortSignal): Promise<RenderResponse>;
}

function sendJson(response: ServerResponse, status: number, value: unknown): void {
  const body = Buffer.from(JSON.stringify(value));
  response.writeHead(status, {
    "Content-Type": "application/json; charset=utf-8",
    "Content-Length": body.length,
    "Cache-Control": "no-store",
    "X-Content-Type-Options": "nosniff",
  }).end(body);
}

async function readBody(request: IncomingMessage, signal: AbortSignal): Promise<Buffer> {
  const declared = request.headers["content-length"];
  if (declared && !/^\d+$/.test(declared)) {
    request.resume();
    throw new ServiceError(413, "glb_too_large");
  }
  if (declared) validateGlbByteLength(Number(declared));
  if (signal.aborted) throw signal.reason;
  return new Promise<Buffer>((resolve, reject) => {
    const chunks: Buffer[] = [];
    let total = 0;
    const cleanup = () => {
      request.removeListener("data", onData);
      request.removeListener("end", onEnd);
      request.removeListener("error", onError);
      signal.removeEventListener("abort", onAbort);
    };
    const fail = (error: unknown) => {
      cleanup();
      chunks.length = 0;
      reject(error);
    };
    const onAbort = () => {
      request.resume();
      fail(signal.reason ?? new ServiceError(499, "request_cancelled"));
    };
    const onError = () => fail(new ServiceError(499, "request_cancelled"));
    const onData = (chunk: Buffer | Uint8Array) => {
      const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
      total += bytes.length;
      try {
        validateGlbByteLength(total);
      } catch (error) {
        request.resume();
        fail(error);
        return;
      }
      chunks.push(bytes);
    };
    const onEnd = () => {
      cleanup();
      if (total === 0) reject(new ServiceError(422, "malformed_glb"));
      else resolve(Buffer.concat(chunks, total));
    };
    request.on("data", onData);
    request.once("end", onEnd);
    request.once("error", onError);
    signal.addEventListener("abort", onAbort, { once: true });
  });
}

export class VisualRendererServer {
  private readonly queue = new RenderQueue();
  private server: Server | undefined;

  constructor(
    private readonly engine: Renderer,
    private readonly deadlineMs = RENDER_DEADLINE_MS,
  ) {
    if (!Number.isSafeInteger(deadlineMs) || deadlineMs < 1 || deadlineMs >= 30_000) {
      throw new Error("render deadline must be between 1 and 29999 milliseconds");
    }
  }

  get port(): number {
    const address = this.server?.address();
    if (!address || typeof address === "string") throw new Error("server is not listening");
    return address.port;
  }

  get pendingRenderCount(): number {
    return this.queue.pendingCount;
  }

  async start(port: number): Promise<void> {
    this.server = createServer((request, response) => {
      void this.handle(request, response).catch((error: unknown) => {
        const serviceError = error instanceof ServiceError
          ? error
          : new ServiceError(500, "internal_error");
        if (!response.destroyed && !response.headersSent) {
          sendJson(response, serviceError.status, errorMessage(serviceError.code));
        }
        else response.destroy();
      });
    });
    this.server.requestTimeout = 35_000;
    this.server.headersTimeout = 10_000;
    await new Promise<void>((resolve, reject) => {
      this.server!.once("error", reject);
      this.server!.listen(port, "0.0.0.0", () => {
        this.server!.removeListener("error", reject);
        resolve();
      });
    });
  }

  private async handle(request: IncomingMessage, response: ServerResponse): Promise<void> {
    const path = new URL(request.url ?? "/", "http://service.invalid").pathname;
    const healthRoute = path === "/health/live" || path === "/health/ready";
    if (healthRoute && request.method !== "GET") {
      response.setHeader("Allow", "GET");
      sendJson(response, 405, errorMessage("method_not_allowed"));
      return;
    }
    if (path === "/health/live") {
      sendJson(response, 200, { status: "live" });
      return;
    }
    if (path === "/health/ready") {
      sendJson(response, this.engine.ready ? 200 : 503, { status: this.engine.ready ? "ready" : "not_ready" });
      return;
    }
    if (path !== "/v1/render") {
      sendJson(response, 404, errorMessage("not_found"));
      return;
    }
    if (request.method !== "POST") {
      response.setHeader("Allow", "POST");
      sendJson(response, 405, errorMessage("method_not_allowed"));
      return;
    }
    if (request.headers["content-type"] !== "application/octet-stream") {
      throw new ServiceError(415, "unsupported_content_type");
    }
    const distinctSpecs = request.headersDistinct["x-faktory-render-spec"];
    if (!distinctSpecs || distinctSpecs.length !== 1) throw new ServiceError(400, "invalid_render_spec");
    const spec = parseRenderSpec(distinctSpecs[0]);
    const controller = new AbortController();
    let complete = false;
    const cancelRequest = () => {
      if (!complete) controller.abort(new ServiceError(499, "request_cancelled"));
    };
    const cancelResponse = () => {
      if (!complete && !response.writableFinished) cancelRequest();
    };
    request.once("aborted", cancelRequest);
    response.once("close", cancelResponse);
    const deadline = setTimeout(() => {
      controller.abort(new ServiceError(504, "render_timeout"));
    }, this.deadlineMs);
    deadline.unref();
    try {
      const result = await this.queue.run<RenderResponse>(controller.signal, async (signal) => {
        const glb = await readBody(request, signal);
        if (signal.aborted) throw signal.reason;
        validateGlb(glb);
        return this.engine.render(glb, spec, signal);
      });
      if (controller.signal.aborted) throw controller.signal.reason;
      complete = true;
      sendJson(response, 200, result);
    } finally {
      complete = true;
      clearTimeout(deadline);
      request.removeListener("aborted", cancelRequest);
      response.removeListener("close", cancelResponse);
    }
  }

  async close(): Promise<void> {
    if (!this.server) return;
    const server = this.server;
    this.server = undefined;
    await new Promise<void>((resolve, reject) => {
      server.close((error) => error ? reject(error) : resolve());
    });
  }
}
