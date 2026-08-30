"use client";

import { useSyncExternalStore } from "react";

/**
 * One place that knows the state of every feed on the screen.
 *
 * The chrome has to answer "is what I am looking at current?" for the whole
 * page, not per panel, so each poll and each stream registers itself here and
 * the connection indicator reads the register. A panel that quietly stopped
 * updating is the failure mode this exists to make impossible to miss.
 */

export type FeedKind = "stream" | "poll";

export type FeedState =
  /** Registered, not started. */
  | "idle"
  /** First attempt in flight. */
  | "connecting"
  /** Receiving. */
  | "open"
  /** Connected, but nothing has arrived for longer than the channel's bound. */
  | "stale"
  /** Dropped, waiting out a backoff before trying again. */
  | "reconnecting"
  /** Stopped by the operator. */
  | "paused"
  /** The last attempt failed and reported why. */
  | "error"
  /** Deliberately stopped, not an error. */
  | "closed";

export interface FeedStatus {
  readonly id: string;
  readonly label: string;
  readonly kind: FeedKind;
  readonly state: FeedState;
  /** When data last arrived on this feed. */
  readonly lastEventAt: number | null;
  /** Consecutive failed attempts since the last success. */
  readonly attempts: number;
  /** When the next reconnect is due, for a countdown. */
  readonly retryAt: number | null;
  readonly detail: string | null;
  readonly updatedAt: number;
}

export type FeedPatch = Partial<Omit<FeedStatus, "id" | "label" | "kind">>;

const EMPTY: readonly FeedStatus[] = Object.freeze([]);

class ConnectionRegister {
  private feeds = new Map<string, FeedStatus>();
  private listeners = new Set<() => void>();
  private snapshot: readonly FeedStatus[] = EMPTY;
  private reconnectRequests = new Map<string, () => void>();

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = (): readonly FeedStatus[] => this.snapshot;

  getServerSnapshot = (): readonly FeedStatus[] => EMPTY;

  register(id: string, label: string, kind: FeedKind, reconnect: () => void): void {
    this.reconnectRequests.set(id, reconnect);
    const existing = this.feeds.get(id);
    this.feeds.set(id, {
      id,
      label,
      kind,
      state: existing?.state ?? "idle",
      lastEventAt: existing?.lastEventAt ?? null,
      attempts: existing?.attempts ?? 0,
      retryAt: existing?.retryAt ?? null,
      detail: existing?.detail ?? null,
      updatedAt: Date.now(),
    });
    this.publish();
  }

  deregister(id: string): void {
    this.reconnectRequests.delete(id);
    if (this.feeds.delete(id)) this.publish();
  }

  update(id: string, patch: FeedPatch): void {
    const existing = this.feeds.get(id);
    if (!existing) return;
    const next: FeedStatus = { ...existing, ...patch, updatedAt: Date.now() };
    const unchanged =
      next.state === existing.state &&
      next.lastEventAt === existing.lastEventAt &&
      next.attempts === existing.attempts &&
      next.retryAt === existing.retryAt &&
      next.detail === existing.detail;
    if (unchanged) return;
    this.feeds.set(id, next);
    this.publish();
  }

  /** Ask every registered feed to reconnect now. A real control, not a hint. */
  reconnectAll(): number {
    const requests = [...this.reconnectRequests.values()];
    for (const request of requests) request();
    return requests.length;
  }

  reconnect(id: string): boolean {
    const request = this.reconnectRequests.get(id);
    if (!request) return false;
    request();
    return true;
  }

  private publish(): void {
    this.snapshot = Object.freeze([...this.feeds.values()].sort((a, b) => a.id.localeCompare(b.id)));
    for (const listener of this.listeners) listener();
  }
}

export const connections = new ConnectionRegister();

export function useConnections(): readonly FeedStatus[] {
  return useSyncExternalStore(
    connections.subscribe,
    connections.getSnapshot,
    connections.getServerSnapshot,
  );
}

export type LinkHealth = "live" | "degraded" | "down" | "idle";

export interface LinkSummary {
  readonly health: LinkHealth;
  readonly total: number;
  readonly open: number;
  readonly stale: number;
  readonly failing: number;
  readonly label: string;
}

/** The single word the chrome shows, and the counts behind it. */
export function summariseLink(feeds: readonly FeedStatus[]): LinkSummary {
  if (feeds.length === 0) {
    return { health: "idle", total: 0, open: 0, stale: 0, failing: 0, label: "no feeds" };
  }
  let open = 0;
  let stale = 0;
  let failing = 0;
  for (const feed of feeds) {
    if (feed.state === "open") open += 1;
    else if (feed.state === "stale") stale += 1;
    else if (feed.state === "error" || feed.state === "reconnecting") failing += 1;
  }
  if (failing === feeds.length) {
    return { health: "down", total: feeds.length, open, stale, failing, label: "disconnected" };
  }
  if (failing > 0 || stale > 0) {
    return { health: "degraded", total: feeds.length, open, stale, failing, label: "degraded" };
  }
  if (open === 0) {
    return { health: "idle", total: feeds.length, open, stale, failing, label: "connecting" };
  }
  return { health: "live", total: feeds.length, open, stale, failing, label: "live" };
}
