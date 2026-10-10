import { describe, expect, it } from "vitest";
import { safeCss, validateCompanion } from "@rfhold/faktory-companion-kit";
const companion = { format: "faktory-companion-v1", kit_version: "1", revision: "rev", html: "<h1>Guide</h1>", css: "", assets: [] };
describe("runtime input boundary", () => {
  it("rejects bad envelopes, revisions, asset paths and resource syntax", () => {
    expect(validateCompanion(companion, "rev")).toEqual(companion);
    for (const patch of [{ kit_version: "2" }, { revision: "other" }, { assets: [{ id: "assets/../x.svg", mime_type: "image/svg+xml", content: "" }] }, { assets: Array(65).fill({}) }, { html: "x".repeat(1048576) }])
      expect(() => validateCompanion({ ...companion, ...patch }, "rev")).toThrow();
    for (const css of ['@import "https://example.test";', 'p{background:url(https://example.test)}', 'p{background:u\\72l(x)}', 'p{background:image-set("https://example.test")}'])
      expect(safeCss(css)).toBe("");
    expect(safeCss("p{color:red;display:grid}")).toContain("display:grid");
    expect(safeCss("@media(max-width:600px){p{color:red}}@page{margin:12mm}")).toContain("@media");
    expect(safeCss('rect{fill:url("#local")} /* svg */', true)).toContain("#local");
    expect(safeCss('rect{fill:url("https://example.test/x")}', true)).toBe("");
    expect(safeCss('p:before{content:"contact@example.test; url(x) /* note */"}')).toContain("contact@");
    for (const css of ['/* authored note */p{color:r\\65 d;display:grid}', 'p{width:calc(100% - 2rem)}'])
      expect(safeCss(css)).toBe(css);
    for (const css of ['p{background:u/**/rl(x)}', '@\\69 mport "x";', 'p{background:\\75\\72\\6c (x)}'])
      expect(safeCss(css)).toBe("");
  });
});
