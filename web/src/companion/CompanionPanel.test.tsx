import { QueryClient, QueryClientProvider } from "@tanstack/solid-query";
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
const mocks = vi.hoisted(() => ({ config: vi.fn(), fetch: vi.fn(), mount: vi.fn(), destroy: vi.fn(), print: vi.fn() }));
vi.mock("./protocol", async (importOriginal) => ({ ...await importOriginal<typeof import("./protocol")>(), fetchCompanionOrigin: mocks.config, fetchCompanion: mocks.fetch, mountCompanion: mocks.mount }));
import { CompanionPanel } from "./CompanionPanel";
const disposers: (() => void)[] = [];
const browserSettings = (window as unknown as {
  happyDOM: {
    settings: {
      disableIframePageLoading: boolean;
    };
  };
}).happyDOM.settings;
beforeEach(() => { browserSettings.disableIframePageLoading = true; mocks.config.mockResolvedValue("https://guide.test"); });
afterEach(() => { disposers.splice(0).forEach(dispose => dispose()); document.body.replaceChildren(); browserSettings.disableIframePageLoading = false; vi.unstubAllEnvs(); vi.clearAllMocks(); });
const companion = (revision: string) => ({ format: "faktory-companion-v1", kit_version: "1", revision, html: "<p>Reference</p>", css: "", assets: [] });
function setup() {
  const [revision, setRevision] = createSignal("rev-1");
  const [stale, setStale] = createSignal(false);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const host = document.createElement("div");
  document.body.append(host);
  disposers.push(render(() => <QueryClientProvider client={client}><CompanionPanel modelId="fixture" revision={revision()} stale={stale()} /></QueryClientProvider>, host));
  return { host, setRevision, setStale };
}
describe("companion model UI", () => {
  it("fails closed with disabled runtime configuration despite a build origin", async () => {
    vi.stubEnv("VITE_COMPANION_ORIGIN", "https://ignored.test");
    mocks.config.mockResolvedValue(null);
    const { host } = setup();
    await vi.waitFor(() => expect(host.textContent).toContain("not configured"));
    expect(host.querySelector("iframe")).toBeNull();
    expect(mocks.fetch).not.toHaveBeenCalled();
    expect(host.textContent).toContain("not configured");
  });
  it("mounts successful-revision documents and destroys sessions on watch update/unmount", async () => {
    vi.stubEnv("VITE_COMPANION_ORIGIN", "https://guide.test");
    mocks.fetch.mockImplementation(async (_model: string, rev: string) => companion(rev));
    mocks.mount.mockImplementation((_frame, _companion, onState) => { onState("ready"); return { destroy: mocks.destroy, print: mocks.print }; });
    const { host, setRevision, setStale } = setup();
    await vi.waitFor(() => expect(host.querySelector("iframe")).not.toBeNull());
    const first = host.querySelector("iframe")!;
    expect(first.getAttribute("sandbox")).toBe("allow-scripts allow-modals");
    expect(first.hasAttribute("credentialless")).toBe(true);
    expect(first.src).toBe("https://guide.test/frame");
    setStale(true);
    expect(host.textContent).toContain("last successful revision");
    expect(host.querySelector("iframe")).toBe(first);
    first.dispatchEvent(new Event("load"));
    host.querySelector<HTMLButtonElement>("button")!.click();
    expect(mocks.print).toHaveBeenCalledOnce();
    setRevision("rev-2");
    await vi.waitFor(() => expect(mocks.fetch).toHaveBeenCalledWith("fixture", "rev-2", expect.any(AbortSignal)));
    await vi.waitFor(() => { expect(host.querySelector("iframe")).not.toBeNull(); expect(host.querySelector("iframe")).not.toBe(first); });
    expect(mocks.destroy).toHaveBeenCalled();
    host.querySelector("iframe")!.dispatchEvent(new Event("load"));
    expect(mocks.mount.mock.calls.at(-1)?.[1].revision).toBe("rev-2");
    disposers.pop()!();
    expect(mocks.destroy.mock.calls.length).toBeGreaterThanOrEqual(2);
  });
  it("fails closed when runtime configuration cannot be fetched", async () => {
    mocks.config.mockRejectedValue(new Error("401"));
    const { host } = setup();
    await vi.waitFor(() => expect(host.textContent).toContain("not configured"));
    expect(host.querySelector("iframe")).toBeNull();
    expect(mocks.fetch).not.toHaveBeenCalled();
  });
  it("shows absent and invalid documents without a frame", async () => {
    vi.stubEnv("VITE_COMPANION_ORIGIN", "https://guide.test");
    mocks.fetch.mockResolvedValueOnce(null).mockRejectedValueOnce(new Error("422"));
    const { host, setRevision } = setup();
    await vi.waitFor(() => expect(host.textContent).toContain("No companion"));
    expect(host.querySelector("iframe")).toBeNull();
    setRevision("rev-2");
    await vi.waitFor(() => expect(host.textContent).toContain("Companion unavailable"));
    expect(host.querySelector("iframe")).toBeNull();
  });
});
