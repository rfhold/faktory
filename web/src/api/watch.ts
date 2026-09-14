import type { QueryClient } from "@tanstack/solid-query";
import type { FaktoryClient } from "./client";
import { replaceModels, upsertModel } from "./queries";
import { reportApplicationError } from "../telemetry";

const MAX_BACKOFF_MS = 15_000;
let activeWatch: { references: number; controller: AbortController } | undefined;

function wait(milliseconds: number, signal: AbortSignal) {
  return new Promise<void>((resolve) => {
    const timeout = window.setTimeout(resolve, milliseconds);
    signal.addEventListener(
      "abort",
      () => {
        window.clearTimeout(timeout);
        resolve();
      },
      { once: true },
    );
  });
}

export async function consumeModelWatch(
  client: FaktoryClient,
  queryClient: QueryClient,
  signal: AbortSignal,
) {
  let failures = 0;
  while (!signal.aborted) {
    try {
      let receivedSnapshot = false;
      for await (const message of client.watchModels({}, { signal })) {
        if (!receivedSnapshot) {
          if (message.event.case !== "initialSnapshot") {
            throw new Error("WatchModels did not begin with an initial snapshot");
          }
          await Promise.all([
            queryClient.cancelQueries({ queryKey: ["models"], exact: true }),
            ...message.event.value.models.map((model) =>
              queryClient.cancelQueries({ queryKey: ["models", model.id], exact: true }),
            ),
          ]);
          replaceModels(queryClient, message.event.value.models);
          receivedSnapshot = true;
          failures = 0;
          continue;
        }

        if (message.event.case !== "modelChanged" || !message.event.value.model) {
          throw new Error("WatchModels returned an invalid change event");
        }
        await Promise.all([
          queryClient.cancelQueries({ queryKey: ["models"], exact: true }),
          queryClient.cancelQueries({
            queryKey: ["models", message.event.value.model.id],
            exact: true,
          }),
        ]);
        upsertModel(queryClient, message.event.value.model);
      }
      if (!signal.aborted) throw new Error("WatchModels stream ended");
    } catch {
      if (signal.aborted) return;
      reportApplicationError("recovery", "watch_failed");
      const delay = Math.min(1_000 * 2 ** failures, MAX_BACKOFF_MS);
      failures += 1;
      await wait(delay, signal);
    }
  }
}

export function startModelWatch(client: FaktoryClient, queryClient: QueryClient) {
  if (activeWatch) {
    activeWatch.references += 1;
  } else {
    const controller = new AbortController();
    activeWatch = { references: 1, controller };
    void consumeModelWatch(client, queryClient, controller.signal);
  }

  return () => {
    if (!activeWatch) return;
    activeWatch.references -= 1;
    if (activeWatch.references === 0) {
      activeWatch.controller.abort();
      activeWatch = undefined;
    }
  };
}
