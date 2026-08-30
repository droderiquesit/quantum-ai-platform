"use client";

import { useEffect, useRef, useState } from "react";

/**
 * A history of a value this console has watched, kept in the browser.
 *
 * The platform serves counters, not curves: `/system/metrics` says how many
 * cycles have run, never how that number moved. So a chart of it has to come
 * from somewhere, and there are only two honest options — add a backend that
 * records the series, or record what this console itself observed and say so.
 *
 * This is the second. Every series it produces is captioned with the instant of
 * its first observation, because a chart labelled "cycles" that silently began
 * ninety seconds ago is a chart that lies about its own window. The moment a
 * platform-side history exists, this should be replaced by it: a client-side
 * buffer is lost on reload, is per-tab, and can never describe anything that
 * happened before someone opened the page.
 *
 * Deliberately not persisted to `localStorage`. A series stitched across
 * reloads would have gaps where the tab was closed and no way to show them, and
 * a broken line drawn as a continuous one is the worst of both.
 */

export interface Series {
  /** Oldest first. Bounded by `cap`. */
  readonly values: readonly number[];
  /** When the first retained observation was taken. */
  readonly since: number | null;
  /** Observations taken, including ones already dropped off the front. */
  readonly observed: number;
}

const EMPTY: Series = { values: [], since: null, observed: 0 };

/**
 * Append `value` whenever it changes identity, keeping at most `cap` points.
 *
 * `null` and non-finite values are skipped rather than recorded as zero: a
 * failed poll is not an observation of nothing, and charting it as zero would
 * put a cliff in the line every time the platform was briefly unreachable.
 */
export function useSeries(value: number | null | undefined, cap = 60): Series {
  const [series, setSeries] = useState<Series>(EMPTY);
  // The last value appended, held in a ref so re-renders that do not change it
  // cannot push a duplicate point and stretch the window with flat noise.
  const last = useRef<number | null>(null);

  useEffect(() => {
    if (value === null || value === undefined || !Number.isFinite(value)) return;
    if (last.current === value) return;
    last.current = value;
    setSeries((previous) => {
      const values = [...previous.values, value].slice(-cap);
      return {
        values,
        since: previous.since ?? Date.now(),
        observed: previous.observed + 1,
      };
    });
  }, [value, cap]);

  return series;
}

/** How the caption should describe a series' window. */
export function describeWindow(series: Series): string {
  if (series.since === null) return "nothing observed yet";
  const seconds = Math.max(1, Math.round((Date.now() - series.since) / 1000));
  const window =
    seconds < 90
      ? `${seconds}s`
      : seconds < 5400
        ? `${Math.round(seconds / 60)}m`
        : `${Math.round(seconds / 3600)}h`;
  const dropped = series.observed - series.values.length;
  return `${series.values.length} pt over ${window} observed here${dropped > 0 ? ` · ${dropped} aged out` : ""}`;
}
