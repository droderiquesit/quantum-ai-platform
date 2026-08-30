/**
 * Charts, drawn by hand in SVG.
 *
 * No charting library. `package.json` is governed the way `Cargo.toml` is —
 * every addition is reviewed and the transitive tree is part of the diff — and
 * a chart library is a large tree to take on for shapes this simple. Everything
 * here is a path or a rect over a linear scale, in about the space the wrapper
 * around a library would have taken.
 *
 * Three rules hold across every mark below, because a chart on a trading
 * surface is read at a glance and a wrong glance is worse than no chart:
 *
 * * **A series of one point is not a line.** Every component refuses to draw
 *   rather than emitting a flat segment through a single observation, which
 *   reads as a measured constant.
 * * **The domain is stated, never inferred silently.** A sparkline over a
 *   constant series has no range; it is drawn on its midline and says so by
 *   being flat, not by being auto-scaled into noise.
 * * **Colour means direction.** Green and red are for gain, loss and breach.
 *   Structure is grey and the accent is blue, so a flash of either signal
 *   colour always means something.
 */
import type { ReactNode } from "react";

/** A finite series, with the non-finite points dropped and counted. */
function clean(values: readonly number[]): { points: number[]; dropped: number } {
  const points = values.filter((v) => Number.isFinite(v));
  return { points, dropped: values.length - points.length };
}

function extent(points: readonly number[]): { lo: number; hi: number; flat: boolean } {
  let lo = points[0] ?? 0;
  let hi = lo;
  for (const v of points) {
    if (v < lo) lo = v;
    if (v > hi) hi = v;
  }
  // A constant series has no range to scale over. Reporting it as flat lets the
  // caller draw a midline rather than dividing by zero or inventing a spread.
  return { lo, hi, flat: hi - lo < Number.EPSILON };
}

function pathFor(
  points: readonly number[],
  width: number,
  height: number,
  pad: number,
): { line: string; area: string; last: { x: number; y: number } } {
  const { lo, hi, flat } = extent(points);
  const usable = height - pad * 2;
  const step = points.length > 1 ? width / (points.length - 1) : 0;
  const y = (v: number) => (flat ? pad + usable / 2 : pad + usable - ((v - lo) / (hi - lo)) * usable);

  let line = "";
  points.forEach((v, i) => {
    line += `${i === 0 ? "M" : "L"}${(i * step).toFixed(2)},${y(v).toFixed(2)}`;
  });
  const lastX = (points.length - 1) * step;
  const lastY = y(points[points.length - 1] ?? 0);
  return {
    line,
    area: `${line}L${lastX.toFixed(2)},${height}L0,${height}Z`,
    last: { x: lastX, y: lastY },
  };
}

export interface SparklineProps {
  readonly values: readonly number[];
  readonly width?: number;
  readonly height?: number;
  /** Overrides the direction colour. Omit to derive it from first vs last. */
  readonly tone?: "up" | "down" | "flat" | "accent";
  readonly label: string;
}

/**
 * A trend, small enough to sit inside a number.
 *
 * The endpoint is marked. Without it the eye has to find which end is "now",
 * and on a series that happens to be symmetric it will sometimes get it wrong.
 */
export function Sparkline({ values, width = 96, height = 26, tone, label }: SparklineProps) {
  const { points } = clean(values);
  if (points.length < 2) {
    return (
      <span
        className="num text-[10px] text-[color:var(--color-ink-faint)]"
        title={`${label}: ${points.length} observation(s) — a line needs at least two`}
      >
        {points.length === 0 ? "no data" : "1 obs"}
      </span>
    );
  }
  const first = points[0] ?? 0;
  const last = points[points.length - 1] ?? 0;
  const direction = tone ?? (last > first ? "up" : last < first ? "down" : "flat");
  const stroke =
    direction === "up"
      ? "var(--color-up)"
      : direction === "down"
        ? "var(--color-down)"
        : direction === "accent"
          ? "var(--color-accent)"
          : "var(--color-ink-faint)";
  const { line, area, last: end } = pathFor(points, width, height, 3);
  const id = `sp-${label.replace(/\W/g, "")}`;

  return (
    <svg
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      role="img"
      aria-label={`${label}: ${points.length} observations, ${direction}`}
      className="overflow-visible"
    >
      <defs>
        <linearGradient id={id} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor={stroke} stopOpacity="0.22" />
          <stop offset="100%" stopColor={stroke} stopOpacity="0" />
        </linearGradient>
      </defs>
      <path d={area} fill={`url(#${id})`} />
      <path d={line} fill="none" stroke={stroke} strokeWidth="1.25" strokeLinejoin="round" />
      <circle cx={end.x} cy={end.y} r="1.9" fill={stroke} />
    </svg>
  );
}

export interface AreaChartProps {
  readonly values: readonly number[];
  readonly height?: number;
  readonly label: string;
  /** Rendered under the plot, left to right. */
  readonly caption?: ReactNode;
  readonly tone?: "up" | "down" | "accent";
}

/**
 * A full-width series with a faint grid.
 *
 * The grid is four lines and no axis labels: this is a shape-reader, and the
 * exact values live in the table beside it. A chart that repeats the table's
 * numbers badly is worse than one that does not try.
 */
export function AreaChart({ values, height = 132, label, caption, tone = "accent" }: AreaChartProps) {
  const { points, dropped } = clean(values);
  if (points.length < 2) {
    return (
      <div
        className="flex items-center justify-center border border-dashed border-[color:var(--color-line)] text-[11px] text-[color:var(--color-ink-faint)]"
        style={{ height }}
      >
        {points.length === 0
          ? "nothing observed yet"
          : "one observation — a line needs at least two"}
      </div>
    );
  }
  const W = 600;
  const stroke =
    tone === "up" ? "var(--color-up)" : tone === "down" ? "var(--color-down)" : "var(--color-accent)";
  const { line, area, last } = pathFor(points, W, height, 6);
  const { lo, hi, flat } = extent(points);
  const id = `ac-${label.replace(/\W/g, "")}`;

  return (
    <div className="flex flex-col gap-1">
      <svg
        viewBox={`0 0 ${W} ${height}`}
        preserveAspectRatio="none"
        className="w-full"
        style={{ height }}
        role="img"
        aria-label={`${label}: ${points.length} observations from ${lo} to ${hi}`}
      >
        <defs>
          <linearGradient id={id} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={stroke} stopOpacity="0.20" />
            <stop offset="100%" stopColor={stroke} stopOpacity="0" />
          </linearGradient>
        </defs>
        {[0.25, 0.5, 0.75].map((f) => (
          <line
            key={f}
            x1="0"
            x2={W}
            y1={height * f}
            y2={height * f}
            stroke="var(--color-line)"
            strokeWidth="0.6"
          />
        ))}
        <path d={area} fill={`url(#${id})`} />
        <path
          d={line}
          fill="none"
          stroke={stroke}
          strokeWidth="1.4"
          vectorEffect="non-scaling-stroke"
          strokeLinejoin="round"
        />
        <circle cx={last.x} cy={last.y} r="2.4" fill={stroke} />
      </svg>
      <div className="flex items-baseline justify-between text-[10px] text-[color:var(--color-ink-faint)]">
        <span className="num">
          {flat ? `constant at ${lo}` : `${lo} – ${hi}`}
          {dropped > 0 ? ` · ${dropped} non-finite dropped` : ""}
        </span>
        {caption ? <span className="num">{caption}</span> : null}
      </div>
    </div>
  );
}

export interface GaugeProps {
  /** 0–1. Values outside are clamped for the arc and reported in the label. */
  readonly fraction: number;
  readonly label: string;
  readonly caption?: string;
  readonly tone?: "ok" | "warn" | "bad" | "accent";
  readonly size?: number;
}

/**
 * A utilisation arc.
 *
 * Clamped for drawing only. A value above one is a real state — a limit
 * exceeded — so the arc fills and the caption still carries the true number,
 * rather than the reader seeing a full gauge and assuming exactly full.
 */
export function Gauge({ fraction, label, caption, tone = "accent", size = 92 }: GaugeProps) {
  const safe = Number.isFinite(fraction) ? fraction : 0;
  const drawn = Math.max(0, Math.min(1, safe));
  const stroke =
    tone === "ok"
      ? "var(--color-up)"
      : tone === "warn"
        ? "var(--color-warn)"
        : tone === "bad"
          ? "var(--color-down)"
          : "var(--color-accent)";
  const r = size / 2 - 7;
  const c = Math.PI * r; // half circumference: this is a 180° arc
  const cx = size / 2;
  const cy = size / 2 + 2;

  return (
    <div className="flex flex-col items-center gap-0.5">
      <svg
        width={size}
        height={size * 0.62}
        viewBox={`0 0 ${size} ${size * 0.62}`}
        role="img"
        aria-label={`${label}: ${(safe * 100).toFixed(1)} per cent`}
      >
        <path
          d={`M${cx - r},${cy} A${r},${r} 0 0 1 ${cx + r},${cy}`}
          fill="none"
          stroke="var(--color-line)"
          strokeWidth="6"
          strokeLinecap="round"
        />
        <path
          d={`M${cx - r},${cy} A${r},${r} 0 0 1 ${cx + r},${cy}`}
          fill="none"
          stroke={stroke}
          strokeWidth="6"
          strokeLinecap="round"
          strokeDasharray={`${(c * drawn).toFixed(2)} ${c.toFixed(2)}`}
        />
        <text
          x={cx}
          y={cy - 4}
          textAnchor="middle"
          className="num"
          fill="var(--color-ink)"
          fontSize="15"
          fontWeight="600"
        >
          {Number.isFinite(fraction) ? `${(safe * 100).toFixed(0)}%` : "—"}
        </text>
      </svg>
      <span className="text-[10.5px] text-[color:var(--color-ink-dim)]">{label}</span>
      {caption ? (
        <span className="num text-[9.5px] text-[color:var(--color-ink-faint)]">{caption}</span>
      ) : null}
    </div>
  );
}

export interface BarsProps {
  readonly items: readonly { readonly label: string; readonly value: number; readonly tone?: "up" | "down" | "accent" | "flat" }[];
  readonly unit?: string;
}

/**
 * A horizontal ranking.
 *
 * Scaled against the largest value present, not against a fixed maximum: the
 * question this answers is "which of these is biggest", and a fixed scale
 * makes every bar short whenever the set happens to be small.
 */
export function Bars({ items, unit }: BarsProps) {
  if (items.length === 0) {
    return <p className="text-[11px] text-[color:var(--color-ink-faint)]">nothing to rank</p>;
  }
  const max = Math.max(...items.map((i) => Math.abs(i.value)), 1);
  return (
    <ul className="flex flex-col gap-1.5">
      {items.map((item) => {
        const width = (Math.abs(item.value) / max) * 100;
        const stroke =
          item.tone === "up"
            ? "var(--color-up)"
            : item.tone === "down"
              ? "var(--color-down)"
              : item.tone === "flat"
                ? "var(--color-ink-faint)"
                : "var(--color-accent)";
        return (
          <li key={item.label} className="flex items-center gap-2">
            <span className="w-[124px] shrink-0 truncate text-[11.5px] text-[color:var(--color-ink-dim)]">
              {item.label}
            </span>
            <span className="h-[7px] flex-1 overflow-hidden rounded-[2px] bg-[color:var(--color-line)]">
              <span
                className="block h-full rounded-[2px]"
                style={{ width: `${width}%`, background: stroke }}
              />
            </span>
            <span className="num w-[58px] shrink-0 text-right text-[11px] text-[color:var(--color-ink)]">
              {item.value}
              {unit ?? ""}
            </span>
          </li>
        );
      })}
    </ul>
  );
}

export interface HeatCell {
  readonly row: string;
  readonly column: string;
  /** −1 to 1. Null renders as "not measured" rather than as zero. */
  readonly value: number | null;
}

/**
 * A matrix where absence and zero are different colours.
 *
 * The distinction is the point. A correlation nobody measured and a correlation
 * measured at zero look identical on most heatmaps, and on a risk surface that
 * is the difference between "these are independent" and "we did not look".
 */
export function Heatmap({
  cells,
  rows,
  columns,
  label,
}: {
  readonly cells: readonly HeatCell[];
  readonly rows: readonly string[];
  readonly columns: readonly string[];
  readonly label: string;
}) {
  const at = (row: string, column: string) =>
    cells.find((c) => c.row === row && c.column === column)?.value ?? null;

  return (
    <div className="overflow-x-auto">
      <table className="border-separate border-spacing-[2px]" aria-label={label}>
        <thead>
          <tr>
            <th />
            {columns.map((c) => (
              <th
                key={c}
                className="num px-1 pb-1 text-left text-[9.5px] font-normal text-[color:var(--color-ink-faint)]"
              >
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r}>
              <th className="num pr-2 text-right text-[9.5px] font-normal text-[color:var(--color-ink-faint)]">
                {r}
              </th>
              {columns.map((c) => {
                const v = at(r, c);
                const style =
                  v === null
                    ? {
                        background: "transparent",
                        border: "1px dashed var(--color-line-strong)",
                      }
                    : {
                        background:
                          v >= 0
                            ? `color-mix(in srgb, var(--color-up) ${Math.round(Math.abs(v) * 72)}%, transparent)`
                            : `color-mix(in srgb, var(--color-down) ${Math.round(Math.abs(v) * 72)}%, transparent)`,
                        border: "1px solid transparent",
                      };
                return (
                  <td
                    key={c}
                    title={v === null ? `${r} × ${c}: not measured` : `${r} × ${c}: ${v.toFixed(2)}`}
                    className="num h-[26px] w-[42px] text-center text-[9.5px] text-[color:var(--color-ink)]"
                    style={style}
                  >
                    {v === null ? "" : v.toFixed(2)}
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
