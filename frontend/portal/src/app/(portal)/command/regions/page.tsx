"use client";

import { Chip, Freshness, KeyValue } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { Bars, Gauge } from "@/components/viz/primitives";
import { platform } from "@/lib/api/client";
import type { MeshStatus, Regions } from "@/lib/api/types";
import { directionOf, formatCount, formatDecimal, formatTimestamp } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * Every edge cell the platform has heard from, and how recently.
 *
 * Two rules keep this page honest. First, it renders only cells the platform
 * reports — a hard-coded list of seven regions would show a healthy row for a
 * cell that has never spoken, which is the exact failure a status page exists
 * to expose. Second, the headline figures are dashes until /regions answers:
 * "0 halted" read off a page that could not reach the platform is a claim
 * nobody measured.
 */
export default function RegionalBrainStatusPage() {
  const regions = useResource<Regions>(platform.regions, {
    key: "command-regions",
    label: "GET /regions",
    intervalMs: 15_000,
  });
  const mesh = useResource<MeshStatus>(platform.mesh, {
    key: "command-regions-mesh",
    label: "GET /mesh",
    intervalMs: 15_000,
  });

  // undefined (not zero) until an answer with cells in it has landed, so the
  // KPI row shows "—" rather than a fabricated all-clear while unreadable.
  const cells = regions.data?.cells;
  const staleCount = cells?.filter((cell) => cell.stale).length;
  const haltedCount = cells?.filter((cell) => cell.halted).length;
  const breakCount = cells?.reduce((sum, cell) => sum + cell.reconciliation_breaks, 0);

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Regional brain status"
          meta={<Freshness resource={regions} name="regions" />}
          actions={
            haltedCount !== undefined && haltedCount > 0 ? (
              <Chip tone="bad">{formatCount(haltedCount)} cell(s) halted</Chip>
            ) : staleCount !== undefined && staleCount > 0 ? (
              <Chip tone="warn">{formatCount(staleCount)} cell(s) stale</Chip>
            ) : cells !== undefined ? (
              <Chip tone="ok">all reported cells fresh</Chip>
            ) : null
          }
        />
        <PanelBody>
          <KpiRow>
            <Kpi
              label="Cells reporting"
              value={formatCount(cells?.length)}
              note="only cells the platform has heard from are shown"
            />
            <Kpi
              label="Stale"
              value={formatCount(staleCount)}
              tone={staleCount === undefined ? "neutral" : staleCount > 0 ? "warn" : "ok"}
              note={
                regions.data
                  ? `last report older than ${regions.data.freshness_bound}`
                  : "freshness bound not yet read"
              }
            />
            <Kpi
              label="Halted"
              value={formatCount(haltedCount)}
              tone={haltedCount === undefined ? "neutral" : haltedCount > 0 ? "bad" : "ok"}
              note="a halted cell keeps its book and stops trading"
            />
            <Kpi
              label="Reconciliation breaks"
              value={formatCount(breakCount)}
              tone={breakCount === undefined ? "neutral" : breakCount > 0 ? "bad" : "ok"}
              note="summed across every reporting cell"
            />
          </KpiRow>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Cells" meta={<Freshness resource={regions} name="cells" />} />
        <PanelBody flush>
          <ResourceView resource={regions} loadingRows={5}>
            {(data) =>
              data.cells.length === 0 ? (
                <EmptyBlock headline="The platform answered, and no cell has reported.">
                  <p>
                    This is a measured zero, not a read failure: /regions responded and its list
                    was empty. A cell appears here only after it has sent an observation, so an
                    empty fleet means no edge cell has spoken — not that this console is blind.
                  </p>
                </EmptyBlock>
              ) : (
                <TableWell maxHeight="420px" label="Edge cell observations">
                  <table className="dt">
                    <thead>
                      <tr>
                        <th scope="col">Cell</th>
                        <th scope="col">Reported</th>
                        <th scope="col" className="n">
                          Age
                        </th>
                        <th scope="col">State</th>
                        <th scope="col" className="n">
                          Positions
                        </th>
                        <th scope="col" className="n">
                          Strategies
                        </th>
                        <th scope="col" className="n">
                          Breaks
                        </th>
                        <th scope="col" className="n">
                          Gross
                        </th>
                        <th scope="col" className="n">
                          Net
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {data.cells.map((cell) => (
                        <tr
                          key={cell.cell}
                          data-alert={
                            cell.halted || cell.reconciliation_breaks > 0 ? "true" : undefined
                          }
                        >
                          <td className="num">{cell.cell}</td>
                          <td className="num text-[10.5px]">{formatTimestamp(cell.reported_at)}</td>
                          <td className="n">{cell.age}</td>
                          <td>
                            {cell.halted ? (
                              <Chip tone="bad">halted</Chip>
                            ) : cell.stale ? (
                              <Chip tone="warn">stale</Chip>
                            ) : (
                              <Chip tone="ok">fresh</Chip>
                            )}
                          </td>
                          <td className="n">{formatCount(cell.positions)}</td>
                          <td className="n">{formatCount(cell.strategies)}</td>
                          <td
                            className="n"
                            data-direction={
                              cell.reconciliation_breaks > 0 ? "negative" : undefined
                            }
                          >
                            {formatCount(cell.reconciliation_breaks)}
                          </td>
                          <td className="n">{formatDecimal(cell.gross)}</td>
                          <td className="n" data-direction={directionOf(cell.net)}>
                            {formatDecimal(cell.net)}
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

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[2fr_1fr]">
        <Panel>
          <PanelHead
            title="Positions and freshness"
            meta={<Freshness resource={regions} name="positions per cell" />}
          />
          <PanelBody>
            <ResourceView resource={regions} loadingRows={4}>
              {(data) =>
                data.cells.length === 0 ? (
                  <EmptyBlock headline="No cell has reported, so there is nothing to rank." />
                ) : (
                  <div className="flex flex-wrap items-start gap-6">
                    <div className="min-w-[240px] flex-1">
                      <Bars
                        items={data.cells.map((cell) => ({
                          label: cell.cell,
                          value: cell.positions,
                          tone: cell.halted ? ("down" as const) : ("accent" as const),
                        }))}
                        unit=" pos"
                      />
                    </div>
                    <Gauge
                      fraction={
                        data.cells.filter((cell) => !cell.stale).length / data.cells.length
                      }
                      label="cells fresh"
                      caption={`${data.cells.filter((cell) => !cell.stale).length} of ${data.cells.length} inside ${data.freshness_bound}`}
                      tone={data.cells.some((cell) => cell.stale) ? "warn" : "ok"}
                    />
                  </div>
                )
              }
            </ResourceView>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead title="Mesh backbone" meta={<Freshness resource={mesh} name="mesh" />} />
          <PanelBody>
            <ResourceView resource={mesh} loadingRows={4}>
              {(data) =>
                data.served ? (
                  <div className="flex flex-col gap-3">
                    <dl className="flex flex-col">
                      <KeyValue label="Cells served">{formatCount(data.cells_served)}</KeyValue>
                      <KeyValue label="Deltas absorbed">
                        {formatCount(data.deltas_absorbed)}
                      </KeyValue>
                      <KeyValue label="Envelopes dispatched">
                        {formatCount(data.envelopes_dispatched)}
                      </KeyValue>
                      <KeyValue label="Inbox depth">{formatCount(data.inbox_depth)}</KeyValue>
                    </dl>
                    <p className="text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
                      The mesh is how cell observations reach this page at all. Cells decide alone
                      by design, so a quiet mesh degrades this view before it degrades any cell.
                    </p>
                  </div>
                ) : (
                  <EmptyBlock headline="The mesh is not being served.">
                    <p>
                      {data.error ??
                        "The platform answered and reported the mesh as not served, without a reason."}
                    </p>
                  </EmptyBlock>
                )
              }
            </ResourceView>
          </PanelBody>
        </Panel>
      </div>
    </div>
  );
}
