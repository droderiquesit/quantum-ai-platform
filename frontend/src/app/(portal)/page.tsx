"use client";

import { usePlatform } from "@/components/chrome/PlatformProvider";
import { Chip, Freshness, StatusChip } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { RunCycleCard } from "@/components/data/RunCycle";
import { EmptyBlock, LoadingBlock, ResourceView, UnavailableBlock } from "@/components/data/States";
import { AreaChart, Bars, Gauge } from "@/components/viz/primitives";
import { platform } from "@/lib/api/client";
import {
  isUnavailable,
  type Opportunities,
  type Portfolio,
  type Risk,
  type SystemMetrics,
} from "@/lib/api/types";
import { formatCount, formatPercent } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";
import { describeWindow, useSeries } from "@/lib/hooks/useSeries";

/**
 * The first screen: posture, throughput, and what the loop is holding.
 *
 * Chart-led, but every curve on it is a series this browser accumulated by
 * polling — the platform serves counters, not history. Each one carries the
 * window it was observed over for that reason. The alternative, drawing a
 * counter as though it were a recorded time series, would put a plausible
 * trend on the most-read screen in the console with nothing behind it.
 */
export default function CommandOverview() {
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

  const data = metrics.data;
  const cycles = useSeries(data?.cycles ?? null);
  const events = useSeries(data?.events_logged ?? null);
  const queued = useSeries(data?.opportunities_queued ?? null);
  const proposals = useSeries(data?.proposals ?? null);
  const orders = useSeries(data?.orders ?? null);
  const fills = useSeries(data?.fills ?? null);
  const refusals = useSeries(data?.refusals ?? null);

  const halted = health.data?.halted ?? status.data?.halted ?? null;
  const breaks = health.data?.reconciliation_breaks ?? null;

  // Refusals over everything the risk engine ruled on. Undefined rather than
  // zero when nothing has been ruled on: a desk with no orders has no refusal
  // rate, and drawing one at 0% would read as "nothing is being refused".
  const ruled = data === null ? 0 : data.orders + data.refusals;
  const refusalRate = data === null || ruled === 0 ? null : data.refusals / ruled;

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Posture"
          meta={<Freshness resource={health} name="platform health" />}
          actions={
            <>
              <StatusChip
                tone={halted === null ? "warn" : halted ? "bad" : "ok"}
                label={halted === null ? "halt state unknown" : halted ? "HALTED" : "running"}
                pulse={halted === false}
              />
              {status.data ? (
                <Chip
                  tone="info"
                  title={`autonomy ${status.data.autonomy}, ceiling ${status.data.ceiling}`}
                >
                  {/* Said once when the two agree. Repeating the same word is
                      what pushed this chip off the right of a phone. */}
                  {status.data.autonomy === status.data.ceiling
                    ? `${status.data.autonomy} · at ceiling`
                    : `${status.data.autonomy} · ceiling ${status.data.ceiling}`}
                </Chip>
              ) : null}
              <Chip tone={health.data?.live_capable ? "bad" : "ok"}>
                {health.data === null
                  ? "live capability unread"
                  : health.data.live_capable
                    ? "live-capable"
                    : "cannot reach a live venue"}
              </Chip>
            </>
          }
        />
        <PanelBody>
          {metrics.loading && metrics.outcome === null ? (
            <LoadingBlock rows={2} label="reading /system/metrics" />
          ) : (
            <KpiRow>
              <Kpi
                label="Cycles"
                value={formatCount(data?.cycles)}
                series={cycles}
                trend="accent"
                note="loop iterations since this process started"
              />
              <Kpi
                label="Events logged"
                value={formatCount(data?.events_logged)}
                series={events}
                trend="accent"
                note={
                  status.data?.archived === null
                    ? "in-memory only — nothing archived"
                    : `${formatCount(status.data?.archived)} archived to storage`
                }
                tone={status.data?.archived === null ? "warn" : "neutral"}
              />
              <Kpi
                label="Opportunities queued"
                value={formatCount(data?.opportunities_queued)}
                series={queued}
                note="found by DISCOVER, awaiting REASON"
              />
              <Kpi
                label="Proposals"
                value={formatCount(data?.proposals)}
                series={proposals}
                note="cleared the action bar in DECIDE"
              />
              <Kpi
                label="Orders"
                value={formatCount(data?.orders)}
                series={orders}
                note="released by ACT against the simulator"
              />
              <Kpi
                label="Fills"
                value={formatCount(data?.fills)}
                series={fills}
                tone={data?.live_fills ? "bad" : "neutral"}
                note={data?.live_fills ? "A LIVE FILL IS PRESENT" : "every fill simulated"}
              />
              <Kpi
                label="Refusals"
                value={formatCount(data?.refusals)}
                series={refusals}
                trend="down"
                tone={data !== undefined && data !== null && data.refusals > 0 ? "warn" : "neutral"}
                note="risk said no before an order existed"
              />
              <Kpi
                label="Reconciliation breaks"
                value={breaks === null ? "—" : formatCount(breaks)}
                tone={breaks !== null && breaks > 0 ? "bad" : "ok"}
                note="the book against the venue"
              />
            </KpiRow>
          )}
        </PanelBody>
      </Panel>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[2fr_1fr]">
        <Panel>
          <PanelHead
            title="Event log growth"
            meta={<Freshness resource={metrics} name="metrics" />}
            actions={<Chip>observed by this tab</Chip>}
          />
          <PanelBody>
            <AreaChart
              values={events.values}
              label="events logged"
              height={168}
              caption={
                <>
                  {describeWindow(events)}. The platform serves a counter, not a curve; this line is
                  what this browser watched it do. Reload and it starts again.
                </>
              }
            />
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead title="Decision funnel" meta={<Freshness resource={metrics} name="metrics" />} />
          <PanelBody>
            <ResourceView resource={metrics} loadingRows={4}>
              {(m) => (
                <div className="flex flex-col gap-3">
                  <Bars
                    items={[
                      { label: "Queued", value: m.opportunities_queued, tone: "accent" },
                      { label: "Proposed", value: m.proposals, tone: "accent" },
                      { label: "Ordered", value: m.orders, tone: "accent" },
                      { label: "Filled", value: m.fills, tone: "up" },
                      { label: "Refused", value: m.refusals, tone: "down" },
                    ]}
                  />
                  <div className="flex items-center justify-between gap-3 border-t border-[color:var(--color-line)] pt-3">
                    {refusalRate === null ? (
                      <p className="text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
                        No refusal rate: nothing has been ruled on yet. A rate of 0% here would say
                        the risk engine is passing everything, which is a different claim.
                      </p>
                    ) : (
                      <Gauge
                        fraction={refusalRate}
                        label="refused"
                        caption={`${formatCount(m.refusals)} of ${formatCount(ruled)} ruled on`}
                        tone={refusalRate > 0.5 ? "warn" : "accent"}
                      />
                    )}
                    <Chip tone={m.live_fills ? "bad" : "ok"}>
                      {m.live_fills ? "live fill present" : "paper only"}
                    </Chip>
                  </div>
                </div>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>
      </div>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead title="Risk posture" meta={<Freshness resource={risk} name="risk" />} />
          <PanelBody>
            <ResourceView resource={risk} loadingRows={4}>
              {(r) => {
                const concentrations = r.concentrations;
                return (
                  <div className="flex flex-col gap-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <StatusChip
                        tone={r.kill_switch.halted ? "bad" : "ok"}
                        label={r.kill_switch.halted ? "kill switch tripped" : "kill switch clear"}
                      />
                      {r.kill_switch.halted_scopes.map((scope) => (
                        <Chip key={scope} tone="bad">
                          {scope}
                        </Chip>
                      ))}
                      <Chip>{r.kill_switch.clearances} clearance(s)</Chip>
                    </div>
                    {r.kill_switch.halted && r.kill_switch.reason ? (
                      <p className="text-[11.5px] text-[color:var(--color-ink-dim)]">
                        Tripped by <span className="num">{r.kill_switch.tripped_by || "unknown"}</span>:{" "}
                        {r.kill_switch.reason}
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
              {(q) =>
                q.opportunities.length === 0 ? (
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
                        </tr>
                      </thead>
                      <tbody>
                        {q.opportunities.map((opportunity) => (
                          <tr key={opportunity.id}>
                            <td className="num">{opportunity.id}</td>
                            <td className="whitespace-normal">{opportunity.headline}</td>
                            <td className="n">{opportunity.score.toFixed(3)}</td>
                            <td className="n">{formatPercent(opportunity.confidence)}</td>
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

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[1fr_2fr]">
        <Panel>
          <PanelHead title="Book" meta={<Freshness resource={portfolio} name="portfolio" />} />
          <PanelBody>
            <ResourceView resource={portfolio} loadingRows={3}>
              {(book) => (
                <div className="flex flex-col gap-2">
                  <KpiRow>
                    <Kpi label="Proposals" value={formatCount(book.proposals)} />
                    <Kpi label="Orders" value={formatCount(book.orders)} />
                    <Kpi label="Fills" value={formatCount(book.fills)} />
                  </KpiRow>
                  <Chip tone={book.paper_only ? "ok" : "bad"}>
                    {book.paper_only ? "paper only" : "NO — live fills present"}
                  </Chip>
                  <p className="text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
                    Position-level detail sits behind the desk&rsquo;s capability gate and is not
                    served over HTTP. These are counts, not a book.
                  </p>
                </div>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>

        <RunCycleCard
          onRan={() => {
            metrics.refresh();
            portfolio.refresh();
            opportunities.refresh();
            status.refresh();
          }}
        />
      </div>
    </div>
  );
}
