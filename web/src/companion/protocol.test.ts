import { afterEach, describe, expect, it, vi } from "vitest";
import { companionOrigin, fetchCompanion, fetchCompanionOrigin } from "./protocol";
afterEach(() => vi.unstubAllGlobals());
describe("companion parent boundary", () => {
  it("uses bounded authenticated runtime config with no build fallback", async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify({ companion_origin: "https://guide.test" }), { headers: { "content-type": "application/json" } }));
    vi.stubGlobal("fetch", fetcher);
    expect(await fetchCompanionOrigin()).toBe("https://guide.test");
    expect(fetcher).toHaveBeenCalledWith("/runtime-config", expect.objectContaining({ credentials: "same-origin", cache: "no-store", redirect: "error" }));
    for (const config of [{ companion_origin: null }, { companion_origin: "https://guide.test/frame" }, { companion_origin: window.location.origin }]) {
      fetcher.mockResolvedValueOnce(new Response(JSON.stringify(config), { headers: { "content-type": "application/json" } }));
      expect(await fetchCompanionOrigin()).toBeNull();
    }
    for (const body of ["{}", 'x'.repeat(4097), JSON.stringify({ companion_origin: 123 })]) {
      fetcher.mockResolvedValueOnce(new Response(body, { headers: { "content-type": "application/json" } }));
      await expect(fetchCompanionOrigin()).rejects.toThrow();
    }
    fetcher.mockResolvedValueOnce(new Response(null, { status: 401 }));
    await expect(fetchCompanionOrigin()).rejects.toThrow();
  });
  it("requires a distinct exact configured origin", () => {
    expect(companionOrigin(undefined, "https://model.test")).toBeUndefined();
    expect(companionOrigin("https://model.test", "https://model.test")).toBeUndefined();
    expect(companionOrigin("https://guide.test/frame", "https://model.test")).toBeUndefined();
    expect(companionOrigin("https://guide.test", "https://model.test")).toBe("https://guide.test");
  });
  it("fetches only the authenticated same-origin successful revision route", async () => {
    const payload = { format: "faktory-companion-v1", kit_version: "1", revision: "rev-1", html: "<p>Guide</p>", css: "", assets: [] };
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify(payload), { headers: { "content-type": "application/json" } }));
    vi.stubGlobal("fetch", fetcher);
    expect(await fetchCompanion("bench", "rev-1")).toEqual(payload);
    expect(fetcher.mock.calls[0][0]).toBe("/artifacts/bench/rev-1/companion");
    expect(fetcher.mock.calls[0][1].credentials).toBe("same-origin");
  });
  it("handles absent safely and rejects invalid/oversized documents", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(null, { status: 404 })));
    expect(await fetchCompanion("x", "rev")).toBeNull();
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response("x".repeat(1048577), { headers: { "content-type": "application/json" } })));
    await expect(fetchCompanion("x", "rev")).rejects.toThrow("size limit");
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response("{}", { headers: { "content-type": "application/json" } })));
    await expect(fetchCompanion("x", "rev")).rejects.toThrow();
  });
});
