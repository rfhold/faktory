import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { randomBytes } from "node:crypto";
const script = readFileSync(new URL("./frame.js", import.meta.url), "utf8");
const origins = (process.env.COMPANION_PARENT_ORIGINS ?? "").split(/\s+/).filter(Boolean);
for (const origin of origins) {
  const url = new URL(origin);
  if (!["http:", "https:"].includes(url.protocol) || url.origin !== origin)
    throw new Error("COMPANION_PARENT_ORIGINS must contain exact HTTP(S) origins");
}
const server = createServer((req, res) => {
  res.setHeader("Cache-Control", "no-store");
  res.setHeader("X-Content-Type-Options", "nosniff");
  res.setHeader("Referrer-Policy", "no-referrer");
  if (req.method === "GET" && ["/health", "/ready"].includes(req.url ?? "")) {
    res.writeHead(200, { "Content-Type": "text/plain" });
    res.end("ok");
    return;
  }
  if (req.method !== "GET" || req.url !== "/frame") {
    res.writeHead(404);
    res.end();
    return;
  }
  const nonce = randomBytes(24).toString("base64");
  res.setHeader("Content-Security-Policy", `default-src 'none'; connect-src 'none'; frame-src 'none'; object-src 'none'; font-src 'none'; img-src blob: data:; style-src 'unsafe-inline'; script-src 'nonce-${nonce}'; base-uri 'none'; form-action 'none'; frame-ancestors ${origins.length ? origins.join(" ") : "'none'"}`);
  res.setHeader("Permissions-Policy", "camera=(), microphone=(), geolocation=(), payment=(), usb=()");
  res.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
  res.end(`<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><main id="companion"></main><script nonce="${nonce}">globalThis.__companionOrigins=${JSON.stringify(origins).replace(/</g, "\\u003c")};${script.replace(/<\/script/gi, "<\\/script")}</script></body></html>`);
});
server.listen(Number(process.env.PORT ?? 8082), "0.0.0.0");
