import { describe, expect, it, vi } from "vitest";
import { RenderQueue } from "../src/queue.js";

describe("single renderer queue", () => {
  it("runs exactly one job and rejects beyond its pending bound", async () => {
    const queue = new RenderQueue(2);
    const releases: Array<() => void> = [];
    let running = 0;
    let maximum = 0;
    const work = () => queue.run(new AbortController().signal, async () => {
      running += 1;
      maximum = Math.max(maximum, running);
      await new Promise<void>((resolve) => releases.push(resolve));
      running -= 1;
      return "done";
    });
    const first = work();
    const second = work();
    const third = work();
    await expect(work()).rejects.toThrow("render_queue_saturated");
    releases.shift()!();
    await first;
    await vi.waitFor(() => expect(releases).toHaveLength(1));
    releases.shift()!();
    await second;
    await vi.waitFor(() => expect(releases).toHaveLength(1));
    releases.shift()!();
    await third;
    expect(maximum).toBe(1);
  });

  it("removes an aborted pending job and frees capacity immediately", async () => {
    const queue = new RenderQueue(1);
    let release!: () => void;
    const active = queue.run(new AbortController().signal, () => new Promise<void>((resolve) => {
      release = resolve;
    }));
    const pendingController = new AbortController();
    const pending = queue.run(pendingController.signal, async () => "pending");
    await expect(queue.run(new AbortController().signal, async () => "saturated"))
      .rejects.toThrow("render_queue_saturated");

    pendingController.abort(new Error("cancelled"));
    await expect(pending).rejects.toThrow("cancelled");
    expect(queue.pendingCount).toBe(0);
    const replacement = queue.run(new AbortController().signal, async () => "replacement");
    release();
    await active;
    await expect(replacement).resolves.toBe("replacement");
  });

  it("cancels active work, runs cleanup, and releases the next job", async () => {
    const queue = new RenderQueue(1);
    const activeController = new AbortController();
    let cleaned = false;
    let nextStarted = false;
    const active = queue.run(activeController.signal, async (signal) => {
      return new Promise<string>((_resolve, reject) => {
        signal.addEventListener("abort", () => {
          cleaned = true;
          reject(signal.reason);
        }, { once: true });
      });
    });
    const next = queue.run(new AbortController().signal, async () => {
      nextStarted = true;
      return "next";
    });

    await vi.waitFor(() => expect(queue.pendingCount).toBe(1));
    activeController.abort(new Error("active_cancelled"));
    await expect(active).rejects.toThrow("active_cancelled");
    await expect(next).resolves.toBe("next");
    expect(cleaned).toBe(true);
    expect(nextStarted).toBe(true);
  });
});
