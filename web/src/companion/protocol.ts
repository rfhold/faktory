import { MAX_BYTES, validateCompanion, type Companion } from "@rfhold/faktory-companion-kit";
export function companionOrigin(config: string | undefined, parentOrigin: string): string | undefined {
  if (!config)
    return;
  try {
    const url = new URL(config);
    if (["https:", "http:"].includes(url.protocol) && url.origin === config && url.origin !== parentOrigin)
      return url.origin;
  }
  catch { /* Invalid configuration fails closed. */ }
}
export async function fetchCompanionOrigin(signal?: AbortSignal): Promise<string | null> {
  const response = await fetch("/runtime-config", { credentials: "same-origin", signal, headers: { Accept: "application/json" }, redirect: "error", cache: "no-store" });
  if (!response.ok || response.headers.get("content-type")?.split(";")[0].trim() !== "application/json")
    throw new Error("Companion configuration unavailable");
  const reader = response.body?.getReader();
  if (!reader)
    throw new Error("Companion configuration unavailable");
  let text = "", length = 0;
  const decoder = new TextDecoder();
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done)
        break;
      length += value.byteLength;
      if (length > 4096)
        throw new Error("Companion configuration unavailable");
      text += decoder.decode(value, { stream: true });
    }
    const config: unknown = JSON.parse(text + decoder.decode());
    if (!config || typeof config !== "object" || Object.keys(config).join() !== "companion_origin")
      throw new Error("Companion configuration unavailable");
    const origin = (config as {
      companion_origin: unknown;
    }).companion_origin;
    if (origin === null)
      return null;
    if (typeof origin !== "string")
      throw new Error("Companion configuration unavailable");
    return companionOrigin(origin, window.location.origin) ?? null;
  }
  finally {
    await reader.cancel();
  }
}
export async function fetchCompanion(modelId: string, revision: string, signal?: AbortSignal): Promise<Companion | null> {
  const response = await fetch(`/artifacts/${encodeURIComponent(modelId)}/${encodeURIComponent(revision)}/companion`, { credentials: "same-origin", signal, headers: { Accept: "application/json" }, redirect: "error" });
  if (response.status === 404)
    return null;
  if (!response.ok || response.headers.get("content-type")?.split(";")[0].trim() !== "application/json")
    throw new Error("Companion unavailable");
  const reader = response.body?.getReader();
  if (!reader)
    throw new Error("Companion unavailable");
  const decoder = new TextDecoder();
  let text = "", length = 0;
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done)
        break;
      length += value.byteLength;
      if (length > MAX_BYTES)
        throw new Error("Companion exceeds size limit");
      text += decoder.decode(value, { stream: true });
    }
    text += decoder.decode();
    return validateCompanion(JSON.parse(text), revision);
  }
  finally {
    await reader.cancel();
  }
}
export function mountCompanion(frame: HTMLIFrameElement, companion: Companion, onState: (state: "ready" | "error") => void): {
  print: () => void;
  destroy: () => void;
} {
  const channel = new MessageChannel();
  const nonce = Array.from(crypto.getRandomValues(new Uint8Array(16)), n => n.toString(16).padStart(2, "0")).join("");
  const revision = companion.revision;
  let ready = false, sent = false, destroyed = false;
  const timeout = setTimeout(() => { onState("error"); destroy(); }, 10000);
  const destroy = () => {
    if (!destroyed)
      channel.port1.postMessage({ type: "companion:dispose", nonce, revision }); destroyed = true; clearTimeout(timeout); channel.port1.close(); channel.port2.close();
  };
  channel.port1.onmessage = ({ data: m }) => {
    if (destroyed || !m || typeof m !== "object" || Object.keys(m).sort().join() !== "nonce,revision,type" || m.nonce !== nonce || m.revision !== revision)
      return;
    if (m.type === "companion:ready" && !sent) {
      sent = true;
      channel.port1.postMessage({ type: "companion:content", nonce, revision, companion });
    }
    else if (m.type === "companion:rendered" && sent && !ready) {
      ready = true;
      clearTimeout(timeout);
      onState("ready");
    }
    else if (m.type === "companion:error") {
      onState("error");
      destroy();
    }
  };
  channel.port1.start();
  // A sandbox without allow-same-origin has an opaque origin: target the exact WindowProxy, not a global receiver.
  frame.contentWindow?.postMessage({ type: "companion:init", nonce, revision }, "*", [channel.port2]);
  return {
    destroy, print: () => {
      if (ready && !destroyed)
        channel.port1.postMessage({ type: "companion:print", nonce, revision });
    }
  };
}
