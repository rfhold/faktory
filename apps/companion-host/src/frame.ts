import { MAX_BYTES, foundationCss, installComponents, sanitize, safeCss, validateCompanion } from "@rfhold/faktory-companion-kit";
import { CutRenderBudgetError, renderCutWidget } from "@rfhold/faktory-companion-kit/cut-widget";
declare global {
  var __companionOrigins: string[];
}
installComponents(window);
const root = document.getElementById("companion")!;
let channel: MessagePort | undefined;
let urls: string[] = [];
const MAX_RENDER_NODES = 16384;
const MAX_CUT_WIDGETS = 16;
const MAX_ASSET_REFERENCES = 128;
const bytes = (value: string) => new TextEncoder().encode(value).byteLength;
function nodeCount(node: Node): number {
  const walker = document.createTreeWalker(node, NodeFilter.SHOW_ALL);
  let count = 0;
  while (walker.nextNode()) {
    if (++count > MAX_RENDER_NODES) throw new Error("Companion is too complex");
  }
  return count;
}
const cleanup = () => {
  channel?.close();
  channel = undefined;
  urls.forEach(url => URL.revokeObjectURL(url));
  urls = [];
  root.replaceChildren();
};
window.addEventListener("pagehide", cleanup);
function handshake(event: MessageEvent) {
  const v = event.data;
  if (
    channel || event.source !== parent || !globalThis.__companionOrigins.includes(event.origin) ||
    !v || typeof v !== "object" || Object.keys(v).sort().join() !== "nonce,revision,type" ||
    v.type !== "companion:init" || typeof v.nonce !== "string" || !/^[a-f0-9]{32}$/.test(v.nonce) ||
    typeof v.revision !== "string" || !/^[a-zA-Z0-9._-]{1,128}$/.test(v.revision) || event.ports.length !== 1
  )
    return;
  channel = event.ports[0];
  window.removeEventListener("message", handshake);
  const port = channel;
  let delivered = false;
  port.onmessage = (message) => {
    const m = message.data;
    if (!m || typeof m !== "object" || m.nonce !== v.nonce || m.revision !== v.revision)
      return;
    if (m.type === "companion:dispose" && Object.keys(m).sort().join() === "nonce,revision,type") {
      cleanup();
      return;
    }
    if (m.type === "companion:print" && Object.keys(m).sort().join() === "nonce,revision,type" && delivered) {
      window.print();
      return;
    }
    if (delivered || m.type !== "companion:content" || Object.keys(m).sort().join() !== "companion,nonce,revision,type")
      return;
    delivered = true;
    try {
      const c = validateCompanion(m.companion, v.revision);
      const fragment = sanitize(document, c.html);
      const assets = new Map(c.assets.map(a => [a.id, a]));
      const references = fragment.querySelectorAll("[data-companion-asset]");
      if (references.length > MAX_ASSET_REFERENCES || fragment.querySelectorAll("fk-cut-layout").length > MAX_CUT_WIDGETS)
        throw new Error("Companion is too complex");
      let nodes = nodeCount(fragment) + 2;
      let outputBytes = bytes(c.html) + bytes(c.css) + bytes(foundationCss);
      const reserve = (addedNodes: number, addedBytes = 0) => {
        nodes += addedNodes;
        outputBytes += addedBytes;
        if (nodes > MAX_RENDER_NODES || outputBytes > MAX_BYTES) throw new Error("Companion is too complex");
      };
      // Reserve connectedCallback expansion while the source is still inert.
      for (const tabs of fragment.querySelectorAll("fk-tabs")) {
        const panels = Array.from(tabs.children).filter(el => el.hasAttribute("data-tab"));
        reserve(1 + panels.length * 2, panels.reduce((total, panel) => total + bytes(panel.getAttribute("data-tab") ?? ""), 0));
      }
      for (const measurement of fragment.querySelectorAll("fk-measurement"))
        reserve(1, bytes(["label", "value", "unit"].map(key => measurement.getAttribute(key)).filter(Boolean).join(" ")));
      reserve(0);
      const svgUrls = new Map<string, string>();
      const widgets = new Map<string, { fragment: DocumentFragment; nodes: number; bytes: number }>();
      const jsonText = new Map<string, string>();
      for (const el of references) {
        const asset = assets.get(el.getAttribute("data-companion-asset")!);
        if (!asset)
          continue;
        if (asset.mime_type === "image/svg+xml") {
          let url = svgUrls.get(asset.id);
          if (!url) {
            const safe = sanitize(document, asset.content, true);
            const svg = safe.firstElementChild;
            if (!svg || svg.localName !== "svg" || safe.children.length !== 1)
              throw new Error("Invalid SVG asset");
            const content = new XMLSerializer().serializeToString(svg);
            reserve(nodeCount(safe), bytes(content));
            url = URL.createObjectURL(new Blob([content], { type: "image/svg+xml" }));
            urls.push(url);
            svgUrls.set(asset.id, url);
          }
          if (el.tagName === "IMG") {
            el.setAttribute("src", url);
          }
          else {
            reserve(1);
            const image = document.createElement("img");
            image.src = url;
            image.alt = el.getAttribute("alt") ?? "Companion diagram";
            el.replaceChildren(image);
          }
        }
        else if (el.localName === "fk-cut-layout") {
          let cached = widgets.get(asset.id);
          if (!cached) {
            let output: DocumentFragment;
            try {
              output = renderCutWidget(document, JSON.parse(asset.content), MAX_RENDER_NODES - nodes);
            }
            catch (error) {
              if (error instanceof CutRenderBudgetError) throw error;
              output = document.createDocumentFragment();
              output.append("Cut layout unavailable. Check explicit mm inputs and available stock.");
            }
            cached = { fragment: output, nodes: nodeCount(output), bytes: bytes(output.textContent ?? "") };
            reserve(cached.nodes, cached.bytes);
            widgets.set(asset.id, cached);
          }
          // Reserve before cloning: budget failures must escape the optional tool error handler.
          reserve(cached.nodes, cached.bytes);
          el.replaceChildren(cached.fragment.cloneNode(true));
        }
        else {
          let text = jsonText.get(asset.id);
          if (text === undefined) {
            text = JSON.stringify(JSON.parse(asset.content), null, 2);
            jsonText.set(asset.id, text);
          }
          reserve(2, bytes(text));
          const pre = document.createElement("pre");
          pre.textContent = text;
          el.replaceChildren(pre);
        }
      }
      const style = document.createElement("style");
      style.textContent = foundationCss + "\n" + safeCss(c.css);
      root.replaceChildren(style, fragment);
      port.postMessage({ type: "companion:rendered", nonce: v.nonce, revision: v.revision });
    }
    catch {
      port.postMessage({ type: "companion:error", nonce: v.nonce, revision: v.revision });
      cleanup();
      root.textContent = "Companion unavailable: invalid document.";
    }
  };
  port.start();
  port.postMessage({ type: "companion:ready", nonce: v.nonce, revision: v.revision });
}
window.addEventListener("message", handshake);
