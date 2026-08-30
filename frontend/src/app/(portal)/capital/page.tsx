"use client";

import { Chip, Freshness, KeyValue } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { Bars, Gauge } from "@/components/viz/primitives";
import { platform } from "@/lib/api/client";
import type { Capital, Regions } from "@/lib/api/types";
import { formatCount, formatDecimal, formatTimestamp } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * The bounds capital is allocated within, and every envelope issued against
 * them.
 *
 * Bounds and use are read as two separate facts and shown as two separate
 * facts. An envelope reports its own use, and when it has not reported, this
 * page says "not reported" rather than showing zero — a cell that has gone
 * quiet holding an envelope is the condition a recall exists for, and it must
 * not look like a cell that is holding one and doing nothing with it.
 *
 * Every figure here is a decimal string from the platform, rendered without
 * being parsed. Money is never turned into a float in this browser.
 */
export default function CapitalPage() {
  const capital = useResource<Capital>(platform.capital, {
    key: "capital",
    label: "GET /capital",
    intervalMs: 15_000,
  });
  const regions = useResource<Regions>(platform.regions, {
    key: "capital-regions",
    label: "GET /regions",
    intervalMs: 20_000,
  });

  const data = capital.data;
  const envelopes = data?.envelopes ?? [];
  const recalls = data?.outstanding_recalls ?? [];
  const reported = envelopes.filter((envelope) => envelope.used.reported);
  const silent = envelopes.length - reported.length;

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Capital"
          meta={<Freshness resource={capital} name="capital" />}
          actions={
            recalls.length > 0 ? (
              <Chip tone="bad">{recalls.length} outstanding recall(s)</Chip>
            ) : (
              <Chip tone="ok">no recall outstanding</Chip>
            )
          }
        />
        <PanelBody>
          <KpiRow>
            <Kpi
              label="Envelopes issued"
              value={formatCount(envelopes.length)}
              note="grants a cell may trade inside"
            />
            <Kpi
              label="Reporting use"
              value={formatCount(reported.length)}
              note={`${formatCount(silent)} have not reported`}
              tone={silent > 0 ? "warn" : "neutral"}
            />
            <Kpi
              label="Outstanding recalls"
              value={formatCount(recalls.length)}
              tone={recalls.length > 0 ? "bad" : "ok"}
              note="issued and not yet acknowledged"
            />
            <Kpi
              label="Total budget"
              value={formatDecimal(data?.bounds.total_budget)}
              note="the ceiling every other bound sits under"
            />
          </KpiRow>
        </PanelBody>
      </Panel>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[1fr_2fr]">
        <Panel>
          <PanelHead title="Bounds" meta={<Freshness resource={capital} name="bounds" />} />
          <PanelBody>
            <ResourceView resource={capital} loadingRows={4}>
              {(c) => (
                <div className="flex flex-col gap-3">
                  <dl className="flex flex-col">
                    <KeyValue label="Total budget">{formatDecimal(c.bounds.total_budget)}</KeyValue>
                    <KeyValue label="Per strategy">{formatDecimal(c.bounds.per_strategy)}</KeyValue>
                    <KeyValue label="Per cell">{formatDecimal(c.bounds.per_cell)}</KeyValue>
                    <KeyValue label="Per venue">{formatDecimal(c.bounds.per_venue)}</KeyValue>
                  </dl>
                  <p className="text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
                    These are the bounds an envelope is cut from. They are limits on what may be
                    granted, not a statement of what is deployed.
                  </p>
                </div>
              )}
            </ResourceView>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead title="Envelopes" meta={<Freshness resource={capital} name="envelopes" />} />
          <PanelBody flush>
            <ResourceView resource={capital} loadingRows={5}>
              {(c) =>
                c.envelopes.length === 0 ? (
                  <EmptyBlock headline="No envelope is outstanding.">
                    <p>
                      No cell holds a grant right now. An envelope is issued when a strategy is
                      allocated capital and expires on its own clock, so an empty list means the
                      desk has nothing deployed — not that the allocator is unreachable.
                    </p>
                  </EmptyBlock>
                ) : (
                  <TableWell maxHeight="380px" label="Capital envelopes">
                    <table className="dt">
                      <thead>
                        <tr>
                          <th scope="col">Cell</th>
                          <th scope="col">Strategy</th>
                          <th scope="col" className="n">
                            Gross limit
                          </th>
                          <th scope="col" className="n">
                            Committed
                          </th>
                          <th scope="col" className="n">
                            Orders
                          </th>
                          <th scope="col">Expires</th>
                        </tr>
                      </thead>
                      <tbody>
                        {c.envelopes.map((envelope) => (
                          <tr
                            key={`${envelope.cell}:${envelope.strategy}`}
                            data-alert={envelope.used.reported ? undefined : "true"}
                          >
                            <td className="num">{envelope.cell}</td>
                            <td className="num">{envelope.strategy}</td>
                            <td className="n">{formatDecimal(envelope.gross_limit)}</td>
                            <td className="n">
                              {envelope.used.reported ? (
                                formatDecimal(envelope.used.gross_committed)
                              ) : (
                                <span
                                  className="text-[color:var(--color-warn)]"
                                  title={envelope.used.reason}
                                >
                                  not reported
                                </span>
                              )}
                            </td>
                            <td className="n">
                              {envelope.used.reported ? formatCount(envelope.used.orders_sent) : "—"}
                            </td>
                            <td className="num text-[10.5px]">
                              {formatTimestamp(envelope.expires_at)}
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

      {recalls.length > 0 ? (
        <Panel>
          <PanelHead
            title="Outstanding recalls"
            meta={<Freshness resource={capital} name="recalls" />}
            actions={<Chip tone="bad">{recalls.length}</Chip>}
          />
          <PanelBody flush>
            <TableWell maxHeight="320px" label="Outstanding recalls">
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
                    <th scope="col">Backstop expiry</th>
                  </tr>
                </thead>
                <tbody>
                  {recalls.map((recall) => (
                    <tr key={`${recall.cell}:${recall.strategy}:${recall.issued_at}`} data-alert="true">
                      <td className="num">{recall.cell}</td>
                      <td className="num">{recall.strategy}</td>
                      <td className="whitespace-normal">
                        {recall.reason}
                        <span className="block text-[11px] text-[color:var(--color-ink-dim)]">
                          {recall.detail}
                        </span>
                      </td>
                      <td className="n">{formatDecimal(recall.gross_recalled)}</td>
                      <td className="num text-[10.5px]">{formatTimestamp(recall.acknowledge_by)}</td>
                      <td className="num text-[10.5px]">
                        {formatTimestamp(recall.backstop_expiry)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </TableWell>
          </PanelBody>
        </Panel>
      ) : null}

      <Panel>
        <PanelHead title="Where it is deployed" meta={<Freshness resource={regions} name="regions" />} />
        <PanelBody>
          <ResourceView resource={regions} loadingRows={4}>
            {(r) =>
              r.cells.length === 0 ? (
                <EmptyBlock headline="No cell has reported." />
              ) : (
                <div className="flex flex-wrap items-start gap-6">
                  <Bars
                    items={r.cells.map((cell) => ({
                      label: cell.cell,
                      value: cell.positions,
                      tone: cell.halted ? "down" : "accent",
                    }))}
                    unit=" pos"
                  />
                  <Gauge
                    fraction={
                      r.cells.length === 0
                        ? 0
                        : r.cells.filter((cell) => !cell.stale).length / r.cells.length
                    }
                    label="cells fresh"
                    caption={`${r.cells.filter((cell) => !cell.stale).length} of ${r.cells.length} inside ${r.freshness_bound}`}
                    tone={r.cells.some((cell) => cell.stale) ? "warn" : "ok"}
                  />
                </div>
              )
            }
          </ResourceView>
        </PanelBody>
      </Panel>
    </div>
  );
}
