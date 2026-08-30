"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ApiOutcome, ApiResponse } from "@/lib/api/client";
import { connections, type FeedState } from "./connections";

/**
 * Poll one REST endpoint and account for the age of what came back.
 *
 * A dashboard's real failure is not an error banner, it is a number that
 * stopped changing an hour ago and still looks like a number. So the hook
 * distinguishes four things a caller must render differently: nothing has
 * arrived yet, something arrived and is current, something arrived and is now
 * older than the panel's freshness bound, and the last attempt failed — in
 * which case the previous answer is still shown, and shown as stale.
 */

export interface UseResourceOptions {
  /** Stable identity for the connection register. */
  readonly key: string;
  readonly label: string;
  /** Poll period. Omit for a single fetch on mount. */
  readonly intervalMs?: number;
  readonly enabled?: boolean;
  /** Age past which the last good answer is marked stale. */
  readonly staleAfterMs?: number;
}

export interface Resource<D> {
  /** True until the first answer of any kind has landed. */
  readonly loading: boolean;
  /** True while a refresh is in flight over an answer already shown. */
  readonly refreshing: boolean;
  readonly outcome: ApiOutcome<D> | null;
  readonly data: D | null;
  readonly receivedAt: number | null;
  readonly latencyMs: number | null;
  readonly stale: boolean;
  /** Consecutive failed attempts since the last answer. */
  readonly attempts: number;
  refresh(): void;
}

export function useResource<D>(
  fetcher: (signal: AbortSignal) => Promise<ApiResponse<D>>,
  options: UseResourceOptions,
): Resource<D> {
  const { key, label, intervalMs, enabled = true, staleAfterMs } = options;
  const staleBound = staleAfterMs ?? (intervalMs === undefined ? 60_000 : intervalMs * 3);

  const [outcome, setOutcome] = useState<ApiOutcome<D> | null>(null);
  const [receivedAt, setReceivedAt] = useState<number | null>(null);
  const [latencyMs, setLatencyMs] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [stale, setStale] = useState(false);
  const [attempts, setAttempts] = useState(0);
  const [generation, setGeneration] = useState(0);

  // The fetcher is often written inline at the call site. Held in a ref so a
  // new closure on every render does not restart the poll on every render.
  const fetcherRef = useRef(fetcher);
  useEffect(() => {
    fetcherRef.current = fetcher;
  }, [fetcher]);

  const refresh = useCallback(() => {
    setGeneration((value) => value + 1);
  }, []);

  const feedId = `poll:${key}`;

  useEffect(() => {
    connections.register(feedId, label, "poll", refresh);
    return () => connections.deregister(feedId);
  }, [feedId, label, refresh]);

  useEffect(() => {
    if (!enabled) return;

    const controller = new AbortController();
    let cancelled = false;
    let poll: ReturnType<typeof setInterval> | undefined;
    let staleTimer: ReturnType<typeof setTimeout> | undefined;

    const run = async () => {
      setRefreshing(true);
      let response: ApiResponse<D>;
      try {
        response = await fetcherRef.current(controller.signal);
      } catch (cause) {
        if (cancelled) return;
        setAttempts((value) => value + 1);
        setRefreshing(false);
        setLoading(false);
        setOutcome({
          kind: "error",
          endpoint: key,
          status: null,
          detail: cause instanceof Error ? cause.message : "the request failed",
        });
        return;
      }
      if (cancelled) return;

      setOutcome(response.outcome);
      setRefreshing(false);
      setLoading(false);
      setLatencyMs(response.latencyMs);

      if (response.outcome.kind === "unreachable" || response.outcome.kind === "error") {
        // Keep the previous timestamp: what is on screen is as old as it was.
        setAttempts((value) => value + 1);
        setStale(true);
        return;
      }

      setAttempts(0);
      setReceivedAt(response.receivedAt);
      setStale(false);
      if (staleTimer !== undefined) clearTimeout(staleTimer);
      staleTimer = setTimeout(() => {
        if (!cancelled) setStale(true);
      }, staleBound);
    };

    void run();
    if (intervalMs !== undefined && intervalMs > 0) {
      poll = setInterval(() => void run(), intervalMs);
    }

    return () => {
      cancelled = true;
      controller.abort();
      if (poll !== undefined) clearInterval(poll);
      if (staleTimer !== undefined) clearTimeout(staleTimer);
    };
  }, [enabled, intervalMs, generation, key, staleBound]);

  const feedState: FeedState = useMemo(() => {
    if (!enabled) return "closed";
    if (loading) return "connecting";
    if (outcome === null) return "idle";
    if (outcome.kind === "unreachable") return "reconnecting";
    if (outcome.kind === "error") return "error";
    if (stale) return "stale";
    return "open";
  }, [enabled, loading, outcome, stale]);

  useEffect(() => {
    connections.update(feedId, {
      state: feedState,
      lastEventAt: receivedAt,
      attempts,
      detail: outcome && outcome.kind !== "ok" ? outcome.kind : null,
    });
  }, [feedId, feedState, receivedAt, attempts, outcome]);

  return useMemo<Resource<D>>(
    () => ({
      loading,
      refreshing,
      outcome,
      data: outcome !== null && outcome.kind === "ok" ? outcome.data : null,
      receivedAt,
      latencyMs,
      stale,
      attempts,
      refresh,
    }),
    [loading, refreshing, outcome, receivedAt, latencyMs, stale, attempts, refresh],
  );
}
