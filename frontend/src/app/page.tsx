"use client";

import { usePlatform } from "@/components/chrome/PlatformProvider";
import { Chip, Freshness, Metric, MetricRow, StatusChip } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { RunCycleCard } from "@/components/data/RunCycle";
import { EmptyBlock, LoadingBlock, ResourceView, UnavailableBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import { isUnavailable, type Opportunities, type Portfolio, type Risk, type SystemMetrics } from "@/lib/api/types";
import { formatCount, formatPercent } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * What an operator needs in the first ten seconds: whether the platform is
 * running, what authority it is running under, whether the book disagrees with
 * any venue, and what the loop has produced.
 */
export default function ExecutiveOverview() {
  const { health, status } = usePlatform();

  const metrics = useResource<SystemMetrics>(platform.systemMetrics, {
    key: "system-metrics",
    label: "GET /system/metrics",
    intervalMs: 10_000,
  });
  const portfolio = useResource<Portfolio>(platform.portfolio, {
    key: "portfolio-summary",
    label: "GET /portfolio",
    intervalMs: 15_000,
  });
  const risk = useResource<Risk>(platform.risk, {
    key: "risk-summary",
    label: "GET /risk",
    intervalMs: 15_000,
  });
  const opportunities = useResource<Opportunities>(platform.opportunities, {
    key: "opportunities",
    label: "GET /opportunities",
    intervalMs: 10_000,
  });
  const pnl = useResource<unknown>(platform.pnl, {
    key: "pnl-summary",
    label: "GET /pnl",
    intervalMs: 30_000,
  });

  const halted = health.data?.halted ?? status.data?.halted ?? null;
  const breaks = health.data?.reconciliation_breaks ?? null;

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Platform state"
          meta={<Freshness resource={health} name="platform health" />}
          actions={
            <>
              {status.data ? (
                <Chip tone="info">
                  autonomy {status.data.autonomy} / ceiling {status.data.ceiling}
                </Chip>
              ) : null}
            </>
          }
        />
        <PanelBody>
          {health.loading && health.outcome === null ? (
            <LoadingBlock rows={2} label="reading /health" />
          ) : (
            <MetricRow>
              <Metric
                label="Halt state"
                value={halted === null ? "unknown" : halted ? "HALTED" : "running"}
                tone={halted === null ? "warn" : halted ? "bad" : "ok"}
                hint={health.data ? `status: ${health.data.status}` : "no answer from /health"}
              />
              <Metric
                label="Reconciliation breaks"
                value={breaks === null ? "—" : formatCount(breaks)}
                tone={breaks !== null && breaks > 0 ? "bad" : "ok"}
                hint="book against venue"
              />
              <Metric
                label="Cycles"
                value={formatCount(status.data?.cycles)}
                hint="loop iterations since start"
              />
              <Metric
                label="Events logged"
                value={formatCount(status.data?.events)}
                hint={
                  status.data?.archived === null
                    ? "nothing archived: in-memory only"
                    : `${formatCount(status.data?.archived)} archived`
                }
                tone={status.data?.archived === null ? "warn" : undefined}
              />
              <Metric
                label="Live capable"
                value={
                  health.data === null ? "—" : health.data.live_capable ? "yes" : "no"
                }
                tone={health.data?.live_capable ? "bad" : "ok"}
                hint="whether the process can reach a live venue"
              />
              <Metric
                label="Mesh"
                value={status.data ? (status.data.mesh.served ? "served" : "not served") : "—"}
                tone={status.data && !status.data.mesh.served ? "warn" : undefined}
                hint="central half of the backbone"
              />
            </MetricRow>
          )}
        </PanelBody>
      </Panel>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-3">
        <Panel>
          <PanelHead title="Loop throughput" meta={<Freshness resource={metrics} name="metrics" />} />
          <PanelBody>
            <ResourceView resource={metrics} loadingRows={3}>
              {(data) => (
                <dl className="flex flex-col">
                  <KV label="Opportunities queued" value={formatCount(data.opportunities_queued)} />
                  <KV label="Proposals" value={formatCount(data.proposals)} />
                  <KV label="Orders" value={formatCount(data.orders)} />
                  <KV label="Fills" value={formatCount(data.fills)} />
                  <KV label="Refusals" value={formatCount(data.refusals)} />
                  <div className="flex items-baseline justify-between gap-4 py-1">
                    <dt className="text-[11px] text-[color:var(--color-ink-dim)]">
                      Any live fill
                    </dt>
                    <dd>
                      <Chip tone={data.live_fills ? "bad" : "ok"}>
                        {data.live_fills ? "yes — not paper only" : "no"}
                      </Chip>
                    </dd>
                  </div>
                </dl>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead title="Book" meta={<Freshness resource={portfolio} name="portfolio" />} />
          <PanelBody>
            <ResourceView resource={portfolio} loadingRows={3}>
              {(data) => (
                <dl className="flex flex-col">
                  <KV label="Proposals" value={formatCount(data.proposals)} />
                  <KV label="Orders" value={formatCount(data.orders)} />
                  <KV label="Fills" value={formatCount(data.fills)} />
                  <div className="flex items-baseline justify-between gap-4 py-1">
                    <dt className="text-[11px] text-[color:var(--color-ink-dim)]">Paper only</dt>
                    <dd>
                      <Chip tone={data.paper_only ? "ok" : "bad"}>
                        {data.paper_only ? "yes" : "NO — live fills present"}
                      </Chip>
                    </dd>
                  </div>
                  <p className="pt-2 text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
                    Position-level detail is behind the desk&rsquo;s capability gate and is not
                    served over HTTP. These are counts, not a book.
                  </p>
                </dl>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead title="Profit and loss" meta={<Freshness resource={pnl} name="P&L" />} />
          <PanelBody>
            <ResourceView resource={pnl} loadingRows={3}>
              {() => (
                <EmptyBlock headline="The platform returned a P&L body this console does not model.">
                  <p>
                    <code className="num">GET /api/v1/pnl</code> answered with data rather than an
                    absence. Attribution rendering is not implemented here yet.
                  </p>
                </EmptyBlock>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>
      </div>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead
            title="Risk posture"
            meta={<Freshness resource={risk} name="risk" />}
          />
          <PanelBody>
            <ResourceView resource={risk} loadingRows={4}>
              {(data) => {
                const switchState = data.kill_switch;
                const concentrations = data.concentrations;
                return (
                  <div className="flex flex-col gap-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <StatusChip
                        tone={switchState.halted ? "bad" : "ok"}
                        label={switchState.halted ? "kill switch tripped" : "kill switch clear"}
                      />
                      {switchState.halted_scopes.map((scope) => (
                        <Chip key={scope} tone="bad">
                          {scope}
                        </Chip>
                      ))}
                      <Chip>{switchState.clearances} clearance(s)</Chip>
                    </div>
                    {switchState.halted && switchState.reason ? (
                      <p className="text-[11.5px] text-[color:var(--color-ink-dim)]">
                        Tripped by <span className="num">{switchState.tripped_by || "unknown"}</span>:{" "}
                        {switchState.reason}
                      </p>
                    ) : null}
                    {isUnavailable(concentrations) ? (
                      <UnavailableBlock
                        subject={concentrations.subject}
                        reason={concentrations.reason}
                      />
                    ) : concentrations.findings.length === 0 ? (
                      <EmptyBlock headline="No concentration limit is breached." />
                    ) : (
                      <table className="dt">
                        <thead>
                          <tr>
                            <th scope="col">Axis</th>
                            <th scope="col">Bucket</th>
                            <th scope="col" className="n">
                              Share
                            </th>
                            <th scope="col" className="n">
                              Limit
                            </th>
                          </tr>
                        </thead>
                        <tbody>
                          {concentrations.findings.map((finding) => (
                            <tr key={`${finding.axis}:${finding.bucket}`} data-alert="true">
                              <td className="num">{finding.axis}</td>
                              <td>{finding.bucket}</td>
                              <td className="n" data-direction="negative">
                                {formatPercent(finding.share)}
                              </td>
                              <td className="n">{formatPercent(finding.limit)}</td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    )}
                  </div>
                );
              }}
            </ResourceView>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead
            title="Opportunity queue"
            meta={<Freshness resource={opportunities} name="opportunities" />}
          />
          <PanelBody flush>
            <ResourceView resource={opportunities} loadingRows={5}>
              {(data) =>
                data.opportunities.length === 0 ? (
                  <EmptyBlock headline="The queue is empty.">
                    <p>
                      Observed, not assumed: <code className="num">GET /api/v1/opportunities</code>{" "}
                      returned an empty list. Run a cycle to fill it.
                    </p>
                  </EmptyBlock>
                ) : (
                  <TableWell maxHeight="260px" label="Opportunity queue">
                    <table className="dt">
                      <thead>
                        <tr>
                          <th scope="col">Id</th>
                          <th scope="col">Headline</th>
                          <th scope="col" className="n">
                            Score
                          </th>
                          <th scope="col" className="n">
                            Confidence
                          </th>
                          <th scope="col">Detectors</th>
                        </tr>
                      </thead>
                      <tbody>
                        {data.opportunities.map((opportunity) => (
                          <tr key={opportunity.id}>
                            <td className="num">{opportunity.id}</td>
                            <td className="whitespace-normal">{opportunity.headline}</td>
                            <td className="n">{opportunity.score.toFixed(3)}</td>
                            <td className="n">{formatPercent(opportunity.confidence)}</td>
                            <td className="num text-[10px] text-[color:var(--color-ink-dim)]">
                              {opportunity.detectors.join(", ") || "—"}
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </TableWell>
                )
              }
            </ResourceView>
          </PanelBody>
        </Panel>
      </div>

      <RunCycleCard
        onRan={() => {
          metrics.refresh();
          portfolio.refresh();
          opportunities.refresh();
          status.refresh();
        }}
      />
    </div>
  );
}

function KV({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between gap-4 border-b border-[color:var(--color-line)] py-1 last:border-b-0">
      <dt className="text-[11px] text-[color:var(--color-ink-dim)]">{label}</dt>
      <dd className="num text-[12px]">{value}</dd>
    </div>
  );
}
