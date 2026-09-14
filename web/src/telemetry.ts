import {
  getWebInstrumentations,
  initializeFaro,
  type BrowserConfig,
  type Faro,
} from "@grafana/faro-web-sdk";
import { TracingInstrumentation } from "@grafana/faro-web-tracing";

const COLLECTOR_URL = "https://faro.holdenitdown.net/collect";

type TelemetryApi = Pick<Faro["api"], "pushEvent" | "setPage" | "setView">;
type FaroInitializer = (config: BrowserConfig) => Faro;

let initializationAttempted = false;
let telemetryApi: TelemetryApi | undefined;
let lastView: string | undefined;

export function initializeTelemetry(options: {
  enabled?: boolean;
  version?: string;
  pathname?: string;
  initialize?: FaroInitializer;
} = {}): void {
  if (initializationAttempted || !(options.enabled ?? __FAKTORY_TELEMETRY_ENABLED__)) return;
  initializationAttempted = true;

  try {
    const faro = (options.initialize ?? initializeFaro)(createFaroConfig({
      version: options.version ?? __FAKTORY_BUILD_VERSION__,
      pathname: options.pathname ?? window.location.pathname,
    }));
    telemetryApi = faro.api;
  } catch {
    telemetryApi = undefined;
  }
}

export function createFaroConfig(options: {
  version: string;
  pathname?: string;
}): BrowserConfig {
  const initialView = normalizeApplicationView(options.pathname ?? "/");

  return {
    url: COLLECTOR_URL,
    app: {
      name: "faktory-spa",
      version: boundedBuildValue(options.version),
      environment: "production",
    },
    instrumentations: [
      ...getWebInstrumentations(),
      new TracingInstrumentation(),
    ],
    experimental: { trackNavigation: true },
    pageTracking: {
      page: { id: initialView, url: initialView },
      generatePageId: (location) => normalizeApplicationView(location.pathname),
    },
    view: { name: initialView },
  };
}

export function reportApplicationView(pathname: string): void {
  const view = normalizeApplicationView(pathname);
  if (!telemetryApi || view === lastView) return;
  lastView = view;
  try {
    telemetryApi.setPage({ id: view, url: view });
    telemetryApi.setView({ name: view });
  } catch {
    // Telemetry must not participate in routing or rendering.
  }
}

export function reportApplicationError(
  category: "startup" | "dispatch" | "recovery",
  outcome: string,
): void {
  if (!telemetryApi) return;
  try {
    telemetryApi.pushEvent("faktory.application_error", {
      category,
      outcome: controlledOutcome(outcome),
    });
  } catch {
    // Reporting failures are deliberately ignored by product logic.
  }
}

export function normalizeApplicationView(pathname: string): string {
  const path = applicationPathname(pathname).split(/[?#]/, 1)[0].replace(/\/{2,}/g, "/");
  if (path === "/") return path;
  if (/^\/models\/[^/]+\/?$/.test(path)) return "/models/:id";
  return "/unknown";
}

export function resetTelemetryForTests(): void {
  initializationAttempted = false;
  telemetryApi = undefined;
  lastView = undefined;
}

function boundedBuildValue(value: string): string {
  const bounded = value.trim().slice(0, 64);
  return /^[0-9A-Za-z._+-]+$/.test(bounded) ? bounded : "unknown";
}

function applicationPathname(value: string): string {
  try {
    return new URL(value, "https://faktory.invalid").pathname;
  } catch {
    return "/unknown";
  }
}

function controlledOutcome(value: string): string {
  const bounded = value.slice(0, 32);
  return /^[a-z][a-z0-9_]*$/.test(bounded) ? bounded : "unknown";
}
