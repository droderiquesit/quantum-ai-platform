"use client";

import { Chip, Freshness } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView, StateBlock } from "@/components/data/States";
import { SimulatedBanner } from "@/components/data/Simulated";
import { AreaChart } from "@/components/viz/primitives";
import { platform } from "@/lib/api/client";
import type { Strategies } from "@/lib/api/types";
import { formatCount } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";
import { simWalk } from "@/lib/sim";

/**
 * The backtester, seen from the one angle the console can actually see it:
 * which candidates have cleared it.
 *
 * The simulator itself is real Rust — the Deep Brain runs every candidate
 * through it and gates promotion on the outcome — but it serves no HTTP
 * surface, so no report can be fetched. Rendering a plausible report anyway
 * would put a fabricated performance record on a research screen, which is the
 * failure this page is built to refuse. What it shows instead is the truth
 * about the gap, the ladder evidence the platform does serve, and one
 * illustration that says on its face it is one.
 */

/**
 * The promotion gate rungs, weakest evidence first — the same ladder the
 * strategies page renders. A rung the platform sends that is not listed here
 * still appears in the table, as itself, so a rung added upstream shows up
 * rather than disappearing.
 */
const LADDER = ["proposed", "backtested", "paper", "shadow", "live_capped", "retired"] as const;

/**
 * A candidate can only stand on these rungs after the backtest gate held
 * evidence for it. "proposed" sits before the gate, and "retired" records an
 * exit rather than evidence — a candidate can retire from any rung — so
 * neither is counted as backtest evidence, on either side.
 */
const EVIDENCE_STAGES: ReadonlySet<string> = new Set(["backtested", "paper", "shadow", "live_capped"]);

function evidenceChip(stage: string) {
  if (EVIDENCE_STAGES.has(stage)) return <Chip tone="ok">backtest evidence held</Chip>;
  if (stage === "proposed") return <Chip tone="neutral">before the gate</Chip>;
  if (stage === "retired") return <Chip tone="neutral">exit, evidence not recorded</Chip>;
  return <Chip tone="warn">rung unknown to this console</Chip>;
}

/**
 * Computed once at module load from a fixed seed, so the illustration is
 * identical on every render, every load, every machine. An illustration that
 * moved would invite the reader to see a backtest running where none is.
 * The name is fictional; no candidate on the ladder is called this.
 */
const ILLUSTRATION = simWalk("backtest-illustration:harbour-lantern", 120, {
  start: 100,
  drift: 0.0012,
  volatility: 0.018,
  floor: 1,
});

export default function BacktestingPage() {
  const strategies = useResource<Strategies>(platform.strategies, {
    key: "backtesting-strategies",
    label: "GET /strategies",
    intervalMs: 20_000,
  });

  const candidates = strategies.data?.strategies ?? [];
  const withEvidence = candidates.filter((candidate) => EVIDENCE_STAGES.has(candidate.stage));
  const beforeGate = candidates.filter((candidate) => candidate.stage === "proposed");
  const unaccounted = candidates.length - withEvidence.length - beforeGate.length;

  const byStage = new Map<string, number>();
  for (const candidate of candidates) {
    byStage.set(candidate.stage, (byStage.get(candidate.stage) ?? 0) + 1);
  }
  const extraStages = [...byStage.keys()]
    .filter((stage) => !LADDER.includes(stage as (typeof LADDER)[number]))
    .sort();

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead title="Where the backtester lives" actions={<Chip tone="warn">no endpoint</Chip>} />
        <PanelBody>
          <StateBlock
            tone="warn"
            label="no http surface"
            headline="The backtester is real, and this console cannot reach it."
          >
            <p>
              The Deep Brain runs every strategy candidate through the backtest simulator inside its
              evolution loop and gates promotion on the outcome. That code is Rust, in a separate
              process, and it serves no HTTP surface — there is no report to fetch. The contract this
              page is written against is <code className="num">GET /api/v1/backtests</code>; when the
              platform serves it, real reports render here.
            </p>
            <p className="mt-1.5 text-[color:var(--color-ink-faint)]">
              Until then, nothing on this page is a backtest result. The only measured facts below
              are the ladder stages the platform does serve.
            </p>
          </StateBlock>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Backtest evidence on the ladder"
          meta={<Freshness resource={strategies} name="strategies" />}
          actions={<Chip>GET /api/v1/strategies</Chip>}
        />
        <PanelBody>
          <ResourceView resource={strategies} loadingRows={5}>
            {(data) =>
              data.strategies.length === 0 ? (
                <EmptyBlock headline="The ladder is empty, so nothing has been backtested.">
                  <p>
                    <code className="num">GET /api/v1/strategies</code> answered with an empty list —
                    a measured zero, not a failed read. No candidate is registered with the promotion
                    gate in this deployment, so there is no candidate the backtester could have
                    passed or failed.
                  </p>
                </EmptyBlock>
              ) : (
                <div className="flex flex-col gap-3">
                  <KpiRow>
                    <Kpi
                      label="Candidates"
                      value={formatCount(data.strategies.length)}
                      note="registered with the promotion gate"
                    />
                    <Kpi
                      label="Backtest evidence held"
                      value={formatCount(withEvidence.length)}
                      tone={withEvidence.length > 0 ? "ok" : "neutral"}
                      note="standing on a rung only the backtester grants"
                    />
                    <Kpi
                      label="Before the gate"
                      value={formatCount(beforeGate.length)}
                      note="proposed, not yet run through the simulator"
                    />
                    <Kpi
                      label="Evidence not recorded"
                      value={formatCount(unaccounted)}
                      note="retired or on a rung this console does not know"
                    />
                  </KpiRow>
                  <TableWell maxHeight="320px" label="The promotion ladder">
                    <table className="dt">
                      <thead>
                        <tr>
                          <th scope="col">Rung</th>
                          <th scope="col" className="n">
                            Candidates
                          </th>
                          <th scope="col">What the rung says about the backtest</th>
                        </tr>
                      </thead>
                      <tbody>
                        {[...LADDER, ...extraStages].map((rung, index) => (
                          <tr key={rung}>
                            <td className="num">
                              {index < LADDER.length ? `${index + 1}. ${rung}` : rung}
                            </td>
                            <td className="n">{formatCount(byStage.get(rung) ?? 0)}</td>
                            <td>{evidenceChip(rung)}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </TableWell>
                </div>
              )
            }
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="What a report will render" actions={<Chip tone="neutral">illustration</Chip>} />
        <PanelBody>
          <div className="flex flex-col gap-3">
            <SimulatedBanner subject="backtest report" contract="GET /api/v1/backtests">
              <p className="max-w-[80ch] text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
                The curve below is a generated illustration of the equity-curve panel a real backtest
                report will occupy. It describes no strategy, no market and no period; the name is
                fictional. It exists so the shape of the page is reviewable before the surface does.
              </p>
            </SimulatedBanner>
            <AreaChart
              values={ILLUSTRATION}
              label="illustrative equity curve for the fictional candidate harbour-lantern"
              height={160}
              caption={<>illustration, fixed seed, not a result</>}
            />
          </div>
        </PanelBody>
      </Panel>
    </div>
  );
}
