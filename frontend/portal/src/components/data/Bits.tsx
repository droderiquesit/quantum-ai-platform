"use client";

import type { ReactNode } from "react";
import { describeOutcome } from "@/lib/api/client";
import type { Direction } from "@/lib/format";
import { formatAgo, formatDurationMs } from "@/lib/format";
import type { FeedState } from "@/lib/hooks/connections";
import type { EventStream } from "@/lib/hooks/useEventStream";
import { useNow } from "@/lib/hooks/useNow";
import type { Resource } from "@/lib/hooks/useResource";

export type Tone = "neutral" | "ok" | "warn" | "bad" | "info";

export function Chip({
  tone = "neutral",
  children,
  title,
}: {
  tone?: Tone;
  children: ReactNode;
  title?: string;
}) {
  return (
    <span className="chip" data-tone={tone === "neutral" ? undefined : tone} title={title}>
      {children}
    </span>
  );
}

export function StatusChip({
  tone,
  label,
  pulse = false,
  title,
}: {
  tone: Tone;
  label: string;
  pulse?: boolean;
  title?: string;
}) {
  return (
    <Chip tone={tone} title={title}>
      <span className="dot" data-pulse={pulse ? "true" : undefined} aria-hidden="true" />
      {label}
    </Chip>
  );
}

export const FEED_TONE: Record<FeedState, Tone> = {
  idle: "neutral",
  connecting: "info",
  open: "ok",
  stale: "warn",
  reconnecting: "warn",
  paused: "neutral",
  error: "bad",
  closed: "neutral",
};

export const FEED_LABEL: Record<FeedState, string> = {
  idle: "idle",
  connecting: "connecting",
  open: "live",
  stale: "stale",
  reconnecting: "reconnecting",
  paused: "paused",
  error: "error",
  closed: "closed",
};

/** A single figure, with what it is and how confident to be in it. */
export function Metric({
  label,
  value,
  hint,
  direction,
  tone,
}: {
  label: string;
  value: ReactNode;
  hint?: ReactNode;
  direction?: Direction;
  tone?: Tone;
}) {
  const toneColour =
    tone === "bad"
      ? "var(--color-down)"
      : tone === "warn"
        ? "var(--color-warn)"
        : tone === "ok"
          ? "var(--color-up)"
          : undefined;
  return (
    <div className="flex min-w-0 flex-col gap-0.5 border-l border-[color:var(--color-line)] px-3 py-1.5 first:border-l-0 first:pl-0">
      <span className="eyebrow truncate">{label}</span>
      <span
        className="num truncate text-[17px] leading-tight"
        data-direction={direction}
        style={toneColour && !direction ? { color: toneColour } : undefined}
      >
        {value}
      </span>
      {hint ? (
        <span className="truncate text-[10px] text-[color:var(--color-ink-faint)]">{hint}</span>
      ) : null}
    </div>
  );
}

export function MetricRow({ children }: { children: ReactNode }) {
  return <div className="flex flex-wrap items-stretch gap-y-1">{children}</div>;
}

export function KeyValue({
  label,
  children,
  mono = true,
}: {
  label: string;
  children: ReactNode;
  mono?: boolean;
}) {
  return (
    <div className="flex items-baseline justify-between gap-4 border-b border-[color:var(--color-line)] py-1 last:border-b-0">
      <dt className="shrink-0 text-[11px] text-[color:var(--color-ink-dim)]">{label}</dt>
      <dd
        className={`min-w-0 truncate text-right text-[12px] ${mono ? "num" : ""}`}
        title={typeof children === "string" ? children : undefined}
      >
        {children}
      </dd>
    </div>
  );
}

/**
 * The age and health of a polled panel, with the control that refreshes it.
 *
 * Shown in every panel head that reads REST, because "when was this true?" is
 * the question a number on a trading screen cannot answer for itself.
 */
export function Freshness({ resource, name }: { resource: Resource<unknown>; name: string }) {
  const now = useNow();
  const { receivedAt, latencyMs, stale, refreshing, outcome, refresh, attempts } = resource;

  const tone: Tone =
    outcome === null
      ? "neutral"
      : outcome.kind === "ok"
        ? stale
          ? "warn"
          : "ok"
        : outcome.kind === "unavailable"
          ? "warn"
          : "bad";

  const label =
    outcome === null
      ? "loading"
      : outcome.kind === "ok"
        ? stale
          ? "stale"
          : "ok"
        : outcome.kind === "unavailable"
          ? "absent"
          : outcome.kind;

  return (
    <>
      <StatusChip
        tone={tone}
        label={label}
        title={outcome === null ? "waiting for the first answer" : describeOutcome(outcome)}
      />
      <span className="num text-[10px] text-[color:var(--color-ink-faint)]">
        {now === null ? "—" : formatAgo(receivedAt, now)}
        {latencyMs === null ? "" : ` · ${formatDurationMs(latencyMs)}`}
        {attempts > 0 ? ` · ${attempts} failed` : ""}
      </span>
      <button
        type="button"
        className="btn"
        data-variant="ghost"
        onClick={refresh}
        disabled={refreshing}
        aria-label={`Refresh ${name}`}
        title={`Refresh ${name} now`}
      >
        {refreshing ? "…" : "↻"}
      </button>
    </>
  );
}

/** The state of a stream, with the controls that act on it. */
export function StreamControls({ stream, name }: { stream: EventStream; name: string }) {
  const now = useNow();
  const countdown =
    stream.retryAt !== null && now !== null ? Math.max(0, Math.round((stream.retryAt - now) / 1000)) : null;

  return (
    <>
      <StatusChip
        tone={FEED_TONE[stream.state]}
        label={FEED_LABEL[stream.state]}
        pulse={stream.state === "open"}
        title={stream.error ?? `${name} stream`}
      />
      <span className="num text-[10px] text-[color:var(--color-ink-faint)]">
        {stream.received} evt
        {stream.cursor === null ? "" : ` · cur ${stream.cursor}`}
        {stream.gaps.length > 0 ? ` · ${stream.gaps.length} gap` : ""}
        {countdown !== null && countdown > 0 ? ` · retry ${countdown}s` : ""}
        {stream.lastEventAt !== null && now !== null ? ` · ${formatAgo(stream.lastEventAt, now)}` : ""}
      </span>
      <button
        type="button"
        className="btn"
        data-variant="ghost"
        onClick={() => stream.setPaused(!stream.paused)}
        aria-pressed={stream.paused}
        aria-label={stream.paused ? `Resume the ${name} stream` : `Pause the ${name} stream`}
      >
        {stream.paused ? "Resume" : "Pause"}
      </button>
      <button
        type="button"
        className="btn"
        data-variant="ghost"
        onClick={stream.reconnect}
        aria-label={`Reconnect the ${name} stream`}
      >
        Reconnect
      </button>
    </>
  );
}
