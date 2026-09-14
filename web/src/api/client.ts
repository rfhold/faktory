import { createClient } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import { FaktoryService } from "../../../proto/gen/ts/faktory/v1/faktory_pb";

const transport = createGrpcWebTransport({
  baseUrl: window.location.origin,
  fetch: (input, init) => globalThis.fetch(input, { ...init, credentials: "same-origin" }),
});

export const faktoryClient = createClient(FaktoryService, transport);
export type FaktoryClient = typeof faktoryClient;
