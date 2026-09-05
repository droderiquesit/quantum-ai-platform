"use client";

import Link from "next/link";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Chip, Freshness, StatusChip, StreamControls } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import {
  isUnavailable,
  type CycleStage,
  type Fills,
  type MeshStatus,
  type Opportunities,
  type Orders,
  type Portfolio,
  type Predictions,
  type Proposals,
  type Regions,
  type SystemMetrics,
  type SystemStatus,
  type SystemView,
} from "@/lib/api/types";
import { formatClock, formatCount, formatDecimal } from "@/lib/format";
import { useCycleReports } from "@/lib/hooks/useCycleReports";
import { useEventStream } from "@/lib/hooks/useEventStream";
import { useResource, type Resource } from "@/lib/hooks/useResource";

/**
 * The loop as a flow: eight stages left to right, and what moved along each
 * edge on the latest cycle.
 *
 * Every figure on this page is one of three things — a field a route answered,
 * a field the health stream carried, or a sentence from the cycle report the
 * platform returned to this console — and each panel names which. Where a
 * route answers `available: false`, its reason is rendered in the platform's
 * own words. Nothing here is computed from a number the platform did not
 * state, and there is no control on this page that advances the loop: the one
 * that does lives on the loop page and is linked, not duplicated.
 *
 * The stage detail has exactly one source. The platform serves a cycle's
 * stages only in the response to the `POST /cycle` that ran it, so this page
 * reads the last report this tab received (`useCycleReports`) and says which
 * cycle it was — and says so again, loudly, when the platform's own cycle
 * count has moved past it. A page that showed cycle 4's sentences under a
 * cycle-9 heading would be attributing what the loop did to the wrong run.
 */

/** The loop's stages, in the order the kernel traverses them. */
const STAGES = [
  "sense",
  "understand",
  "discover",
  "reason",
  "simulate",
  "decide",
  "act",
  "learn",
] as const;

type StageName = (typeof STAGES)[number];

const POLL_MS = 15_000;

export default function DataflowPage() {
  const status = useResource<SystemStatus>(platform.systemStatus, {
    key: "dataflow-status",
    label: "GET /system/status",
    intervalMs: POLL_MS,
  });
  const system = useResource<SystemView>(platform.system, {
    key: "dataflow-system",
    label: "GET /system",
    intervalMs: POLL_MS,
  });
  const metrics = useResource<SystemMetrics>(platform.systemMetrics, {
    key: "dataflow-metrics",
    label: "GET /system/metrics",
    intervalMs: POLL_MS,
  });
  const opportunities = useResource<Opportunities>(platform.opportunities, {
    key: "dataflow-opportunities",
    label: "GET /opportunities",
    intervalMs: POLL_MS,
  });
  const proposals = useResource<Proposals>(platform.proposals, {
    key: "dataflow-proposals",
    label: "GET /proposals",
    intervalMs: POLL_MS,
  });
  const orders = useResource<Orders>(platform.orders, {
    key: "dataflow-orders",
    label: "GET /orders",
    intervalMs: POLL_MS,
  });
  const fills = useResource<Fills>(platform.fills, {
    key: "dataflow-fills",
    label: "GET /fills",
    intervalMs: POLL_MS,
  });
  const portfolio = useResource<Portfolio>(platform.portfolio, {
    key: "dataflow-portfolio",
    label: "GET /portfolio",
    intervalMs: POLL_MS,
  });
  const pnl = useResource<unknown>(platform.pnl, {
    key: "dataflow-pnl",
    label: "GET /pnl",
    intervalMs: POLL_MS,
  });
  const predictions = useResource<Predictions>(platform.predictions, {
    key: "dataflow-predictions",
    label: "GET /predictions",
    intervalMs: POLL_MS,
  });
  const mesh = useResource<MeshStatus>(platform.mesh, {
    key: "dataflow-mesh",
    label: "GET /mesh",
    intervalMs: POLL_MS,
  });
  const regions = useResource<Regions>(platform.regions, {
    key: "dataflow-regions",
    label: "GET /regions",
    intervalMs: POLL_MS,
  });

  const health = useEventStream({ channel: "health", label: "SSE /stream/health", maxEvents: 60 });

  // The newest `cycles` the health stream carried. `health.changed` publishes
  // the same reading `GET /health` computes plus the counters; `cycles` is
  // one of them, read off the platform's own state.
  const streamCycles = useMemo<number | null>(() => {
    for (const event of health.events) {
      const cycles = event.payload["cycles"];
      if (typeof cycles === "number" && Number.isFinite(cycles)) return cycles;
    }
    return null;
  }, [health.events]);

  // Every refresh function is stable, so listing them is listing constants;
  // one callback so the effect below has one dependency to name.
  const { refresh: refreshStatus } = status;
  const { refresh: refreshSystem } = system;
  const { refresh: refreshMetrics } = metrics;
  const { refresh: refreshOpportunities } = opportunities;
  const { refresh: refreshProposals } = proposals;
  const { refresh: refreshOrders } = orders;
  const { refresh: refreshFills } = fills;
  const { refresh: refreshPortfolio } = portfolio;
  const { refresh: refreshPnl } = pnl;
  const { refresh: refreshPredictions } = predictions;
  const { refresh: refreshMesh } = mesh;
  const { refresh: refreshRegions } = regions;
  const refreshAll = useCallback(() => {
    refreshStatus();
    refreshSystem();
    refreshMetrics();
    refreshOpportunities();
    refreshProposals();
    refreshOrders();
    refreshFills();
    refreshPortfolio();
    refreshPnl();
    refreshPredictions();
    refreshMesh();
    refreshRegions();
  }, [
    refreshStatus,
    refreshSystem,
    refreshMetrics,
    refreshOpportunities,
    refreshProposals,
    refreshOrders,
    refreshFills,
    refreshPortfolio,
    refreshPnl,
    refreshPredictions,
    refreshMesh,
    refreshRegions,
  ]);

  // Re-read every route when the platform's cycle count moves. On a change,
  // not on the first reading: the first reading arrives beside the first
  // poll and re-fetching then would be a second request for the same fact.
  const seenCycles = useRef<number | null>(null);
  const [refetched, setRefetched] = useState<{ cycles: number; at: number } | null>(null);
  useEffect(() => {
    if (streamCycles === null) return;
    if (seenCycles.current !== null && seenCycles.current !== streamCycles) {
      refreshAll();
      setRefetched({ cycles: streamCycles, at: Date.now() });
    }
    seenCycles.current = streamCycles;
  }, [streamCycles, refreshAll]);

  const runs = useCycleReports();
  const latest = runs[runs.length - 1] ?? null;
  const stageOf = useMemo(() => {
    const seen = new Map<string, CycleStage>();
    for (const stage of latest?.report.stages ?? []) seen.set(stage.stage, stage);
    return (name: StageName): CycleStage | null => seen.get(name) ?? null;
  }, [latest]);

  // Where the report and the platform disagree about which cycle is latest.
  const reportCycle = latest?.report.cycle ?? null;
  const platformCycles = streamCycles ?? status.data?.cycles ?? null;
  const drift: "none" | "behind" | "restarted" | null =
    reportCycle === null || platformCycles === null
      ? null
      : platformCycles === reportCycle
        ? "none"
        : platformCycles > reportCycle
          ? "behind"
          : "restarted";

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Dataflow"
          meta={<StreamControls stream={health} name="health" />}
          actions={
            <>
              {status.data === null ? null : (
                <StatusChip
                  tone={status.data.live_capable ? "bad" : "ok"}
                  label={status.data.live_capable ? "LIVE-CAPABLE" : "PAPER TRADING"}
                  title="GET /system/status: live_capable"
                />
              )}
              <Link href="/loop" className="btn" data-testid="dataflow-loop-link">
                Run a cycle on the loop page
              </Link>
            </>
          }
        />
        <PanelBody>
          <KpiRow>
            <Kpi
              label="Cycles (stream)"
              value={
                <span data-testid="dataflow-stream-cycles">
                  {streamCycles === null ? "—" : formatCount(streamCycles)}
                </span>
              }
              note={
                streamCycles === null
                  ? "no health.changed event yet"
                  : "from /stream/health; every route re-reads when this moves"
              }
              tone="info"
            />
            <Kpi
              label="Cycles (status)"
              value={formatCount(status.data?.cycles)}
              note="GET /system/status: cycles"
            />
            <Kpi
              label="Report shown"
              value={
                <span data-testid="dataflow-report-cycle">
                  {reportCycle === null ? "none" : `#${formatCount(reportCycle)}`}
                </span>
              }
              tone={drift === "behind" || drift === "restarted" ? "warn" : "neutral"}
              note={
                latest === null
                  ? "no POST /cycle response held by this tab"
                  : `POST /cycle answered ${formatClock(latest.at)}`
              }
            />
            <Kpi
              label="Every stage traversed"
              value={latest === null ? "—" : latest.report.traversed_every_stage ? "yes" : "no"}
              tone={
                latest === null ? "neutral" : latest.report.traversed_every_stage ? "ok" : "warn"
              }
              note="from the report, not inferred"
            />
            <Kpi
              label="Last re-read"
              value={
                <span data-testid="dataflow-refetched">
                  {refetched === null ? "—" : formatClock(refetched.at)}
                </span>
              }
              note={
                refetched === null
                  ? "no cycle-count change seen on the stream yet"
                  : `after the stream reported cycle ${formatCount(refetched.cycles)}`
              }
            />
          </KpiRow>
          {drift === "behind" ? (
            <p
              className="mt-2 text-[11.5px] leading-relaxed text-[color:var(--color-warn)]"
              role="status"
              data-testid="dataflow-drift"
            >
              The platform reports {formatCount(platformCycles)} cycle(s); the stage detail below
              is from cycle {formatCount(reportCycle)}, the last one this console ran. No route
              serves a past cycle&rsquo;s stages, so a cycle run elsewhere shows here only in the
              counts.
            </p>
          ) : drift === "restarted" ? (
            <p
              className="mt-2 text-[11.5px] leading-relaxed text-[color:var(--color-down)]"
              role="status"
              data-testid="dataflow-drift"
            >
              The platform&rsquo;s cycle count ({formatCount(platformCycles)}) is below this
              report&rsquo;s cycle ({formatCount(reportCycle)}), so the process has restarted
              since. The stages below describe a process that no longer runs.
            </p>
          ) : null}
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Eight stages, left to right"
          meta={
            latest === null ? (
              <span className="num text-[10px] text-[color:var(--color-ink-faint)]">
                no report held
              </span>
            ) : (
              <span className="num text-[10px] text-[color:var(--color-ink-faint)]">
                cycle {latest.report.cycle} · {latest.report.correlation_id}
              </span>
            )
          }
        />
        <PanelBody>
          {latest === null ? (
            <div className="mb-3" data-testid="dataflow-no-report">
              <EmptyBlock headline="No cycle report is held by this console.">
                <p>
                  The platform serves a cycle&rsquo;s stage outcomes only in the response to the{" "}
                  <code className="num">POST /api/v1/cycle</code> that ran it. Run one from the{" "}
                  <Link href="/loop" className="underline">
                    loop page
                  </Link>{" "}
                  and come back; the counts on the edges below are live regardless.
                </p>
              </EmptyBlock>
            </div>
          ) : null}

          <div className="overflow-x-auto">
            <div
              className="grid gap-x-4 gap-y-3"
              style={{
                gridTemplateColumns: "repeat(8, minmax(140px, 1fr))",
                minWidth: "1232px",
                alignItems: "start",
              }}
            >
              {/* Row 1: what feeds the loop, bottom-aligned onto the stage row. */}
              <div style={{ gridColumn: "1 / span 2", alignSelf: "end" }}>
                <EdgeCard title="feed → SENSE" reads="POST /cycle · stage sense" flowsDown>
                  {(() => {
                    const sense = stageOf("sense");
                    if (sense === null) return <Muted>no report held</Muted>;
                    return (
                      <>
                        <Big>{formatCount(sense.produced)}</Big>
                        <Muted>observation(s) SENSE counted this cycle</Muted>
                        <Quote>{sense.detail}</Quote>
                      </>
                    );
                  })()}
                </EdgeCard>
              </div>
              <div style={{ gridColumn: "4 / span 5", alignSelf: "end" }}>
                <EdgeCard
                  title="regional cells → mesh → centre"
                  reads="GET /mesh · GET /regions"
                  flowsDown
                >
                  <MeshEdge mesh={mesh} regions={regions} />
                </EdgeCard>
              </div>

              {/* Row 2: the stages. */}
              <ol className="contents" aria-label="the eight stages of the cycle">
                {STAGES.map((name, index) => (
                  <StageNode
                    key={name}
                    index={index}
                    name={name}
                    stage={stageOf(name)}
                    hasReport={latest !== null}
                    last={index === STAGES.length - 1}
                    between={betweenLabel(name, metrics.data)}
                  />
                ))}
              </ol>

              {/* Row 3: what left each edge. */}
              <div style={{ gridColumn: "3 / span 2" }}>
                <EdgeCard title="DISCOVER → REASON" reads="GET /opportunities · GET /system/metrics">
                  <ResourceView resource={opportunities} loadingRows={2}>
                    {(data) => (
                      <>
                        <Big>{formatCount(data.opportunities.length)}</Big>
                        <Muted>
                          opportunity(ies) in the queue now
                          {metrics.data === null
                            ? ""
                            : ` · /system/metrics says ${formatCount(metrics.data.opportunities_queued)} queued`}
                        </Muted>
                        {data.opportunities.length === 0 ? null : (
                          <ul className="mt-1 flex flex-col gap-1">
                            {data.opportunities.slice(0, 6).map((opportunity) => (
                              <li key={opportunity.id} className="text-[11px] leading-snug">
                                <span className="num text-[color:var(--color-ink-faint)]">
                                  score {opportunity.score.toFixed(2)} · confidence{" "}
                                  {opportunity.confidence.toFixed(2)}
                                </span>{" "}
                                {opportunity.headline}
                              </li>
                            ))}
                          </ul>
                        )}
                      </>
                    )}
                  </ResourceView>
                  <StageQuote label="REASON says" stage={stageOf("reason")} />
                </EdgeCard>
              </div>
              <div style={{ gridColumn: "5 / span 2" }}>
                <EdgeCard title="DECIDE → ACT" reads="GET /proposals · GET /orders">
                  <ResourceView resource={proposals} loadingRows={2}>
                    {(data) => (
                      <>
                        <Big>{formatCount(data.proposals.length)}</Big>
                        <Muted>
                          proposal(s) held{countByField(data.proposals, (proposal) => proposal.status)}
                        </Muted>
                        {data.proposals.length === 0 ? null : (
                          <Quote>{data.proposals[data.proposals.length - 1]?.rationale}</Quote>
                        )}
                      </>
                    )}
                  </ResourceView>
                  <ResourceView resource={orders} loadingRows={2}>
                    {(data) => (
                      <div className="mt-2 flex flex-col gap-0.5">
                        <div className="flex flex-wrap items-center gap-1.5">
                          <Chip tone={data.orders.length > 0 ? "info" : "neutral"}>
                            {formatCount(data.orders.length)} order(s) released
                          </Chip>
                          <Chip tone={data.refusals > 0 ? "warn" : "neutral"}>
                            {formatCount(data.refusals)} refused
                          </Chip>
                          {data.reconciliation_breaks.length > 0 ? (
                            <Chip tone="bad">
                              {formatCount(data.reconciliation_breaks.length)} reconciliation
                              break(s)
                            </Chip>
                          ) : null}
                        </div>
                        <Muted>
                          {countByField(data.orders, (order) => order.state) ||
                            "no order states to count"}
                        </Muted>
                        {data.reconciliation_breaks.map((reason) => (
                          <Quote key={reason} tone="bad">
                            {reason}
                          </Quote>
                        ))}
                      </div>
                    )}
                  </ResourceView>
                  <StageQuote label="DECIDE says" stage={stageOf("decide")} />
                </EdgeCard>
              </div>
              <div style={{ gridColumn: "7 / span 2" }}>
                <EdgeCard title="LEARN → claims, calibration" reads="GET /predictions">
                  <ResourceView resource={predictions} loadingRows={2}>
                    {(data) => (
                      <>
                        <div className="flex flex-wrap items-center gap-1.5">
                          <Chip>{formatCount(data.held)} held</Chip>
                          <Chip tone="info">{formatCount(data.open)} open</Chip>
                          <Chip tone={data.resolved > 0 ? "ok" : "neutral"}>
                            {formatCount(data.resolved)} resolved
                          </Chip>
                        </div>
                        <Muted>
                          as of cycle {formatCount(data.as_of_cycle)} · window {formatCount(data.window)}
                        </Muted>
                        {isUnavailable(data.calibration) ? (
                          <Quote tone="warn">{data.calibration.reason}</Quote>
                        ) : (
                          <Muted>
                            calibration over {formatCount(data.calibration.evaluations_in_window)}{" "}
                            evaluation(s), {data.calibration.material ? "material" : "not material"}
                            {typeof data.calibration.report.brier_score === "number"
                              ? ` · brier ${data.calibration.report.brier_score.toFixed(3)}`
                              : ""}
                          </Muted>
                        )}
                      </>
                    )}
                  </ResourceView>
                  <StageQuote label="LEARN says" stage={stageOf("learn")} />
                </EdgeCard>
              </div>

              {/* Row 4: where it lands. */}
              <div style={{ gridColumn: "1 / span 3" }}>
                <EdgeCard title="event log → archive" reads="GET /system/status · GET /system">
                  <ResourceView resource={status} loadingRows={2}>
                    {(data) => (
                      <div className="flex flex-wrap items-center gap-1.5">
                        <Chip tone="info">{formatCount(data.events)} event(s) in memory</Chip>
                        <Chip tone={data.archived === null ? "warn" : "ok"}>
                          {data.archived === null
                            ? "archive: none configured"
                            : `${formatCount(data.archived)} archived`}
                        </Chip>
                        {latest === null ? null : (
                          <Chip tone={latest.report.archived === null ? "warn" : "neutral"}>
                            {latest.report.archived === null
                              ? "last cycle archived nothing"
                              : `${formatCount(latest.report.archived)} archived on cycle ${formatCount(latest.report.cycle)}`}
                          </Chip>
                        )}
                      </div>
                    )}
                  </ResourceView>
                  <ResourceView resource={system} loadingRows={1}>
                    {(data) => (
                      <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
                        <Chip tone={data.chain_intact ? "ok" : "bad"}>
                          {data.chain_intact
                            ? "hash chain intact"
                            : `hash chain broken at ${formatCount(data.chain_broken_at)}`}
                        </Chip>
                        <Muted>GET /system: {formatCount(data.events_logged)} logged</Muted>
                      </div>
                    )}
                  </ResourceView>
                  {latest?.report.archive_error ? (
                    <Quote tone="bad">{latest.report.archive_error}</Quote>
                  ) : null}
                </EdgeCard>
              </div>
              <div style={{ gridColumn: "4 / span 5" }}>
                <EdgeCard
                  title="ACT → fills → portfolio → P&L"
                  reads="GET /fills · GET /portfolio · GET /pnl"
                >
                  <div className="grid gap-3" style={{ gridTemplateColumns: "1.3fr 1fr 1.2fr" }}>
                    <div>
                      <ResourceView resource={fills} loadingRows={2}>
                        {(data) => (
                          <>
                            <div className="flex flex-wrap items-center gap-1.5">
                              <Chip tone={data.fills.length > 0 ? "info" : "neutral"}>
                                {formatCount(data.fills.length)} fill(s)
                              </Chip>
                              <Chip tone={data.any_live_fill ? "bad" : "ok"}>
                                {data.any_live_fill ? "A FILL WAS NOT SIMULATED" : "every fill simulated"}
                              </Chip>
                            </div>
                            {data.fills.length === 0 ? (
                              <Muted>the book holds no fill; an observed zero, not an absence</Muted>
                            ) : (
                              <ul className="mt-1 flex flex-col gap-0.5">
                                {data.fills.slice(0, 6).map((fill, index) => (
                                  <li
                                    key={`${fill.order}-${index}`}
                                    className="num flex flex-wrap items-center gap-1.5 text-[11px]"
                                  >
                                    <span>{fill.instrument}</span>
                                    <span>{fill.side}</span>
                                    <span>{formatDecimal(fill.quantity)}</span>
                                    <span>@ {formatDecimal(fill.price)}</span>
                                    <span className="text-[color:var(--color-ink-faint)]">{fill.venue}</span>
                                    <Chip tone={fill.simulated ? "ok" : "bad"}>
                                      {fill.simulated ? "PAPER" : "NOT SIMULATED"}
                                    </Chip>
                                  </li>
                                ))}
                                {data.fills.length > 6 ? (
                                  <li>
                                    <Muted>and {formatCount(data.fills.length - 6)} more</Muted>
                                  </li>
                                ) : null}
                              </ul>
                            )}
                          </>
                        )}
                      </ResourceView>
                      <StageQuote label="ACT says" stage={stageOf("act")} />
                    </div>
                    <div>
                      <span className="eyebrow">portfolio</span>
                      <ResourceView resource={portfolio} loadingRows={2}>
                        {(data) => (
                          <div className="mt-1 flex flex-col gap-1">
                            <div className="flex flex-wrap items-center gap-1.5">
                              <Chip>{formatCount(data.proposals)} proposal(s)</Chip>
                              <Chip>{formatCount(data.orders)} order(s)</Chip>
                              <Chip>{formatCount(data.fills)} fill(s)</Chip>
                            </div>
                            <StatusChip
                              tone={data.paper_only ? "ok" : "bad"}
                              label={data.paper_only ? "PAPER TRADING" : "NOT PAPER-ONLY"}
                              title="GET /portfolio: paper_only"
                            />
                          </div>
                        )}
                      </ResourceView>
                    </div>
                    <div>
                      <span className="eyebrow">P&amp;L and attribution</span>
                      <div className="mt-1">
                        <ResourceView resource={pnl} loadingRows={2}>
                          {(data) => (
                            <pre className="num whitespace-pre-wrap text-[10.5px] text-[color:var(--color-ink-dim)]">
                              {JSON.stringify(data, null, 1)}
                            </pre>
                          )}
                        </ResourceView>
                      </div>
                    </div>
                  </div>
                </EdgeCard>
              </div>
            </div>
          </div>
          <p className="mt-3 text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
            Stage boxes quote the report&rsquo;s <code className="num">detail</code> verbatim; the
            counts between them are <code className="num">GET /system/metrics</code>; the edge cards
            name the route each reads. Where a route answers{" "}
            <code className="num">available: false</code>, its reason is shown in the
            platform&rsquo;s words. Nothing here can submit an order.
          </p>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Counters between the stages" meta={<Freshness resource={metrics} name="metrics" />} />
        <PanelBody flush>
          <ResourceView resource={metrics} loadingRows={3}>
            {(m) => (
              <TableWell maxHeight="none" label="Loop counters">
                <table className="dt">
                  <thead>
                    <tr>
                      <th scope="col">Edge</th>
                      <th scope="col">Counter</th>
                      <th scope="col" className="n">
                        Value
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    <tr>
                      <td>SENSE → … → LEARN</td>
                      <td className="num">cycles</td>
                      <td className="n">{formatCount(m.cycles)}</td>
                    </tr>
                    <tr>
                      <td>every stage → event log</td>
                      <td className="num">events_logged</td>
                      <td className="n">{formatCount(m.events_logged)}</td>
                    </tr>
                    <tr>
                      <td>DISCOVER → REASON</td>
                      <td className="num">opportunities_queued</td>
                      <td className="n">{formatCount(m.opportunities_queued)}</td>
                    </tr>
                    <tr>
                      <td>DECIDE → ACT</td>
                      <td className="num">proposals</td>
                      <td className="n">{formatCount(m.proposals)}</td>
                    </tr>
                    <tr>
                      <td>ACT → venue (simulated)</td>
                      <td className="num">orders</td>
                      <td className="n">{formatCount(m.orders)}</td>
                    </tr>
                    <tr>
                      <td>venue → portfolio</td>
                      <td className="num">fills</td>
                      <td className="n">{formatCount(m.fills)}</td>
                    </tr>
                    <tr>
                      <td>risk → ACT (before an order exists)</td>
                      <td className="num">refusals</td>
                      <td className="n">{formatCount(m.refusals)}</td>
                    </tr>
                    <tr data-alert={m.live_fills ? "true" : undefined}>
                      <td>venue → portfolio, not simulated</td>
                      <td className="num">live_fills</td>
                      <td className="n">{m.live_fills ? "true" : "false"}</td>
                    </tr>
                  </tbody>
                </table>
              </TableWell>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>
    </div>
  );
}

// --- pieces -----------------------------------------------------------------

/** The counter `GET /system/metrics` keeps for the edge leaving `name`, if any. */
function betweenLabel(name: StageName, metrics: SystemMetrics | null): string | null {
  if (metrics === null) return null;
  switch (name) {
    case "discover":
      return `${formatCount(metrics.opportunities_queued)} queued`;
    case "decide":
      return `${formatCount(metrics.proposals)} proposal(s)`;
    case "act":
      return `${formatCount(metrics.orders)} order(s) · ${formatCount(metrics.fills)} fill(s)`;
    default:
      return null;
  }
}

function StageNode({
  index,
  name,
  stage,
  hasReport,
  last,
  between,
}: {
  index: number;
  name: StageName;
  stage: CycleStage | null;
  hasReport: boolean;
  last: boolean;
  between: string | null;
}) {
  const tone = !hasReport
    ? "neutral"
    : stage === null || !stage.ran
      ? "warn"
      : stage.problems.length > 0
        ? "bad"
        : "ok";
  return (
    <li
      className="relative flex min-h-[132px] flex-col gap-1 border border-[color:var(--color-line-strong)] bg-[color:var(--color-surface)] px-3 py-2"
      data-testid={`dataflow-stage-${name}`}
      data-alert={stage !== null && stage.problems.length > 0 ? "true" : undefined}
    >
      <div className="flex flex-wrap items-center justify-between gap-1">
        <span className="eyebrow">
          {index + 1}. {name.toUpperCase()}
        </span>
        <Chip tone={tone}>
          {!hasReport
            ? "no report"
            : stage === null
              ? "not in report"
              : stage.ran
                ? `${formatCount(stage.produced)} produced`
                : "did not run"}
        </Chip>
      </div>
      <p
        className="text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]"
        data-testid={`dataflow-stage-detail-${name}`}
      >
        {stage === null ? (hasReport ? "the report carried no outcome for this stage" : "—") : stage.detail}
      </p>
      {stage !== null && stage.problems.length > 0 ? (
        <p className="text-[11.5px] leading-relaxed text-[color:var(--color-down)]">
          {stage.problems.join("; ")}
        </p>
      ) : null}
      {between === null ? null : (
        // The counter GET /system/metrics keeps for what leaves this stage,
        // at the foot of the node beside the arrow out of it.
        <span className="num mt-auto self-end text-[9.5px] text-[color:var(--color-ink-faint)]">
          → {between}
        </span>
      )}
      {last ? null : (
        <svg
          width="16"
          height="12"
          viewBox="0 0 16 12"
          className="absolute -right-4 top-1/2 -translate-y-1/2 text-[color:var(--color-ink-faint)]"
          aria-hidden="true"
        >
          <path d="M0 6h13M9 1l5 5-5 5" fill="none" stroke="currentColor" strokeWidth="1.5" />
        </svg>
      )}
    </li>
  );
}

function EdgeCard({
  title,
  reads,
  flowsDown = false,
  children,
}: {
  title: string;
  reads: string;
  flowsDown?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="relative flex h-full flex-col gap-1 border border-dashed border-[color:var(--color-line-strong)] px-3 py-2">
      <div className="flex items-baseline justify-between gap-2">
        <span className="eyebrow">{title}</span>
        <span className="num text-[9.5px] text-[color:var(--color-ink-faint)]">{reads}</span>
      </div>
      <div className="flex flex-col gap-1 text-[12px]">{children}</div>
      {flowsDown ? (
        <svg
          width="12"
          height="14"
          viewBox="0 0 12 14"
          className="absolute -bottom-3.5 left-8 text-[color:var(--color-ink-faint)]"
          aria-hidden="true"
        >
          <path d="M6 0v10M1 6l5 5 5-5" fill="none" stroke="currentColor" strokeWidth="1.5" />
        </svg>
      ) : null}
    </div>
  );
}

/** A stage's own sentence, quoted under the edge it describes. */
function StageQuote({ label, stage }: { label: string; stage: CycleStage | null }) {
  if (stage === null) return null;
  return (
    <div className="mt-1.5 border-t border-[color:var(--color-line)] pt-1.5">
      <span className="eyebrow">{label}</span>
      <Quote>{stage.detail}</Quote>
    </div>
  );
}

function MeshEdge({ mesh, regions }: { mesh: Resource<MeshStatus>; regions: Resource<Regions> }) {
  return (
    <div className="grid gap-3" style={{ gridTemplateColumns: "1fr 1fr" }}>
      <div>
        <span className="eyebrow">mesh backbone</span>
        <div className="mt-1">
          <ResourceView resource={mesh} loadingRows={2}>
            {(data) =>
              !data.served ? (
                <Quote tone="warn">{data.error ?? "the mesh reports served: false"}</Quote>
              ) : data.error !== undefined ? (
                <Quote tone="bad">{data.error}</Quote>
              ) : (
                <div className="flex flex-col gap-1">
                  <div className="flex flex-wrap items-center gap-1.5">
                    <Chip tone="ok">served</Chip>
                    <Chip>{formatCount(data.cells?.length)} cell(s) configured</Chip>
                    <Chip>{formatCount(data.counters?.reports_ingested)} report(s) ingested</Chip>
                  </div>
                  {data.counters === undefined ? null : (
                    <Muted>
                      orders reported {formatCount(data.counters.orders_reported)} · fills reported{" "}
                      {formatCount(data.counters.fills_reported)} · fills omitted{" "}
                      {formatCount(data.counters.fills_omitted)} · refusals{" "}
                      {formatCount(data.counters.refusals_reported)} · envelopes dispatched{" "}
                      {formatCount(data.counters.envelopes_dispatched)} · held{" "}
                      {formatCount(data.counters.envelopes_held)} · rejected{" "}
                      {formatCount(data.counters.envelopes_rejected)} · unserved{" "}
                      {formatCount(data.counters.envelopes_unserved)} · cell halts{" "}
                      {formatCount(data.counters.cell_halts)}
                    </Muted>
                  )}
                </div>
              )
            }
          </ResourceView>
        </div>
      </div>
      <div>
        <span className="eyebrow">cells reporting</span>
        <div className="mt-1">
          <ResourceView resource={regions} loadingRows={2}>
            {(data) => (
              <div className="flex flex-col gap-1">
                <Muted>
                  {formatCount(data.cells.length)} cell(s) · freshness bound {data.freshness_bound}
                </Muted>
                <ul className="flex flex-col gap-0.5">
                  {data.cells.map((cell) => (
                    <li key={cell.cell} className="flex flex-wrap items-center gap-1.5 text-[11px]">
                      <span className="num">{cell.cell}</span>
                      <Chip tone={cell.halted ? "bad" : cell.stale ? "warn" : "ok"}>
                        {cell.halted ? "halted" : cell.stale ? "stale" : "fresh"}
                      </Chip>
                      <span className="num text-[color:var(--color-ink-faint)]">
                        {cell.age} · {formatCount(cell.positions)} position(s) ·{" "}
                        {formatCount(cell.strategies)} strategy(ies) · gross {formatDecimal(cell.gross)}
                      </span>
                      {cell.reconciliation_breaks > 0 ? (
                        <Chip tone="bad">{formatCount(cell.reconciliation_breaks)} break(s)</Chip>
                      ) : null}
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </ResourceView>
        </div>
      </div>
    </div>
  );
}

/** Count `items` by one string field, as " · 2 draft, 1 released". */
function countByField<T>(items: readonly T[], pick: (item: T) => string): string {
  if (items.length === 0) return "";
  const counts = new Map<string, number>();
  for (const item of items) {
    const key = pick(item);
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  return ` · ${[...counts.entries()].map(([key, count]) => `${count} ${key}`).join(", ")}`;
}

function Big({ children }: { children: ReactNode }) {
  return <span className="num text-[19px] font-semibold leading-none">{children}</span>;
}

function Muted({ children }: { children: ReactNode }) {
  return <span className="text-[11px] leading-snug text-[color:var(--color-ink-faint)]">{children}</span>;
}

/** Text from the platform, rendered as it came. */
function Quote({ children, tone }: { children: ReactNode; tone?: "warn" | "bad" }) {
  const colour =
    tone === "bad"
      ? "var(--color-down)"
      : tone === "warn"
        ? "var(--color-warn)"
        : "var(--color-ink-dim)";
  return (
    <p className="text-[11.5px] leading-relaxed" style={{ color: colour }}>
      {children}
    </p>
  );
}
