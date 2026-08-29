"use client";

import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { SimulatedBanner } from "@/components/data/Simulated";
import type { HeatCell } from "@/components/viz/primitives";
import { Heatmap } from "@/components/viz/primitives";
import { simBetween } from "@/lib/sim";

/**
 * Cross-market correlation, illustrated — the surface does not exist yet.
 *
 * The matrix is generated, and generated carefully: each pair is seeded on the
 * *sorted* pair key, so A×B and B×A are the same number by construction rather
 * than by luck, and the diagonal is 1.0 by definition. An asymmetric
 * correlation matrix would not be a simplification — it would be an object
 * that cannot exist, and a reader who knows that would rightly distrust the
 * rest of the console. Fictional instruments only, so no cell here can be read
 * as a claim about how two real markets move together.
 */

const INSTRUMENTS = [
  "EQ-AURORA",
  "EQ-BOREAL",
  "FX-KESTREL",
  "FX-MERIDIAN",
  "CR-ORRERY",
  "CM-THALASSA",
] as const;

function pairwise(a: string, b: string): number {
  if (a === b) return 1;
  // Sorting the key is what makes the matrix symmetric: both orderings of the
  // pair reach the same seed and therefore the same value.
  const key = [a, b].sort().join("|");
  return Math.round(simBetween(`correlation:${key}`, -0.85, 0.9) * 100) / 100;
}

const CELLS: readonly HeatCell[] = INSTRUMENTS.flatMap((row) =>
  INSTRUMENTS.map((column) => ({ row, column, value: pairwise(row, column) })),
);

interface Pair {
  readonly a: string;
  readonly b: string;
  readonly value: number;
}

const PAIRS: readonly Pair[] = INSTRUMENTS.flatMap((a, i) =>
  INSTRUMENTS.slice(i + 1).map((b) => ({ a, b, value: pairwise(a, b) })),
).sort((x, y) => Math.abs(y.value) - Math.abs(x.value));

export default function CorrelationPage() {
  return (
    <div className="flex flex-col gap-3 p-3">
      <SimulatedBanner subject="cross-market correlation" contract="GET /api/v1/correlation" />

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[3fr_2fr]">
        <Panel>
          <PanelHead title="Pairwise correlation" />
          <PanelBody>
            <div className="flex flex-col gap-3">
              <Heatmap
                cells={CELLS}
                rows={[...INSTRUMENTS]}
                columns={[...INSTRUMENTS]}
                label="Simulated pairwise correlation across the fictional universe"
              />
              <div className="max-w-[80ch] text-[11px] leading-relaxed text-[color:var(--color-ink-dim)]">
                <p>
                  <span className="text-[color:var(--color-up)]">Green</span> is positive: the pair
                  tends to move together, and hedging one with the other works.{" "}
                  <span className="text-[color:var(--color-down)]">Red</span> is negative: the pair
                  tends to move opposite ways, and holding both dampens the book. Intensity is
                  strength; the diagonal is 1.00 by definition, not by measurement. A dashed cell
                  would mean a pair nobody measured — different from a pair measured at zero —
                  though every pair in this illustration carries a value.
                </p>
              </div>
            </div>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead title="Strongest pairs" />
          <PanelBody flush>
            <TableWell maxHeight="380px" label="Simulated correlation pairs, strongest first">
              <table className="dt">
                <thead>
                  <tr>
                    <th scope="col">Pair</th>
                    <th scope="col" className="n">
                      Correlation
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {PAIRS.map((pair) => (
                    <tr key={`${pair.a}:${pair.b}`}>
                      <td className="num">
                        {pair.a} × {pair.b}
                      </td>
                      <td
                        className="n"
                        data-direction={
                          pair.value > 0 ? "positive" : pair.value < 0 ? "negative" : "flat"
                        }
                      >
                        {pair.value.toFixed(2)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </TableWell>
            <p className="px-3 py-2 text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
              Ranked by magnitude, sign preserved. Generated from a fixed seed — identical on
              every load.
            </p>
          </PanelBody>
        </Panel>
      </div>
    </div>
  );
}
