"use client";

import { Chip, Freshness, Metric, MetricRow, StreamControls } from "@/components/data/Bits";
import { EventFeed } from "@/components/data/EventFeed";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, MissingEndpointBlock, ResourceView } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import { NOT_YET_SERVED } from "@/lib/api/endpoints";
import type { Capital, Portfolio as PortfolioSummary, Strategies } from "@/lib/api/types";
import { formatCount, formatDecimal, formatTimestamp } from "@/lib/format";
import { useEventStream } from "@/lib/hooks/useEventStream";
import { useResource } from "@/lib/hooks/useResource";

/**
 * The book, the capital behind it, and the two things the platform will not
 * hand over: position rows and a cash ledger.
 *
 * Both absences are named rather than smoothed over. A portfolio page that
 * showed a zero position count from an endpoint that never returns positions
 * would be the single most dangerous screen in this console.
 */
export default function PortfolioPage() {
  const summary = useResource<PortfolioSummary>(platform.portfolio, {
    key: "portfolio",
    label: "GET /portfolio",
    intervalMs: 10_000,
  });
  const capital = useResource<Capital>(platform.capital, {
    key: "capital",
    label: "GET /capital",
    intervalMs: 15_000,
  });
  const strategies = useResource<Strategies>(platform.strategies, {
    key: "strategies",
    label: "GET /strategies",
    intervalMs: 30_000,
  });
  const pnl = useResource<unknown>(platform.pnl, {
    key: "pnl",
    label: "GET /pnl",
    intervalMs: 30_000,
  });
  const positions = useEventStream({
    channel: "positions",
    label: "SSE /stream/positions",
    maxEvents: 200,
  });

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Book summary"
          meta={<Freshness resource={summary} name="portfolio" />}
          actions={<Chip>GET /api/v1/portfolio</Chip>}
        />
        <PanelBody>
          <ResourceView resource={summary} loadingRows={2}>
            {(data) => (
              <>
                <MetricRow>
                  <Metric label="Proposals" value={formatCount(data.proposals)} hint="staged" />
                  <Metric label="Orders" value={formatCount(data.orders)} hint="all states" />
                  <Metric label="Fills" value={formatCount(data.fills)} hint="recorded" />
                  <Metric
                    label="Execution"
                    value={data.paper_only ? "paper only" : "LIVE FILLS PRESENT"}
                    tone={data.paper_only ? "ok" : "bad"}
                    hint="whether any fill came from a real venue"
                  />
                </MetricRow>
                <p className="mt-2 text-[11.5px] leading-relaxed text-[color:var(--color-ink-faint)]">
                  These are counts held by this process. They are not a valuation and not a book.
                </p>
              </>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead title="Positions" actions={<Chip tone="warn">no endpoint</Chip>} />
          <PanelBody>
            <MissingEndpointBlock endpoint={NOT_YET_SERVED["positions"]!} />
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead title="Cash and settlement" actions={<Chip tone="warn">no endpoint</Chip>} />
          <PanelBody>
            <MissingEndpointBlock endpoint={NOT_YET_SERVED["cash"]!} />
          </PanelBody>
        </Panel>
      </div>

      <Panel>
        <PanelHead
          title="Position, P&L and reconciliation events"
          meta={<StreamControls stream={positions} name="positions" />}
        />
        <PanelBody flush>
          <EventFeed stream={positions} channel="positions" maxHeight="34vh" />
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Profit and loss"
          meta={<Freshness resource={pnl} name="P&L" />}
          actions={<Chip>GET /api/v1/pnl</Chip>}
        />
        <PanelBody>
          <ResourceView resource={pnl} loadingRows={3}>
            {(data) => (
              <EmptyBlock headline="The platform returned a P&L body this console does not model.">
                <pre className="num mt-2 max-h-[200px] overflow-auto whitespace-pre-wrap break-all">
                  {JSON.stringify(data, null, 2)}
                </pre>
              </EmptyBlock>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Capital allocation"
          meta={<Freshness resource={capital} name="capital" />}
        />
        <PanelBody flush>
          <ResourceView resource={capital} loadingRows={4}>
            {(data) => (
              <>
                <div className="grid grid-cols-2 gap-x-6 gap-y-1 border-b border-[color:var(--color-line)] px-3 py-2 text-[11.5px] sm:grid-cols-4">
                  <Bound label="Total budget" value={data.bounds.total_budget} />
                  <Bound label="Per strategy" value={data.bounds.per_strategy} />
                  <Bound label="Per cell" value={data.bounds.per_cell} />
                  <Bound label="Per venue" value={data.bounds.per_venue} />
                </div>
                {data.envelopes.length === 0 ? (
                  <EmptyBlock headline="No capital envelope is issued.">
                    <p>
                      Observed, not assumed: the central plane holds the factory in this process and
                      reported an empty list.
                    </p>
                  </EmptyBlock>
                ) : (
                  <TableWell maxHeight="30vh" label="Capital envelopes">
                    <table className="dt">
                      <thead>
                        <tr>
                          <th scope="col">Cell</th>
                          <th scope="col">Strategy</th>
                          <th scope="col" className="n">
                            Gross limit
                          </th>
                          <th scope="col">Expires</th>
                          <th scope="col" className="n">
                            Committed
                          </th>
                          <th scope="col" className="n">
                            Orders sent
                          </th>
                        </tr>
                      </thead>
                      <tbody>
                        {data.envelopes.map((envelope) => (
                          <tr key={`${envelope.cell}:${envelope.strategy}`}>
                            <td className="num">{envelope.cell}</td>
                            <td className="num">{envelope.strategy}</td>
                            <td className="n">{formatDecimal(envelope.gross_limit)}</td>
                            <td className="num text-[10px] text-[color:var(--color-ink-dim)]">
                              {formatTimestamp(envelope.expires_at)}
                            </td>
                            {envelope.used.reported ? (
                              <>
                                <td className="n">{formatDecimal(envelope.used.gross_committed)}</td>
                                <td className="n">{formatCount(envelope.used.orders_sent)}</td>
                              </>
                            ) : (
                              <td
                                className="text-[11px] text-[color:var(--color-warn)]"
                                colSpan={2}
                                title={envelope.used.reason}
                              >
                                not reported
                              </td>
                            )}
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </TableWell>
                )}
                {data.outstanding_recalls.length > 0 ? (
                  <TableWell maxHeight="24vh" label="Outstanding capital recalls">
                    <table className="dt">
                      <thead>
                        <tr>
                          <th scope="col">Cell</th>
                          <th scope="col">Strategy</th>
                          <th scope="col">Reason</th>
                          <th scope="col" className="n">
                            Gross recalled
                          </th>
                          <th scope="col">Acknowledge by</th>
                          <th scope="col">Backstop</th>
                        </tr>
                      </thead>
                      <tbody>
                        {data.outstanding_recalls.map((recall) => (
                          <tr key={`${recall.cell}:${recall.strategy}:${recall.issued_at}`} data-alert="true">
                            <td className="num">{recall.cell}</td>
                            <td className="num">{recall.strategy}</td>
                            <td className="whitespace-normal text-[11.5px]" title={recall.detail}>
                              {recall.reason}
                            </td>
                            <td className="n">{formatDecimal(recall.gross_recalled)}</td>
                            <td className="num text-[10px]">{formatTimestamp(recall.acknowledge_by)}</td>
                            <td className="num text-[10px]">{formatTimestamp(recall.backstop_expiry)}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </TableWell>
                ) : null}
              </>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Strategies holding capital"
          meta={<Freshness resource={strategies} name="strategies" />}
        />
        <PanelBody flush>
          <ResourceView resource={strategies} loadingRows={3}>
            {(data) =>
              data.strategies.length === 0 ? (
                <EmptyBlock headline="No strategy is registered in this process." />
              ) : (
                <TableWell maxHeight="30vh" label="Strategies">
                  <table className="dt">
                    <thead>
                      <tr>
                        <th scope="col">Strategy</th>
                        <th scope="col">Cell</th>
                        <th scope="col">Venue</th>
                        <th scope="col">Stage</th>
                        <th scope="col">Capital</th>
                        <th scope="col">Registered</th>
                      </tr>
                    </thead>
                    <tbody>
                      {data.strategies.map((strategy) => (
                        <tr key={strategy.id}>
                          <td className="num">{strategy.id}</td>
                          <td className="num">{strategy.cell}</td>
                          <td className="num">{strategy.venue}</td>
                          <td>
                            <Chip tone="info">{strategy.stage}</Chip>
                          </td>
                          <td>
                            <Chip tone={strategy.holds_capital ? "ok" : "neutral"}>
                              {strategy.holds_capital ? "allocated" : "none"}
                            </Chip>
                          </td>
                          <td className="num text-[10px] text-[color:var(--color-ink-dim)]">
                            {formatTimestamp(strategy.registered_at)}
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
  );
}

function Bound({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between gap-2">
      <span className="text-[color:var(--color-ink-dim)]">{label}</span>
      <span className="num">{formatDecimal(value)}</span>
    </div>
  );
}
