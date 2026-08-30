"use client";

import { useSyncExternalStore } from "react";

/**
 * One ticking wall clock, shared by everything that renders an age.
 *
 * A single interval rather than one per component: this console shows dozens of
 * "3s ago" cells at once and each running its own timer is dozens of independent
 * re-render schedules for the same second.
 *
 * The snapshot is `null` on the server and until the first client tick, on
 * purpose. An age computed during server rendering is wrong by however long the
 * response spent in flight, and reconciling it against the browser's clock is a
 * hydration mismatch. Everything time-relative shows an em dash for one frame
 * and the truth thereafter.
 */

const listeners = new Set<() => void>();
let current: number | null = null;
let timer: ReturnType<typeof setInterval> | undefined;

const TICK_MS = 1_000;

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (timer === undefined) {
    current = Date.now();
    timer = setInterval(() => {
      current = Date.now();
      for (const notify of listeners) notify();
    }, TICK_MS);
    // The first subscriber has a snapshot of `null`; publish immediately so it
    // does not wait a whole second for a clock that is already known.
    queueMicrotask(() => {
      for (const notify of listeners) notify();
    });
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && timer !== undefined) {
      clearInterval(timer);
      timer = undefined;
      current = null;
    }
  };
}

function getSnapshot(): number | null {
  return current;
}

function getServerSnapshot(): number | null {
  return null;
}

export function useNow(): number | null {
  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot);
}
