"use client";

import { Chip } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { SimulatedBanner } from "@/components/data/Simulated";
import { AreaChart, Gauge } from "@/components/viz/primitives";
import { simBetween, simWalk } from "@/lib/sim";

/**
 * Market predictions, illustrated — because the surface does not exist yet.
 *
 * Everything on this page is generated from fixed seeds and says so at the
 * top. Two protections keep a screenshot of it from ever reading as a real
 * forecast: the instruments are fictional (no ticker below trades anywhere),
 * and the figures are deterministic, identical on every load, so nothing here
 * can appear to move. The universe is instruments only — no P&L, no position,
 * no figure that could read as money the desk owns.
 */

interface SimulatedPrediction {
  /** A fictional instrument. Deliberately unlike any real ticker. */
  readonly instrument: string;
  readonly sector: string;
  readonly horizon: string;
  /** Forecast path in fictional index points, oldest first. */
  readonly path: readonly number[];
  /** Stated confidence in [0, 1] — stated by the generator, earned by nothing. */
  readonly confidence: number;
  readonly direction: "up" | "down" | "flat";
}

const UNIVERSE = [
  { instrument: "EQ-AURORA", sector: "equity factor basket", horizon: "5 sessions" },
  { instrument: "EQ-BOREAL", sector: "equity dispersion basket", horizon: "5 sessions" },
  { instrument: "FX-KESTREL", sector: "fx carry pair", horizon: "3 sessions" },
  { instrument: "FX-MERIDIAN", sector: "fx trade-flow pair", horizon: "3 sessions" },
  { instrument: "CR-ORRERY", sector: "credit spread index", horizon: "10 sessions" },
  { instrument: "CM-THALASSA", sector: "commodity forward strip", horizon: "10 sessions" },
] as const;

// Module scope on purpose: the same seeds produce the same figures on the
// server render and every client render, so nothing can drift between loads
// and invite a reader to see movement where there is none.
const PREDICTIONS: readonly SimulatedPrediction[] = UNIVERSE.map((entry) => {
  const drift = simBetween(`predictions:drift:${entry.instrument}`, -0.004, 0.004);
  const path = simWalk(`predictions:path:${entry.instrument}`, 48, {
    start: 100,
    drift,
    volatility: 0.012,
  });
  const first = path[0] ?? 100;
  const last = path[path.length - 1] ?? 100;
  const direction: SimulatedPrediction["direction"] =
    last > first * 1.002 ? "up" : last < first * 0.998 ? "down" : "flat";
  return {
    instrument: entry.instrument,
    sector: entry.sector,
    horizon: entry.horizon,
    path,
    confidence: simBetween(`predictions:confidence:${entry.instrument}`, 0.4, 0.9),
    direction,
  };
});

const DIRECTION_LABEL: Record<SimulatedPrediction["direction"], string> = {
  up: "projected higher",
  down: "projected lower",
  flat: "projected flat",
};

export default function PredictionsPage() {
  return (
    <div className="flex flex-col gap-3 p-3">
      <SimulatedBanner subject="market predictions" contract="GET /api/v1/predictions">
        <p className="max-w-[80ch] text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
          Every instrument below is fictional — none of these tickers trades anywhere — so a
          screenshot of this page can never be mistaken for a forecast of a real market. The
          prediction machinery itself is real and runs in-process in{" "}
          <code className="num">backend/crates/services/qip-prediction</code>; no HTTP surface exposes its
          output yet.
        </p>
      </SimulatedBanner>

      <div className="grid grid-cols-1 gap-3 lg:grid-cols-2 xl:grid-cols-3">
        {PREDICTIONS.map((prediction) => (
          <Panel key={prediction.instrument}>
            <PanelHead
              title={prediction.instrument}
              meta={<Chip tone="info">{prediction.sector}</Chip>}
              actions={
                <Chip
                  tone={
                    prediction.direction === "up"
                      ? "ok"
                      : prediction.direction === "down"
                        ? "bad"
                        : "neutral"
                  }
                >
                  {DIRECTION_LABEL[prediction.direction]} · {prediction.horizon}
                </Chip>
              }
            />
            <PanelBody>
              <div className="flex flex-col gap-3">
                <AreaChart
                  values={prediction.path}
                  height={120}
                  label={`${prediction.instrument} forecast path`}
                  tone={
                    prediction.direction === "up"
                      ? "up"
                      : prediction.direction === "down"
                        ? "down"
                        : "accent"
                  }
                  caption="generated from a fixed seed"
                />
                <div className="flex items-center justify-between gap-3">
                  <Gauge
                    fraction={prediction.confidence}
                    label="confidence"
                    caption="generated from a fixed seed"
                    tone="accent"
                    size={84}
                  />
                  <p className="max-w-[32ch] text-[10.5px] leading-relaxed text-[color:var(--color-ink-faint)]">
                    A path in fictional index points over {prediction.horizon}. It is an
                    illustration of what the contract would carry, not an output any model
                    produced.
                  </p>
                </div>
              </div>
            </PanelBody>
          </Panel>
        ))}
      </div>
    </div>
  );
}
