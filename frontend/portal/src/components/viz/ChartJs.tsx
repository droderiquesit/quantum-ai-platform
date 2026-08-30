"use client";

import { useEffect, useRef } from "react";
import type { ChartConfiguration } from "chart.js";

/**
 * Chart.js, themed by the design tokens and safe under the theme toggle.
 *
 * The template's charts are Chart.js (ADR 0015), and its README's own rule is
 * that charts re-colour on theme change rather than being rebuilt blind. This
 * wrapper does the equivalent for React: one chart instance per canvas,
 * destroyed on unmount (Chart.js leaks canvases otherwise), rebuilt when the
 * theme attribute flips so colours resolved from CSS variables are re-read.
 *
 * Colours in `config` may be written as `var(--color-…)` strings; they are
 * resolved against the live computed style here, because Chart.js paints into
 * a canvas and a canvas cannot see CSS variables.
 */
export function resolveTokens<T>(value: T): T {
  if (typeof value === "string" && value.startsWith("var(")) {
    const name = value.slice(4, -1).trim();
    const resolved = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    return (resolved || value) as T;
  }
  if (Array.isArray(value)) return value.map(resolveTokens) as T;
  if (value && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, entry] of Object.entries(value)) out[key] = resolveTokens(entry);
    return out as T;
  }
  return value;
}

export function ChartJs({
  config,
  className,
  height = 260,
  label,
}: {
  readonly config: ChartConfiguration;
  readonly className?: string;
  readonly height?: number;
  /** Accessible name; a canvas is a bitmap and says nothing on its own. */
  readonly label: string;
}) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;

    let disposed = false;
    let chart: { destroy(): void } | null = null;

    const build = async () => {
      const { Chart } = await import("chart.js/auto");
      if (disposed || !canvasRef.current) return;
      chart?.destroy();
      chart = new Chart(canvasRef.current, resolveTokens(config));
    };
    void build();

    // Rebuild when the theme flips: colours were resolved from variables at
    // construction and a repaint alone would keep the old theme's ink.
    const observer = new MutationObserver(() => void build());
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });

    return () => {
      disposed = true;
      observer.disconnect();
      chart?.destroy();
    };
  }, [config]);

  return (
    <div className={className} style={{ position: "relative", height }}>
      <canvas ref={canvasRef} role="img" aria-label={label} />
    </div>
  );
}
