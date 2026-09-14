import { beforeEach, describe, expect, it, vi } from "vitest";
import type { BrowserConfig } from "@grafana/faro-web-sdk";

import { telemetryEnabledForBuild } from "./build";
import {
  createFaroConfig,
  initializeTelemetry,
  normalizeApplicationView,
  reportApplicationError,
  reportApplicationView,
  resetTelemetryForTests,
} from "./telemetry";

describe("telemetry", () => {
  beforeEach(() => resetTelemetryForTests());

  it("enables telemetry only for production builds", () => {
    expect([
      ["build", "production", telemetryEnabledForBuild("build", "production")],
      ["build", "development", telemetryEnabledForBuild("build", "development")],
      ["build", "test", telemetryEnabledForBuild("build", "test")],
      ["build", "custom-preview", telemetryEnabledForBuild("build", "custom-preview")],
      ["serve", "production", telemetryEnabledForBuild("serve", "production")],
    ]).toEqual([
      ["build", "production", true],
      ["build", "development", false],
      ["build", "test", false],
      ["build", "custom-preview", false],
      ["serve", "production", false],
    ]);
  });

  it("uses the fixed Faktory identity and package version metadata", () => {
    const config = createFaroConfig({ version: "0.1.0", pathname: "/" });
    expect(config.url).toBe("https://faro.holdenitdown.net/collect");
    expect(config.app).toEqual({
      name: "faktory-spa",
      version: "0.1.0",
      environment: "production",
    });
  });

  it("normalizes application views and collapses unknown paths", () => {
    expect(normalizeApplicationView("/")).toBe("/");
    expect(normalizeApplicationView("/models/private-model?token=secret#part")).toBe("/models/:id");
    expect(normalizeApplicationView("/unexpected/user-content")).toBe("/unknown");
  });

  it("does not initialize or send when telemetry is disabled", () => {
    const initialize = vi.fn();
    const fetchSpy = vi.spyOn(globalThis, "fetch");
    initializeTelemetry({ enabled: false, initialize });
    reportApplicationError("startup", "load_failed");
    expect(initialize).not.toHaveBeenCalled();
    expect(fetchSpy).not.toHaveBeenCalled();
    fetchSpy.mockRestore();
  });

  it("initializes once and fails open", () => {
    const initialize = vi.fn(() => {
      throw new Error("collector unavailable");
    });
    expect(() => initializeTelemetry({ enabled: true, version: "0.1.0", pathname: "/", initialize })).not.toThrow();
    initializeTelemetry({ enabled: true, version: "other", pathname: "/models/private", initialize });
    expect(initialize).toHaveBeenCalledOnce();
  });

  it("uses standard Faro instrumentation without a beforeSend sanitizer", () => {
    const config = createFaroConfig({ version: "0.1.0", pathname: "/" });
    expect(config.beforeSend).toBeUndefined();
    expect(config.ignoreUrls).toBeUndefined();
    expect(config.experimental?.trackNavigation).toBe(true);
    expect(config.instrumentations?.map(({ name }) => name)).toEqual(expect.arrayContaining([
      "@grafana/faro-web-sdk:instrumentation-console",
      "@grafana/faro-web-sdk:instrumentation-navigation",
      "@grafana/faro-web-sdk:instrumentation-session",
      "@grafana/faro-web-sdk:instrumentation-user-action",
      "@grafana/faro-web-tracing",
    ]));
  });

  it("reports each normalized view at most once in succession", () => {
    const setPage = vi.fn();
    const setView = vi.fn();
    const initialize = vi.fn((_config: BrowserConfig) => ({
      api: { pushEvent: vi.fn(), setPage, setView },
    }));
    initializeTelemetry({ enabled: true, version: "0.1.0", pathname: "/", initialize: initialize as never });
    reportApplicationView("/models/private-one");
    reportApplicationView("/models/private-two");
    reportApplicationView("/private/unknown");
    reportApplicationView("/another/unknown");
    expect(setPage.mock.calls).toEqual([
      [{ id: "/models/:id", url: "/models/:id" }],
      [{ id: "/unknown", url: "/unknown" }],
    ]);
    expect(setView.mock.calls).toEqual([
      [{ name: "/models/:id" }],
      [{ name: "/unknown" }],
    ]);
  });

  it("reports repeated errors with bounded controlled fields only", () => {
    const pushEvent = vi.fn();
    const initialize = vi.fn((_config: BrowserConfig) => ({
      api: { pushEvent, setPage: vi.fn(), setView: vi.fn() },
    }));
    initializeTelemetry({ enabled: true, version: "0.1.0", pathname: "/", initialize: initialize as never });
    reportApplicationError("recovery", "watch_failed");
    reportApplicationError("recovery", "private error string from server");
    expect(pushEvent.mock.calls).toEqual([
      ["faktory.application_error", { category: "recovery", outcome: "watch_failed" }],
      ["faktory.application_error", { category: "recovery", outcome: "unknown" }],
    ]);
  });
});
