"use client";

import type { ReactNode } from "react";
import { Sparkline } from "@/components/viz/primitives";
import { describeWindow, type Series } from "@/lib/hooks/useSeries";
import type { Tone } from "./Bits";

/**
 * The headline card: one number, its trend, and where the trend came from.
 *
 * The provenance line is not decoration. Every series on this console is
 * accumulated by the browser from polling, so a sparkline here describes only
 * what this tab has watched — and a reader who assumes it covers the platform's
 * lifetime will misread a two-minute window as a trend. The caption says the
 * window out loud for that reason.
 *
 * `value` is rendered exactly as given. Formatting belongs to the caller, which
 * knows whether the number is a count, a ratio or money; a card that formatted
 * on its own would eventually round something that must not be rounded.
 */
export interface KpiProps {
  readonly label: string;
  readonly value: ReactNode;
  /** Units or qualifier, shown small beside the value. */
  readonly unit?: string;
  readonly series?: Series;
  readonly tone?: Tone;
  /** Overrides the sparkline's direction colour. */
  readonly trend?: "up" | "down" | "flat" | "accent";
  /** One line under the value. Use it to say what the number means. */
  readonly note?: ReactNode;
  readonly title?: string;
}

const RULE: Record<Tone, string> = {
  neutral: "var(--color-line-strong)",
  ok: "var(--color-up)",
  warn: "var(--color-warn)",
  bad: "var(--color-down)",
  info: "var(--color-accent)",
};

export function Kpi({ label, value, unit, series, tone = "neutral", trend, note, title }: KpiProps) {
  return (
    <div
      className="relative flex min-w-0 flex-col gap-1 border border-[color:var(--color-line)] bg-[color:var(--color-surface)] px-3 py-2.5"
      title={title}
    >
      {/* A severity stripe, so what needs attention reads before the number. */}
      <span
        aria-hidden="true"
        className="absolute inset-y-0 left-0 w-[2px]"
        style={{ background: RULE[tone] }}
      />
      <span className="eyebrow truncate">{label}</span>
      <div className="flex items-end justify-between gap-2">
        <div className="flex min-w-0 items-baseline gap-1">
          <span className="num truncate text-[21px] font-semibold leading-none text-[color:var(--color-ink)]">
            {value}
          </span>
          {unit ? (
            <span className="num text-[10.5px] text-[color:var(--color-ink-faint)]">{unit}</span>
          ) : null}
        </div>
        {series ? (
          <Sparkline values={series.values} label={label} tone={trend} width={84} height={24} />
        ) : null}
      </div>
      {note ? (
        <span className="truncate text-[10.5px] text-[color:var(--color-ink-dim)]">{note}</span>
      ) : null}
      {series ? (
        <span className="num truncate text-[9.5px] text-[color:var(--color-ink-faint)]">
          {describeWindow(series)}
        </span>
      ) : null}
    </div>
  );
}

/** A responsive row of cards. */
export function KpiRow({ children }: { children: ReactNode }) {
  return (
    <div className="grid grid-cols-[repeat(auto-fit,minmax(168px,1fr))] gap-2">{children}</div>
  );
}
