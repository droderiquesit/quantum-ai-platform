"use client";

import { Chip, Freshness } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView, UnavailableBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { Predictions, RecordedPrediction } from "@/lib/api/types";
import { isUnavailable } from "@/lib/api/types";
import { formatCount, formatTimestamp } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * Every falsifiable claim the REASON stage wrote down, as the platform serves
 * it, and whether LEARN has yet learned anything from them.
 *
 * This page used to be a seeded illustration under a SIMULATED DATA banner,
 * because no route carried the claims. `GET /api/v1/predictions` now does,
 * grouped by instrument in the platform's own key order, so the illustration
 * is gone and nothing on this page is generated here: the direction is the
 * one the claim stated ("unstated" where it stated none), the confidence is
 * the number written down at the time, and the verdict is LEARN's. With no
 * claim recorded, the page says so and shows the cycle count beside it, so
 * "nothing predicted" reads as a fact about the loop rather than a page that
 * has not loaded.
 */

const STATE_TONE: Record<RecordedPrediction["state"], "neutral" | "ok" | "bad" | "warn"> = {
  open: "neutral",
  held: "ok",
  failed: "bad",
  undetermined: "warn",
};

const DIRECTION_LABEL: Record<RecordedPrediction["direction"], string> = {
  up: "up",
  down: "down",
  unstated: "unstated",
};

function formatHorizon(seconds: number): string {
  if (seconds >= 86_400) return `${(seconds / 86_400).toFixed(1)} d`;
  if (seconds >= 3_600) return `${(seconds / 3_600).toFixed(1)} h`;
  if (seconds >= 60) return `${(seconds / 60).toFixed(0)} min`;
  return `${seconds} s`;
}

function formatConfidence(confidence: number | null): string {
  return confidence === null ? "—" : confidence.toFixed(2);
}

function formatBps(bps: number | null): string {
  return bps === null ? "—" : `${bps > 0 ? "+" : ""}${bps.toFixed(0)} bps`;
}

export default function PredictionsPage() {
  const predictions = useResource<Predictions>(platform.predictions, {
    key: "predictions",
    label: "GET /predictions",
    intervalMs: 20_000,
  });

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Recorded claims"
          meta={<Freshness resource={predictions} name="predictions" />}
          actions={<Chip>GET /api/v1/predictions</Chip>}
        />
        <PanelBody>
          <ResourceView resource={predictions} loadingRows={5}>
            {(data) => {
              const instruments = Object.entries(data.instruments);
              return (
                <div className="flex flex-col gap-3">
                  <KpiRow>
                    <Kpi label="As of cycle" value={formatCount(data.as_of_cycle)} note="cycles run by this process" />
                    <Kpi
                      label="Claims held"
                      value={formatCount(data.held)}
                      note={`of a working set bounded at ${formatCount(data.window)}`}
                    />
                    <Kpi label="Open" value={formatCount(data.open)} note="horizon not yet passed" />
                    <Kpi
                      label="Resolved"
                      value={formatCount(data.resolved)}
                      tone={data.resolved > 0 ? "ok" : "neutral"}
                      note="graded by the LEARN stage"
                    />
                  </KpiRow>
                  {instruments.length === 0 ? (
                    <EmptyBlock headline="The loop has written down no claim.">
                      <p data-testid="predictions-empty">
                        <code className="num">GET /api/v1/predictions</code> answered with no
                        instrument after {formatCount(data.as_of_cycle)} cycle(s) — a measured zero,
                        not a failed read. The REASON stage records a claim only when it convenes on
                        an opportunity, and this process has convened on none. Nothing is shown in
                        its place.
                      </p>
                    </EmptyBlock>
                  ) : (
                    <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
                      {instruments.map(([instrument, entry]) => (
                        <Panel key={instrument} data-testid="prediction-instrument">
                          <PanelHead
                            title={instrument}
                            meta={<Chip tone="info">{formatCount(entry.predictions.length)} claim(s)</Chip>}
                          />
                          <PanelBody flush>
                            <TableWell maxHeight="360px" label={`Claims about ${instrument}`}>
                              <table className="dt">
                                <thead>
                                  <tr>
                                    <th scope="col">Verdict</th>
                                    <th scope="col">Direction</th>
                                    <th scope="col" className="n">
                                      Confidence
                                    </th>
                                    <th scope="col" className="n">
                                      Expected move
                                    </th>
                                    <th scope="col">Horizon</th>
                                    <th scope="col">Made at</th>
                                    <th scope="col">Resolves at</th>
                                    <th scope="col">Scored at</th>
                                    <th scope="col">Statement</th>
                                  </tr>
                                </thead>
                                <tbody>
                                  {entry.predictions.map((claim) => (
                                    <tr
                                      key={`${claim.hypothesis}:${claim.cycle}:${claim.made_at}`}
                                      data-testid="prediction-row"
                                    >
                                      <td>
                                        <Chip tone={STATE_TONE[claim.state] ?? "warn"}>{claim.state}</Chip>
                                      </td>
                                      <td className="num">{DIRECTION_LABEL[claim.direction] ?? claim.direction}</td>
                                      <td className="n">{formatConfidence(claim.confidence)}</td>
                                      <td className="n">{formatBps(claim.expected_move_bps)}</td>
                                      <td className="num">{formatHorizon(claim.horizon_seconds)}</td>
                                      <td className="num">{formatTimestamp(claim.made_at)}</td>
                                      <td className="num">{formatTimestamp(claim.resolves_at)}</td>
                                      <td className="num">
                                        {claim.scored_at === null ? "—" : formatTimestamp(claim.scored_at)}
                                      </td>
                                      <td className="max-w-[48ch] text-[11px] text-[color:var(--color-ink-dim)]">
                                        {claim.statement}
                                        <span className="block text-[10px] text-[color:var(--color-ink-faint)]">
                                          settles on <code className="num">{claim.metric}</code> · cycle{" "}
                                          {formatCount(claim.cycle)}
                                        </span>
                                      </td>
                                    </tr>
                                  ))}
                                </tbody>
                              </table>
                            </TableWell>
                          </PanelBody>
                        </Panel>
                      ))}
                    </div>
                  )}
                  <p className="text-[11px] text-[color:var(--color-ink-faint)]">
                    Instruments are listed in the order the platform serves them. A confidence is the
                    figure the claim was written with; whether it was earned is the calibration
                    below, not this table.
                  </p>
                </div>
              );
            }}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Calibration" actions={<Chip>calibration</Chip>} />
        <PanelBody>
          <ResourceView resource={predictions} loadingRows={3}>
            {(data) =>
              isUnavailable(data.calibration) ? (
                <div data-testid="calibration-unavailable">
                  <UnavailableBlock subject={data.calibration.subject} reason={data.calibration.reason} />
                </div>
              ) : (
                <div className="flex flex-col gap-3" data-testid="calibration-report">
                  <KpiRow>
                    <Kpi
                      label="Evaluations in window"
                      value={formatCount(data.calibration.evaluations_in_window)}
                      note="claims LEARN has graded"
                    />
                    <Kpi
                      label="Evaluated"
                      value={formatCount(data.calibration.report.evaluated ?? null)}
                      note="in the report"
                    />
                    <Kpi
                      label="Brier score"
                      value={
                        typeof data.calibration.report.brier_score === "number"
                          ? data.calibration.report.brier_score.toFixed(4)
                          : "—"
                      }
                      note="lower is better calibrated"
                    />
                    <Kpi
                      label="Material"
                      value={data.calibration.material ? "yes" : "no"}
                      tone={data.calibration.material ? "ok" : "neutral"}
                      note="whether the report clears the platform's evidence floor"
                    />
                  </KpiRow>
                  <dl className="grid grid-cols-1 gap-x-6 md:grid-cols-2">
                    {Object.entries(data.calibration.report)
                      .filter(([key]) => key !== "evaluated" && key !== "brier_score")
                      .map(([key, value]) => (
                        <div
                          key={key}
                          className="flex items-baseline justify-between gap-4 border-b border-[color:var(--color-line)] py-1"
                        >
                          <dt className="text-[11px] text-[color:var(--color-ink-dim)]">{key}</dt>
                          <dd className="num text-[11px]">{typeof value === "object" ? JSON.stringify(value) : String(value)}</dd>
                        </div>
                      ))}
                  </dl>
                </div>
              )
            }
          </ResourceView>
        </PanelBody>
      </Panel>
    </div>
  );
}
