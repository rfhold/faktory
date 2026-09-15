import { randomBytes } from "node:crypto";
import { readFile } from "node:fs/promises";
import { createServer, type Server } from "node:http";

const HTML = Buffer.from(`<!doctype html><html><head><meta charset="utf-8"><meta name="referrer" content="no-referrer"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'self'; connect-src 'self'; img-src 'self'; style-src 'unsafe-inline'"><style>html,body{margin:0;background:#11150f;overflow:hidden}canvas{display:block;width:640px;height:480px}</style></head><body><canvas width="640" height="480"></canvas><script type="module" src="/assets/harness.js"></script></body></html>`);

export interface HarnessJob {
  token: string;
  pageUrl: string;
  modelUrl: string;
  allowedPaths: ReadonlySet<string>;
  dispose(): void;
}

export class HarnessServer {
  private readonly jobs = new Map<string, Buffer>();
  private server: Server | undefined;
  private origin = "";
  private harnessScript: Buffer | undefined;

  get activeJobCount(): number {
    return this.jobs.size;
  }

  async start(): Promise<void> {
    try {
      this.harnessScript = await readFile(new URL("./harness.js", import.meta.url));
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
      this.harnessScript = await readFile(new URL("../dist/harness.js", import.meta.url));
    }
    this.server = createServer((request, response) => {
      response.setHeader("Cache-Control", "no-store");
      response.setHeader("X-Content-Type-Options", "nosniff");
      if (request.method !== "GET" || !request.url) {
        response.writeHead(404).end();
        return;
      }
      const path = new URL(request.url, this.origin).pathname;
      if (path === "/assets/harness.js") {
        response.writeHead(200, { "Content-Type": "text/javascript; charset=utf-8" }).end(this.harnessScript);
        return;
      }
      const match = /^\/jobs\/([a-f0-9]{48})\/(index\.html|model\.glb)$/.exec(path);
      const bytes = match ? this.jobs.get(match[1]) : undefined;
      if (!match || !bytes) {
        response.writeHead(404).end();
        return;
      }
      if (match[2] === "index.html") {
        response.writeHead(200, { "Content-Type": "text/html; charset=utf-8" }).end(HTML);
      } else {
        response.writeHead(200, { "Content-Type": "model/gltf-binary", "Content-Length": bytes.length }).end(bytes);
      }
    });
    await new Promise<void>((resolve, reject) => {
      this.server!.once("error", reject);
      this.server!.listen(0, "127.0.0.1", () => {
        this.server!.removeListener("error", reject);
        resolve();
      });
    });
    const address = this.server.address();
    if (!address || typeof address === "string") throw new Error("harness listener unavailable");
    this.origin = `http://127.0.0.1:${address.port}`;
  }

  createJob(glb: Buffer): HarnessJob {
    const token = randomBytes(24).toString("hex");
    this.jobs.set(token, glb);
    const pagePath = `/jobs/${token}/index.html`;
    const modelPath = `/jobs/${token}/model.glb`;
    return {
      token,
      pageUrl: `${this.origin}${pagePath}`,
      modelUrl: `${this.origin}${modelPath}`,
      allowedPaths: new Set([pagePath, modelPath, "/assets/harness.js"]),
      dispose: () => { this.jobs.delete(token); },
    };
  }

  async close(): Promise<void> {
    this.jobs.clear();
    if (!this.server) return;
    const server = this.server;
    this.server = undefined;
    await new Promise<void>((resolve, reject) => {
      server.close((error) => error ? reject(error) : resolve());
    });
  }
}
