import { Route, Router, useLocation } from "@solidjs/router";
import { QueryClient, QueryClientProvider } from "@tanstack/solid-query";
import { createEffect, lazy, onCleanup, onMount, type ParentProps } from "solid-js";
import { faktoryClient } from "./api/client";
import { startModelWatch } from "./api/watch";
import { CatalogPage } from "./pages/CatalogPage";
import { reportApplicationView } from "./telemetry";

const ModelPage = lazy(() =>
  import("./pages/ModelPage").then((module) => ({ default: module.ModelPage })),
);

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { retry: 1, refetchOnWindowFocus: false },
    mutations: { retry: false },
  },
});

function WatchModels() {
  onMount(() => {
    const stop = startModelWatch(faktoryClient, queryClient);
    onCleanup(stop);
  });
  return null;
}

function TelemetryRoot(props: ParentProps) {
  const location = useLocation();
  createEffect(() => reportApplicationView(location.pathname));
  return props.children;
}

export function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <WatchModels />
      <Router root={TelemetryRoot}>
        <Route path="/" component={CatalogPage} />
        <Route path="/models/:id" component={ModelPage} />
      </Router>
    </QueryClientProvider>
  );
}
