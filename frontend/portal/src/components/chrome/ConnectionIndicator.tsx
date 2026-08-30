"use client";

import { useEffect, useRef, useState } from "react";
import { FEED_LABEL, FEED_TONE, StatusChip } from "@/components/data/Bits";
import { formatAgo } from "@/lib/format";
import { connections, summariseLink, useConnections, type LinkHealth } from "@/lib/hooks/connections";
import { useNow } from "@/lib/hooks/useNow";

const HEALTH_TONE: Record<LinkHealth, "ok" | "warn" | "bad" | "neutral"> = {
  live: "ok",
  degraded: "warn",
  down: "bad",
  idle: "neutral",
};

/**
 * Whether what is on the screen is current, for the whole screen.
 *
 * Every poll and every stream registers itself, so this is not a guess about
 * one connection: it is the state of all of them. Opening it lists each feed,
 * when it last carried anything and what it is doing now, and every row can be
 * reconnected individually — because during an incident the useful question is
 * which feed died, not whether one did.
 */
export function ConnectionIndicator() {
  const feeds = useConnections();
  const summary = summariseLink(feeds);
  const now = useNow();
  const [open, setOpen] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    const onClick = (event: MouseEvent) => {
      if (!containerRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("keydown", onKey);
    document.addEventListener("mousedown", onClick);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("mousedown", onClick);
    };
  }, [open]);

  return (
    <div className="relative" ref={containerRef}>
      <button
        type="button"
        className="btn"
        data-variant="ghost"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        aria-haspopup="dialog"
        data-testid="connection-indicator"
        data-health={summary.health}
        title={`${summary.open} live, ${summary.stale} stale, ${summary.failing} failing of ${summary.total} feed(s)`}
      >
        <StatusChip
          tone={HEALTH_TONE[summary.health]}
          label={summary.label}
          pulse={summary.health === "live"}
        />
        <span className="num hidden text-[10px] text-[color:var(--color-ink-faint)] sm:inline">
          {summary.open}/{summary.total}
        </span>
      </button>

      {open ? (
        <div
          className="absolute right-0 top-[30px] z-50 w-[380px] border border-[color:var(--color-line-strong)] bg-[color:var(--color-surface)] shadow-2xl"
          role="dialog"
          aria-label="Feed connections"
        >
          <div className="flex items-center gap-2 border-b border-[color:var(--color-line)] bg-[color:var(--color-sunken)] px-3 py-2">
            <h2 className="panel-title">Feeds</h2>
            <button
              type="button"
              className="btn ml-auto"
              data-variant="ghost"
              onClick={() => connections.reconnectAll()}
              data-testid="reconnect-all"
            >
              Reconnect all
            </button>
          </div>

          {feeds.length === 0 ? (
            <p className="px-3 py-4 text-[12px] text-[color:var(--color-ink-dim)]">
              No feed on this page has registered yet.
            </p>
          ) : (
            <ul className="max-h-[50vh] overflow-auto">
              {feeds.map((feed) => (
                <li
                  key={feed.id}
                  className="flex items-center gap-2 border-b border-[color:var(--color-line)] px-3 py-1.5 last:border-b-0"
                >
                  <span className="w-[42px] shrink-0">
                    <StatusChip tone={FEED_TONE[feed.state]} label={feed.kind === "stream" ? "sse" : "rest"} />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[11.5px]">{feed.label}</span>
                    <span className="num block text-[10px] text-[color:var(--color-ink-faint)]">
                      {FEED_LABEL[feed.state]}
                      {feed.lastEventAt !== null && now !== null
                        ? ` · ${formatAgo(feed.lastEventAt, now)}`
                        : " · no data yet"}
                      {feed.attempts > 0 ? ` · ${feed.attempts} failed` : ""}
                    </span>
                  </span>
                  <button
                    type="button"
                    className="btn"
                    data-variant="ghost"
                    onClick={() => connections.reconnect(feed.id)}
                    aria-label={`Reconnect ${feed.label}`}
                  >
                    ↻
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : null}
    </div>
  );
}
