export const MAX_GLB_BYTES = 64 * 1024 * 1024;
export const MAX_SPEC_BYTES = 16 * 1024;
export const MAX_PNG_BYTES = 2 * 1024 * 1024;
export const MAX_RESPONSE_BYTES = 24 * 1024 * 1024;
export const MAX_PENDING_RENDERS = 8;
export const RENDER_DEADLINE_MS = 25_000;
export const CONTEXT_CLOSE_GRACE_MS = 1_000;
export const RENDER_WIDTH = 640;
export const RENDER_HEIGHT = 480;
export const RECIPE = "three-v2" as const;
export const CANONICAL_NAMES = [
  "isometric", "front", "back", "left", "right", "top", "bottom",
] as const;
