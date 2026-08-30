"use client";

import type { ReactNode } from "react";
import type { ApiOutcome } from "@/lib/api/client";
import type { MissingEndpoint } from "@/lib/api/endpoints";
import type { Resource } from "@/lib/hooks/useResource";

/**
 * The states a data view can be in, other than having data.
 *
 * Each is visually distinct and each says something specific. The rule they all
 * follow: a panel with nothing in it must say why it has nothing, and must
 * never resemble a panel showing a zero. "No orders" and "the orders endpoint
 * could not be reached" are opposite facts and must not look alike.
 */

type Tone = "neutral" | "info" | "warn" | "bad";

const TONE_CLASS: Record<Tone, string> = {
  neutral: "text-[color:var(--color-ink-faint)] border-[color:var(--color-line-strong)]",
  info: "text-[color:var(--color-accent)] border-[color:var(--color-accent-dim)]",
  warn: "text-[color:var(--color-warn)] border-[color:var(--color-warn)]/40",
  bad: "text-[color:var(--color-down)] border-[color:var(--color-down)]/50",
};

export function StateBlock({
  tone = "neutral",
  label,
  headline,
  children,
  action,
  compact = false,
}: {
  tone?: Tone;
  label: string;
  headline: string;
  children?: ReactNode;
  action?: ReactNode;
  compact?: boolean;
}) {
  return (
    <div
      role="status"
      className={`flex flex-col items-start gap-2 border border-dashed ${
        compact ? "px-3 py-3" : "px-4 py-6"
      } ${TONE_CLASS[tone]}`}
      data-state-block={label.toLowerCase().replace(/\s+/g, "-")}
    >
      <span className="eyebrow" style={{ color: "inherit" }}>
        {label}
      </span>
      <p className="text-[13px] font-medium text-[color:var(--color-ink)]">{headline}</p>
      {children ? (
        <div className="max-w-[70ch] text-[12px] leading-relaxed text-[color:var(--color-ink-dim)]">
          {children}
        </div>
      ) : null}
      {action ? <div className="pt-1">{action}</div> : null}
    </div>
  );
}

/** A skeleton with the shape of the table it will be replaced by. */
export function LoadingBlock({ rows = 4, label = "loading" }: { rows?: number; label?: string }) {
  return (
    <div className="flex flex-col gap-1.5 p-3" role="status" aria-live="polite" aria-busy="true">
      <span className="eyebrow">{label}</span>
      {Array.from({ length: rows }, (_, index) => (
        <div
          key={index}
          className="h-[18px] animate-pulse bg-[color:var(--color-raised)]"
          style={{ width: `${100 - index * 9}%`, animationDelay: `${index * 90}ms` }}
        />
      ))}
      <span className="sr-only">Loading data</span>
    </div>
  );
}

export function EmptyBlock({ headline, children }: { headline: string; children?: ReactNode }) {
  return (
    <StateBlock tone="neutral" label="empty" headline={headline} compact>
      {children}
    </StateBlock>
  );
}

export function UnavailableBlock({ subject, reason }: { subject: string; reason: string }) {
  return (
    <StateBlock
      tone="warn"
      label="not available"
      headline={`The platform serves no ${subject} in this deployment.`}
    >
      <p>{reason}</p>
    </StateBlock>
  );
}

export function MissingEndpointBlock({
  endpoint,
  action,
}: {
  endpoint: MissingEndpoint;
  action?: ReactNode;
}) {
  return (
    <StateBlock
      tone="warn"
      label="endpoint missing"
      headline={`Not yet available — ${endpoint.method} ${endpoint.path} is missing.`}
      action={action}
    >
      <p>
        Needed for {endpoint.needed_for}. {endpoint.note}
      </p>
      <p className="mt-1.5 text-[color:var(--color-ink-faint)]">
        Nothing is shown in its place. This console does not synthesise data it was not given.
      </p>
    </StateBlock>
  );
}

export function RouteMissingBlock({
  path,
  status,
  detail,
  onRetry,
}: {
  path: string;
  status: number;
  detail: string;
  onRetry?: () => void;
}) {
  return (
    <StateBlock
      tone="warn"
      label="endpoint missing"
      headline={`Not yet available — GET /api/v1${path} answered ${status}.`}
      action={onRetry ? <RetryButton onRetry={onRetry} /> : undefined}
    >
      <p>{detail}</p>
    </StateBlock>
  );
}

export function UnreachableBlock({ detail, onRetry }: { detail: string; onRetry?: () => void }) {
  return (
    <StateBlock
      tone="bad"
      label="disconnected"
      headline="The platform could not be reached."
      action={onRetry ? <RetryButton onRetry={onRetry} /> : undefined}
    >
      <p>{detail}</p>
      <p className="mt-1.5 text-[color:var(--color-ink-faint)]">
        Anything still on screen was last read before the connection dropped and is not current.
      </p>
    </StateBlock>
  );
}

export function DeniedBlock({ detail, endpoint }: { detail: string; endpoint: string }) {
  return (
    <StateBlock
      tone="bad"
      label="refused"
      headline={`The console's credential may not read ${endpoint}.`}
    >
      <p>{detail}</p>
      <p className="mt-1.5 text-[color:var(--color-ink-faint)]">
        Roles are per route on the platform. Check the credential in QIP_API_TOKEN carries the role
        this route requires.
      </p>
    </StateBlock>
  );
}

export function ErrorBlock({
  headline,
  detail,
  onRetry,
}: {
  headline: string;
  detail: string;
  onRetry?: () => void;
}) {
  return (
    <StateBlock
      tone="bad"
      label="error"
      headline={headline}
      action={onRetry ? <RetryButton onRetry={onRetry} /> : undefined}
    >
      <p>{detail}</p>
    </StateBlock>
  );
}

function RetryButton({ onRetry }: { onRetry: () => void }) {
  return (
    <button type="button" className="btn" onClick={onRetry}>
      Retry now
    </button>
  );
}

/**
 * Render a resource, or the reason it has nothing to render.
 *
 * Every page routes its panels through this so that no page can accidentally
 * omit one of the failure states, and so that all of them describe the same
 * condition the same way.
 */
export function ResourceView<D>({
  resource,
  children,
  loadingRows = 4,
}: {
  resource: Resource<D>;
  children: (data: D) => ReactNode;
  loadingRows?: number;
}) {
  const { outcome, loading, refresh } = resource;

  if (loading && outcome === null) return <LoadingBlock rows={loadingRows} />;
  if (outcome === null) return <LoadingBlock rows={loadingRows} />;

  return <>{renderOutcome(outcome, children, refresh)}</>;
}

function renderOutcome<D>(
  outcome: ApiOutcome<D>,
  children: (data: D) => ReactNode,
  refresh: () => void,
): ReactNode {
  switch (outcome.kind) {
    case "ok":
      return children(outcome.data);
    case "unavailable":
      return <UnavailableBlock subject={outcome.subject} reason={outcome.reason} />;
    case "missing":
      return (
        <RouteMissingBlock
          path={outcome.endpoint}
          status={outcome.status}
          detail={outcome.detail}
          onRetry={refresh}
        />
      );
    case "denied":
      return <DeniedBlock endpoint={`/api/v1${outcome.endpoint}`} detail={outcome.detail} />;
    case "unreachable":
      return <UnreachableBlock detail={outcome.detail} onRetry={refresh} />;
    case "error":
      return (
        <ErrorBlock
          headline={`/api/v1${outcome.endpoint} answered ${outcome.status ?? "no status"}.`}
          detail={outcome.detail}
          onRetry={refresh}
        />
      );
  }
}
