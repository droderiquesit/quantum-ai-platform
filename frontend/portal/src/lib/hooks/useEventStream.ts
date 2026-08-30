"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { StreamChannel } from "@/lib/api/endpoints";
import { decodeEnvelope, type StreamEnvelope } from "@/lib/sse/envelope";
import { SseParser, type SseFrame } from "@/lib/sse/parser";
import { connections, type FeedState } from "./connections";

/**
 * Subscribe to one server-sent event channel.
 *
 * The hook owns the whole lifecycle an operator cares about: connect, resume
 * from the last cursor after any drop, back off exponentially so a restarting
 * platform is not stampeded, declare itself stale when even the heartbeat has
 * stopped, and stop and start on command. Every one of those states is
 * reported, because a feed that has silently died looks exactly like a market
 * that has stopped moving, and only one of those is an emergency.
 *
 * Three details follow the platform's contract rather than the SSE default:
 *
 * * De-duplication is by `cursor`, never by `sequence`. Sequence restarts at
 *   one on every connection; cursor is the log position and is stable.
 * * A gap is detected from `sequence`, which is contiguous per connection. A
 *   gap means this reader lost events and should reconcile over REST.
 * * The platform heartbeats with SSE comments every ten seconds, so liveness
 *   is measured from the last byte received, not the last event. A quiet
 *   market and a dead socket are different conditions.
 */

export interface UseEventStreamOptions {
  readonly channel: StreamChannel;
  /** How the feed names itself in the connection register. */
  readonly label: string;
  readonly enabled?: boolean;
  /** How many events to retain for display. Oldest are dropped. */
  readonly maxEvents?: number;
  /**
   * Silence — including heartbeats — longer than this marks the feed stale.
   * The platform heartbeats every 10s, so the default allows two to be lost.
   */
  readonly staleAfterMs?: number;
}

export interface StreamGap {
  readonly expected: number;
  readonly received: number;
  readonly at: number;
}

export interface EventStream {
  readonly state: FeedState;
  /** Newest first. */
  readonly events: readonly StreamEnvelope[];
  readonly received: number;
  readonly dropped: number;
  /** Last event of any kind. */
  readonly lastEventAt: number | null;
  /** Last byte of any kind, heartbeats included. */
  readonly lastActivityAt: number | null;
  /** The cursor a reconnect would resume from. */
  readonly cursor: string | null;
  /** Delivery count on the current connection. */
  readonly sequence: number | null;
  /** Sequence discontinuities seen on this feed; newest first. */
  readonly gaps: readonly StreamGap[];
  readonly attempts: number;
  readonly retryAt: number | null;
  readonly error: string | null;
  readonly paused: boolean;
  reconnect(): void;
  setPaused(paused: boolean): void;
  clear(): void;
}

const BASE_BACKOFF_MS = 500;
const MAX_BACKOFF_MS = 30_000;

/** Exponential, capped, jittered — so a restarted platform is not stampeded. */
export function backoffFor(attempt: number, suggestedMs: number | null = null): number {
  const base = suggestedMs !== null && suggestedMs > 0 ? suggestedMs : BASE_BACKOFF_MS;
  const exponential = Math.min(MAX_BACKOFF_MS, base * 2 ** Math.max(0, attempt - 1));
  const jitter = exponential * 0.25 * (Math.random() * 2 - 1);
  return Math.max(250, Math.round(exponential + jitter));
}

export function useEventStream(options: UseEventStreamOptions): EventStream {
  const { channel, label, enabled = true, maxEvents = 250, staleAfterMs = 25_000 } = options;

  // The connection's own state. What the caller sees also folds in the two
  // conditions that are decided outside the connection — disabled and paused —
  // which are derived below rather than written back into this, so the effect
  // never has to set state just to describe a decision the caller already made.
  const [connection, setConnection] = useState<FeedState>("idle");
  const [events, setEvents] = useState<readonly StreamEnvelope[]>([]);
  const [received, setReceived] = useState(0);
  const [dropped, setDropped] = useState(0);
  const [lastEventAt, setLastEventAt] = useState<number | null>(null);
  const [lastActivityAt, setLastActivityAt] = useState<number | null>(null);
  const [cursor, setCursor] = useState<string | null>(null);
  const [sequence, setSequence] = useState<number | null>(null);
  const [gaps, setGaps] = useState<readonly StreamGap[]>([]);
  const [attempts, setAttempts] = useState(0);
  const [retryAt, setRetryAt] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [paused, setPausedState] = useState(false);
  const [generation, setGeneration] = useState(0);

  /** The cursor to resume from. Survives reconnects; that is its whole job. */
  const cursorRef = useRef<string | null>(null);
  const highWaterMarkRef = useRef<number | null>(null);

  const feedId = `stream:${channel}`;

  const reconnect = useCallback(() => {
    setPausedState(false);
    setGeneration((value) => value + 1);
  }, []);

  const setPaused = useCallback((next: boolean) => setPausedState(next), []);

  const clear = useCallback(() => {
    setEvents([]);
    setDropped(0);
    setGaps([]);
  }, []);

  // Registration is separate from the connection so the indicator lists a feed
  // that has not connected yet rather than showing nothing at all.
  useEffect(() => {
    connections.register(feedId, label, "stream", reconnect);
    return () => connections.deregister(feedId);
  }, [feedId, label, reconnect]);

  const state: FeedState = !enabled ? "closed" : paused ? "paused" : connection;

  useEffect(() => {
    connections.update(feedId, { state, lastEventAt: lastActivityAt, attempts, retryAt, detail: error });
  }, [feedId, state, lastActivityAt, attempts, retryAt, error]);

  useEffect(() => {
    if (!enabled || paused) return;

    const controller = new AbortController();
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const wait = (ms: number) =>
      new Promise<void>((resolve) => {
        timer = setTimeout(resolve, ms);
      });

    /** Contiguous per connection, so it resets with the connection. */
    let expectedSequence: number | null = null;
    let suggestedRetryMs: number | null = null;

    const ingest = (frames: readonly SseFrame[]) => {
      const now = Date.now();
      const accepted: StreamEnvelope[] = [];
      const found: StreamGap[] = [];

      for (const frame of frames) {
        if (frame.retryMs !== null) suggestedRetryMs = frame.retryMs;
        const envelope = decodeEnvelope(frame, now);

        // A resumed stream replays from the last acknowledged cursor. Anything
        // at or below the high-water mark has already been shown.
        if (
          envelope.cursor !== null &&
          highWaterMarkRef.current !== null &&
          envelope.cursor <= highWaterMarkRef.current
        ) {
          continue;
        }
        if (envelope.sequence !== null) {
          if (expectedSequence !== null && envelope.sequence !== expectedSequence) {
            found.push({ expected: expectedSequence, received: envelope.sequence, at: now });
          }
          expectedSequence = envelope.sequence + 1;
        }
        if (envelope.cursor !== null) highWaterMarkRef.current = envelope.cursor;
        if (envelope.lastEventId !== null) cursorRef.current = envelope.lastEventId;
        accepted.push(envelope);
      }

      if (found.length > 0) setGaps((previous) => [...found.reverse(), ...previous].slice(0, 20));
      if (accepted.length === 0) return;

      const newest = accepted[accepted.length - 1];
      setEvents((previous) => {
        const next = [...accepted].reverse().concat(previous);
        if (next.length > maxEvents) {
          setDropped((value) => value + (next.length - maxEvents));
          return next.slice(0, maxEvents);
        }
        return next;
      });
      setReceived((value) => value + accepted.length);
      setLastEventAt(now);
      if (newest) {
        setCursor(newest.lastEventId);
        setSequence(newest.sequence);
      }
      setConnection("open");
    };

    const run = async () => {
      let attempt = 0;
      while (!cancelled) {
        setConnection(attempt === 0 ? "connecting" : "reconnecting");
        setRetryAt(null);
        expectedSequence = null;
        try {
          const headers: Record<string, string> = { accept: "text/event-stream" };
          if (cursorRef.current !== null) headers["last-event-id"] = cursorRef.current;

          const response = await fetch(`/api/stream/${channel}`, {
            headers,
            signal: controller.signal,
            cache: "no-store",
          });
          if (!response.ok) {
            throw new Error(`the ${channel} stream answered ${response.status}`);
          }
          const body = response.body;
          if (!body) throw new Error(`the ${channel} stream carried no body`);

          attempt = 0;
          setAttempts(0);
          setError(null);
          setConnection("open");
          setLastActivityAt(Date.now());

          const reader = body.getReader();
          const decoder = new TextDecoder();
          const parser = new SseParser();
          for (;;) {
            const { done, value } = await reader.read();
            if (done) break;
            // Any byte is proof of life, heartbeat comments included.
            setLastActivityAt(Date.now());
            const frames = parser.push(decoder.decode(value, { stream: true }));
            if (frames.length > 0) ingest(frames);
          }
          const tail = parser.end();
          if (tail) ingest([tail]);
          if (cancelled) return;
          // The platform closes every connection at its lifetime bound and
          // names the cursor to resume from, so this is routine, not a fault.
          throw new Error(`the ${channel} stream ended; resuming from cursor ${cursorRef.current ?? "start"}`);
        } catch (cause) {
          if (cancelled || controller.signal.aborted) return;
          setError(cause instanceof Error ? cause.message : "the stream failed");
          setConnection("reconnecting");
        }

        attempt += 1;
        setAttempts(attempt);
        const delay = backoffFor(attempt, suggestedRetryMs);
        setRetryAt(Date.now() + delay);
        await wait(delay);
      }
    };

    void run();

    return () => {
      cancelled = true;
      controller.abort();
      if (timer !== undefined) clearTimeout(timer);
    };
  }, [channel, enabled, paused, generation, maxEvents]);

  // Staleness is time-based, so it needs its own clock rather than an event.
  useEffect(() => {
    if (connection !== "open" && connection !== "stale") return;
    const tick = setInterval(() => {
      if (lastActivityAt !== null && Date.now() - lastActivityAt > staleAfterMs) {
        setConnection((current) => (current === "open" ? "stale" : current));
      }
    }, 1_000);
    return () => clearInterval(tick);
  }, [connection, lastActivityAt, staleAfterMs]);

  return useMemo<EventStream>(
    () => ({
      state,
      events,
      received,
      dropped,
      lastEventAt,
      lastActivityAt,
      cursor,
      sequence,
      gaps,
      attempts,
      retryAt,
      error,
      paused,
      reconnect,
      setPaused,
      clear,
    }),
    [
      state,
      events,
      received,
      dropped,
      lastEventAt,
      lastActivityAt,
      cursor,
      sequence,
      gaps,
      attempts,
      retryAt,
      error,
      paused,
      reconnect,
      setPaused,
      clear,
    ],
  );
}
