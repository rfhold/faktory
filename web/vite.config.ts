import { readFileSync } from "node:fs";
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

import { telemetryEnabledForBuild } from "./src/build";

const packageVersion = (JSON.parse(
  readFileSync(new URL("./package.json", import.meta.url), "utf8"),
) as { version: string }).version;

export default defineConfig(({ command, mode }) => ({
  plugins: [solid()],
  define: {
    __FAKTORY_BUILD_VERSION__: JSON.stringify(packageVersion),
    __FAKTORY_TELEMETRY_ENABLED__: JSON.stringify(telemetryEnabledForBuild(command, mode)),
  },
  build: {
    target: "es2022",
  },
  test: {
    environment: "happy-dom",
  },
}));
