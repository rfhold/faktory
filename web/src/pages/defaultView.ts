export interface DefaultViewInitialization {
  loadKey: string;
  complete: boolean;
}

export function skipDefaultViewInitialization(loadKey: string): DefaultViewInitialization {
  return { loadKey, complete: true };
}

export function initializeDefaultView<T extends { id: string }>(
  state: DefaultViewInitialization | undefined,
  loadKey: string,
  geometryReady: boolean,
  defaultViewId: string,
  views: readonly T[] | undefined,
): { state: DefaultViewInitialization; view?: T } {
  const current = state?.loadKey === loadKey ? state : { loadKey, complete: false };
  if (current.complete || !geometryReady || !views) return { state: current };

  return {
    state: { loadKey, complete: true },
    view: views.find((view) => view.id === defaultViewId),
  };
}
