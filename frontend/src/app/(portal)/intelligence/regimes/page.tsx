"use client";

import { Chip } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { SimulatedBanner } from "@/components/data/Simulated";
import type { HeatCell } from "@/components/viz/primitives";
import { Bars, Gauge, Heatmap } from "@/components/viz/primitives";
import { simBetween, simPick } from "@/lib/sim";

/**
 * Regime detection, illustrated — the surface does not exist yet.
 *
 * The transition matrix is generated under the one constraint that makes a
 * transition matrix a transition matrix: each row is normalised so its
 * probabilities sum to one (to within display rounding). A row summing to 1.3
 * would not be a simplification, it would be an object that cannot exist, and
 * a reader who noticed would rightly stop trusting the page. The diagonal is
 * weighted up because regimes persist — an illustration where the market
 * reshuffles its regime every step would teach the wrong intuition about what
 * the real surface will show.
 */

const REGIMES = [
  { code: "RB", name: "range-bound / low volatility" },
  { code: "TR", name: "trending / expanding volatility" },
  { code: "ST", name: "stressed / correlated selloff" },
  { code: "RO", name: "rotational / dispersion" },
] as const;

type Regime = (typeof REGIMES)[number];

const CURRENT: Regime = simPick("regimes:current", REGIMES);
const CONFIDENCE = simBetween("regimes:confidence", 0.4, 0.9);

/** One row of the transition matrix, normalised to sum to 1 before rounding. */
function transitionRow(from: Regime): readonly number[] {
  const weights = REGIMES.map(
    (to) =>
      // Persistence bias: staying put outweighs any single move.
      (to.code === from.code ? 2.2 : 0) +
      simBetween(`regimes:transition:${from.code}:${to.code}`, 0.05, 1),
  );
  const total = weights.reduce((sum, weight) => sum + weight, 0);
  return weights.map((weight) => Math.round((weight / total) * 100) / 100);
}

const MATRIX: readonly (readonly number[])[] = REGIMES.map((from) => transitionRow(from));

const CELLS: readonly HeatCell[] = REGIMES.flatMap((from, i) =>
  REGIMES.map((to, j) => ({
    row: from.name,
    column: to.code,
    value: MATRIX[i]?.[j] ?? null,
  })),
);

const CURRENT_INDEX = REGIMES.findIndex((regime) => regime.code === CURRENT.code);
const NEXT_LIKELY = REGIMES.map((to, j) => ({
  label: to.name,
  value: MATRIX[CURRENT_INDEX]?.[j] ?? 0,
  tone: to.code === CURRENT.code ? ("flat" as const) : ("accent" as const),
})).sort((a, b) => b.value - a.value);

export default function RegimesPage() {
  return (
    <div className="flex flex-col gap-3 p-3">
      <SimulatedBanner subject="regime detection" contract="GET /api/v1/regimes">
        <p className="max-w-[80ch] text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
          The machinery that would ground this page is real: the world model in{" "}
          <code className="num">crates/services/qip-world-model</code> tracks the believed state
          of the world and what changed between any two instants, in-process. No HTTP surface
          exposes a regime classification yet, so everything below is a seeded illustration of
          the contract.
        </p>
      </SimulatedBanner>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[2fr_3fr]">
        <Panel>
          <PanelHead
            title="Current regime"
            actions={<Chip tone="info">generated from a fixed seed</Chip>}
          />
          <PanelBody>
            <div className="flex flex-col gap-4">
              <div className="flex flex-wrap items-center gap-4">
                <p className="num text-[19px] font-semibold text-[color:var(--color-ink)]">
                  {CURRENT.name}
                </p>
                <Gauge
                  fraction={CONFIDENCE}
                  label="classification confidence"
                  caption="generated from a fixed seed"
                  tone="accent"
                  size={92}
                />
              </div>
              <div className="flex flex-col gap-1.5">
                <span className="eyebrow">most likely next regime</span>
                <Bars items={NEXT_LIKELY} />
                <p className="text-[10px] text-[color:var(--color-ink-faint)]">
                  The current-regime row of the matrix, largest first. Staying put is shown grey
                  so a glance ranks the moves, not the persistence.
                </p>
              </div>
            </div>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead title="Transition matrix" />
          <PanelBody>
            <div className="flex flex-col gap-3">
              <Heatmap
                cells={CELLS}
                rows={REGIMES.map((regime) => regime.name)}
                columns={REGIMES.map((regime) => regime.code)}
                label="Simulated regime transition probabilities"
              />
              <div className="max-w-[80ch] text-[11px] leading-relaxed text-[color:var(--color-ink-dim)]">
                <p>
                  Rows are the regime the market is in; columns are the regime it moves to next,
                  keyed{" "}
                  {REGIMES.map((regime, index) => (
                    <span key={regime.code}>
                      <span className="num">{regime.code}</span> = {regime.name}
                      {index < REGIMES.length - 1 ? ", " : "."}
                    </span>
                  ))}{" "}
                  Each row is normalised to sum to one before rounding, so a row of the
                  two-decimal figures shown may read 0.99 or 1.01. Depth of colour is
                  probability; the heavy diagonal is deliberate, because regimes persist.
                </p>
              </div>
            </div>
          </PanelBody>
        </Panel>
      </div>
    </div>
  );
}
