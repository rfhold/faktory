import { describe, expect, it } from "vitest";
import {
  initializeDefaultView,
  skipDefaultViewInitialization,
  type DefaultViewInitialization,
} from "./defaultView";

const views = [{ id: "front" }, { id: "side" }];

describe("default view initialization", () => {
  it("waits for geometry, applies once, and does not overwrite later camera work", () => {
    let state: DefaultViewInitialization | undefined;

    let result = initializeDefaultView(state, "model-a:revision-1", false, "front", views);
    expect(result.view).toBeUndefined();

    result = initializeDefaultView(result.state, "model-a:revision-1", true, "front", views);
    expect(result.view).toBe(views[0]);

    result = initializeDefaultView(result.state, "model-a:revision-1", true, "side", views);
    expect(result.view).toBeUndefined();
  });

  it("preserves an explicit choice and resets for a different model load", () => {
    const state = skipDefaultViewInitialization("model-a:revision-1");
    const skipped = initializeDefaultView(state, "model-a:revision-1", true, "front", views);
    expect(skipped.view).toBeUndefined();

    const reset = initializeDefaultView(skipped.state, "model-b:revision-2", true, "side", views);
    expect(reset.view).toBe(views[1]);
  });
});
