/**
 * Rendering rules for numbers on a trading screen.
 *
 * Two of these matter more than the rest. Quantities, prices and notionals
 * arrive from the platform as decimal strings and are never parsed into a
 * float here: a book that reads 1000000.1 upstream must not read 1000000.09999
 * downstream. And direction is decided from the string's sign rather than from
 * a computed value, so the only thing that ever turns a cell green or red is
 * the sign the platform sent.
 */

const INTEGER = new Intl.NumberFormat("en-GB", { useGrouping: true, maximumFractionDigits: 0 });

export function formatCount(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return "—";
  return INTEGER.format(value);
}

export function formatPercent(share: number | null | undefined, digits = 2): string {
  if (share === null || share === undefined || !Number.isFinite(share)) return "—";
  return `${(share * 100).toFixed(digits)}%`;
}

/** Micros are the unit the platform charges a budget in; kept integral. */
export function formatMicros(micros: number | null | undefined): string {
  if (micros === null || micros === undefined || !Number.isFinite(micros)) return "—";
  const whole = Math.trunc(micros / 1_000_000);
  const fraction = Math.abs(micros % 1_000_000)
    .toString()
    .padStart(6, "0")
    .slice(0, 2);
  return `${INTEGER.format(whole)}.${fraction}`;
}

export type Direction = "positive" | "negative" | "flat";

/** The sign of a decimal the platform sent, read without parsing it. */
export function directionOf(decimal: string | null | undefined): Direction {
  if (decimal === null || decimal === undefined) return "flat";
  const trimmed = decimal.trim();
  if (trimmed === "") return "flat";
  if (trimmed.startsWith("-")) return /[1-9]/.test(trimmed) ? "negative" : "flat";
  return /[1-9]/.test(trimmed) ? "positive" : "flat";
}

/** A decimal string, grouped for reading, with its own digits preserved. */
export function formatDecimal(decimal: string | null | undefined): string {
  if (decimal === null || decimal === undefined || decimal.trim() === "") return "—";
  const trimmed = decimal.trim();
  const match = /^(-?)(\d+)(?:\.(\d+))?$/.exec(trimmed);
  if (!match) return trimmed;
  const sign = match[1] ?? "";
  const whole = match[2] ?? "0";
  const fraction = match[3];
  const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return fraction === undefined ? `${sign}${grouped}` : `${sign}${grouped}.${fraction}`;
}

export function formatDurationMs(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return "—";
  const abs = Math.abs(ms);
  if (abs < 1_000) return `${Math.round(ms)}ms`;
  if (abs < 60_000) return `${(ms / 1_000).toFixed(1)}s`;
  const minutes = Math.floor(abs / 60_000);
  const seconds = Math.round((abs % 60_000) / 1_000);
  const sign = ms < 0 ? "-" : "";
  if (minutes < 60) return `${sign}${minutes}m ${seconds.toString().padStart(2, "0")}s`;
  const hours = Math.floor(minutes / 60);
  return `${sign}${hours}h ${(minutes % 60).toString().padStart(2, "0")}m`;
}

export function formatAgo(at: number | null | undefined, now: number = Date.now()): string {
  if (at === null || at === undefined || !Number.isFinite(at)) return "never";
  const delta = now - at;
  if (delta < 0) return "just now";
  if (delta < 1_000) return "now";
  return `${formatDurationMs(delta)} ago`;
}

/** UTC wall clock, to the millisecond. Trading logs are read in UTC. */
export function formatClock(at: number | null | undefined): string {
  if (at === null || at === undefined || !Number.isFinite(at)) return "—";
  const date = new Date(at);
  const pad = (value: number, width = 2) => value.toString().padStart(width, "0");
  return `${pad(date.getUTCHours())}:${pad(date.getUTCMinutes())}:${pad(
    date.getUTCSeconds(),
  )}.${pad(date.getUTCMilliseconds(), 3)}`;
}

export function formatUtcDate(at: number | null | undefined): string {
  if (at === null || at === undefined || !Number.isFinite(at)) return "—";
  const date = new Date(at);
  const pad = (value: number) => value.toString().padStart(2, "0");
  return `${date.getUTCFullYear()}-${pad(date.getUTCMonth() + 1)}-${pad(date.getUTCDate())}`;
}

/** An RFC 3339 stamp from the platform, shown as date and UTC clock. */
export function formatTimestamp(iso: string | null | undefined): string {
  if (!iso) return "—";
  const parsed = Date.parse(iso);
  if (!Number.isFinite(parsed)) return iso;
  return `${formatUtcDate(parsed)} ${formatClock(parsed)}`;
}

export function truncate(value: string, width: number): string {
  return value.length <= width ? value : `${value.slice(0, Math.max(0, width - 1))}…`;
}
