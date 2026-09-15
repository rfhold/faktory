import { MAX_PENDING_RENDERS } from "./constants.js";
import { ServiceError } from "./errors.js";

interface Pending<T> {
  signal: AbortSignal;
  work: (signal: AbortSignal) => Promise<T>;
  resolve: (value: T) => void;
  reject: (reason: unknown) => void;
  cancelPending: () => void;
}

function abortReason(signal: AbortSignal): unknown {
  return signal.reason ?? new ServiceError(499, "request_cancelled");
}

export class RenderQueue {
  private active = false;
  private readonly pending: Pending<unknown>[] = [];

  constructor(private readonly maxPending = MAX_PENDING_RENDERS) {}

  get pendingCount(): number {
    return this.pending.length;
  }

  run<T>(signal: AbortSignal, work: (signal: AbortSignal) => Promise<T>): Promise<T> {
    if (signal.aborted) return Promise.reject(abortReason(signal));
    if (this.active && this.pending.length >= this.maxPending) {
      return Promise.reject(new ServiceError(503, "render_queue_saturated"));
    }
    return new Promise<T>((resolve, reject) => {
      const entry = { signal, work, resolve, reject } as Pending<unknown>;
      entry.cancelPending = () => {
        const index = this.pending.indexOf(entry);
        if (index < 0) return;
        this.pending.splice(index, 1);
        reject(abortReason(signal));
      };
      signal.addEventListener("abort", entry.cancelPending, { once: true });
      this.pending.push(entry);
      this.pump();
    });
  }

  private pump(): void {
    if (this.active) return;
    const next = this.pending.shift();
    if (!next) return;
    next.signal.removeEventListener("abort", next.cancelPending);
    if (next.signal.aborted) {
      next.reject(abortReason(next.signal));
      this.pump();
      return;
    }
    this.active = true;
    const work = Promise.resolve().then(() => {
      if (next.signal.aborted) throw abortReason(next.signal);
      return next.work(next.signal);
    });
    void work.then(next.resolve, next.reject).finally(() => {
      this.active = false;
      this.pump();
    });
  }
}
