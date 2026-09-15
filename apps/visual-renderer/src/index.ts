import { RenderEngine } from "./renderEngine.js";
import { VisualRendererServer } from "./server.js";

const rawPort = process.env.PORT ?? "8081";
if (!/^\d+$/.test(rawPort) || Number(rawPort) < 1 || Number(rawPort) > 65_535) {
  throw new Error("invalid PORT");
}

const engine = new RenderEngine();
const server = new VisualRendererServer(engine);
try {
  await engine.start();
  await server.start(Number(rawPort));
} catch (error) {
  await server.close().catch(() => undefined);
  await engine.close().catch(() => undefined);
  throw error;
}
process.stdout.write(`visual_renderer_listening port=${rawPort}\n`);

let stopping = false;
async function stop(): Promise<void> {
  if (stopping) return;
  stopping = true;
  await server.close();
  await engine.close();
}

process.once("SIGTERM", () => { void stop(); });
process.once("SIGINT", () => { void stop(); });
