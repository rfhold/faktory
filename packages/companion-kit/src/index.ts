export const KIT_VERSION = "1";
export const MAX_BYTES = 1048576;
export type Companion = {
  format: "faktory-companion-v1";
  revision: string;
  kit_version: "1";
  html: string;
  css: string;
  assets: {
    id: string;
    mime_type: string;
    content: string;
  }[];
};
const bytes = (s: string) => new TextEncoder().encode(s).length;
export function validateCompanion(value: unknown, revision: string): Companion {
  if (!value || typeof value !== "object")
    throw new Error("Invalid companion");
  const v = value as Companion;
  if (
    v.format !== "faktory-companion-v1" || v.kit_version !== "1" ||
    v.revision !== revision || !/^[a-zA-Z0-9._-]{1,128}$/.test(revision) ||
    typeof v.html !== "string" || typeof v.css !== "string" ||
    !Array.isArray(v.assets) || v.assets.length > 64 || bytes(JSON.stringify(v)) > MAX_BYTES
  )
    throw new Error("Invalid companion");
  const ids = new Set<string>();
  for (const a of v.assets) {
    if (
      !a || typeof a.id !== "string" || a.id.length > 256 ||
      !/^assets\/[a-zA-Z0-9._/-]+\.(svg|json)$/.test(a.id) ||
      a.id.split("/").some(part => !part || part === "." || part === "..") ||
      ids.has(a.id) || typeof a.content !== "string" ||
      a.mime_type !== (a.id.endsWith(".svg") ? "image/svg+xml" : "application/json")
    )
      throw new Error("Invalid asset");
    if (a.mime_type === "application/json")
      JSON.parse(a.content);
    ids.add(a.id);
  }
  return v;
}
export function safeCss(css: string, fragments = false): string {
  // Inspect decoded syntax, but preserve harmless authored comments and escapes.
  const unescape = (text: string) => text.replace(
    /\\(?:([0-9a-f]{1,6})(?:\r\n|[\t\n\r\f ])?|([^\n\r\f0-9a-f])|\r\n|[\n\r\f])/gi,
    (_, hex: string | undefined, char: string | undefined) => hex
      ? String.fromCodePoint(Math.min(parseInt(hex, 16) || 0xfffd, 0x10ffff))
      : char ?? "",
  );
  const syntax = css.replace(/"(?:\\[\s\S]|[^"\\])*"|'(?:\\[\s\S]|[^'\\])*'|\/\*[\s\S]*?\*\//g, token => {
    if (token.startsWith("/*")) return "";
    const text = unescape(token.slice(1, -1));
    return fragments && /^#[a-zA-Z0-9_.:-]+$/.test(text) ? text : '""';
  });
  let decoded = unescape(syntax);
  if (fragments) decoded = decoded.replace(/url\(\s*#[a-zA-Z0-9_.:-]+\s*\)/gi, "");
  const rules = decoded.replace(/@(media|supports|layer|container|keyframes|-webkit-keyframes|page)\b/gi, "");
  return /url\s*\(|(?:image|image-set)\s*\(|expression\s*\(|-moz-binding|behavior\s*:/i.test(decoded) || rules.includes("@") ? "" : css;
}
const htmlTags = new Set("article section main header footer aside nav div span p h1 h2 h3 h4 h5 h6 ul ol li dl dt dd table caption colgroup col thead tbody tfoot tr th td pre code blockquote q strong em b i u s mark time abbr kbd samp var small sub sup br hr wbr figure figcaption img a details summary button label output fk-tabs fk-callout fk-measurement fk-steps fk-asset fk-cut-layout".split(" "));
const svgTags = new Set("svg g path rect circle ellipse line polyline polygon text tspan title desc defs style clipPath linearGradient radialGradient stop marker mask pattern use symbol".toLowerCase().split(" "));
const attrs = new Set("id class title lang dir role tabindex hidden colspan rowspan scope headers alt width height viewbox d x y x1 x2 y1 y2 cx cy r rx ry points fill stroke stroke-width opacity transform offset stop-color clip-path preserveaspectratio data-companion-asset data-tab label value unit open start reversed datetime".split(" "));
const svgAttrs = new Set("fill-opacity fill-rule stroke-opacity stroke-linecap stroke-linejoin stroke-miterlimit stroke-dasharray stroke-dashoffset font-family font-size font-weight font-style text-anchor dominant-baseline dx dy rotate textlength lengthadjust gradientunits gradienttransform spreadmethod stop-opacity fx fy fr clippathunits maskunits maskcontentunits patternunits patterncontentunits patterntransform markerwidth markerheight refx refy orient markerunits vector-effect mask marker-start marker-mid marker-end".split(" "));
export function sanitize(document: Document, html: string, svgOnly = false): DocumentFragment {
  const template = document.createElement("template");
  template.innerHTML = html;
  let count = 0;
  const walk = (parent: ParentNode, depth = 0) => {
    if (depth > 64)
      throw new Error("Companion is too complex");
    for (const node of Array.from(parent.childNodes)) {
      if (++count > 16384)
        throw new Error("Companion is too complex");
      if (node.nodeType === 8) {
        node.remove();
        continue;
      }
      if (node.nodeType !== 1)
        continue;
      const el = node as Element;
      const tag = el.localName.toLowerCase();
      const isSvg = el.namespaceURI === "http://www.w3.org/2000/svg";
      if (!(isSvg ? svgTags : svgOnly ? new Set<string>() : htmlTags).has(tag)) {
        el.remove();
        continue;
      }
      if (isSvg && tag === "style") {
        el.textContent = safeCss(el.textContent ?? "", true);
      }
      for (const attr of Array.from(el.attributes)) {
        const key = attr.name.toLowerCase();
        if (key === "style") {
          const css = safeCss(attr.value, isSvg);
          if (css)
            el.setAttribute("style", css);
          else
            el.removeAttribute(attr.name);
        }
        else if (key === "href" && (tag === "a" || isSvg) && /^#[a-zA-Z0-9_.:-]+$/.test(attr.value)) {
          el.removeAttributeNS(attr.namespaceURI, attr.localName);
          el.setAttribute("href", attr.value);
        }
        else if (isSvg && ["fill", "stroke", "clip-path", "mask", "marker-start", "marker-mid", "marker-end"].includes(key) && /^url\(#[a-zA-Z0-9_.:-]+\)$/.test(attr.value)) { /* Only same-document SVG references survive. */ }
        else if ((!attrs.has(key) && !(isSvg && svgAttrs.has(key)) && !/^aria-[a-z-]+$/.test(key) && !/^data-[a-z0-9-]+$/.test(key)) || /(?:url\s*\(|javascript:|https?:|data:|\/\/)/i.test(attr.value) || attr.namespaceURI)
          el.removeAttribute(attr.name);
      }
      if (tag === "button")
        el.setAttribute("type", "button");
      walk(el, depth + 1);
    }
  };
  walk(template.content);
  return template.content;
}
export const foundationCss = `
:root { color-scheme: light; font-family: Georgia, serif; color: #24383b; background: #f8f7f2; }
body { margin: 0; padding: 24px; line-height: 1.55; overflow-wrap: anywhere; }
* { box-sizing: border-box; }
img, svg { max-width: 100%; height: auto; }
table { display: block; overflow-x: auto; max-width: 100%; border-collapse: collapse; width: 100%; }
th, td { padding: 8px; border-bottom: 1px solid #cad4d1; text-align: left; }
pre { white-space: pre-wrap; }
button { font: inherit; cursor: pointer; padding: 8px 12px; border: 1px solid #77928b; background: #eef3f0; }
fk-callout, .callout { display: block; border-left: 4px solid #436d62; padding: 12px; background: #eef3f0; }
fk-measurement { display: inline-flex; gap: 8px; padding: 8px; border: 1px solid #cad4d1; }
fk-steps { display: block; counter-reset: step; }
fk-steps > section { counter-increment: step; margin: 16px 0; }
fk-steps > section:before { content: counter(step) '. '; font-weight: bold; }
.grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(240px, 100%), 1fr)); gap: 16px; }
@media print {
  body { padding: 0; }
  button, [role="tablist"] { display: none; }
  fk-tabs > [data-tab][hidden] { display: block; }
  table { display: table; overflow: visible; }
  section, tr { break-inside: avoid; }
}`;
export function installComponents(window: Window & typeof globalThis) {
  if (window.customElements.get("fk-tabs"))
    return;
  class Tabs extends window.HTMLElement {
    connectedCallback() {
      const panels = Array.from(this.children).filter(el => el.hasAttribute("data-tab"));
      const bar = window.document.createElement("div");
      bar.setAttribute("role", "tablist");
      panels.forEach((panel, index) => {
        const button = window.document.createElement("button");
        button.type = "button";
        button.textContent = panel.getAttribute("data-tab");
        button.setAttribute("role", "tab");
        const activate = () => panels.forEach((p, i) => { (p as HTMLElement).hidden = i !== index; bar.children[i]?.setAttribute("aria-selected", String(i === index)); });
        button.addEventListener("click", activate);
        bar.append(button);
        (panel as HTMLElement).hidden = index !== 0;
        button.setAttribute("aria-selected", String(index === 0));
        panel.setAttribute("role", "tabpanel");
      });
      this.prepend(bar);
    }
  }
  window.customElements.define("fk-tabs", Tabs);
  for (const tag of ["fk-callout", "fk-steps", "fk-asset"])
    window.customElements.define(tag, class extends window.HTMLElement {
    });
  window.customElements.define("fk-measurement", class extends window.HTMLElement {
    connectedCallback() { this.textContent = [this.getAttribute("label"), this.getAttribute("value"), this.getAttribute("unit")].filter(Boolean).join(" "); }
  });
}
