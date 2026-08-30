"use client";

import { Chip, Freshness } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView, StateBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { MeshStatus, Regions } from "@/lib/api/types";
import { formatCount, formatDecimal } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

/**
 * The backbone between cells and centre, counter by counter.
 *
 * Per ADR 0011 the mesh replaced managed pub/sub: cells publish state deltas
 * up and poll capital envelopes down, and a cell that cannot reach the centre
 * keeps trading inside the envelopes it already holds and stops when they
 * expire. That sentence is the design, so when `/mesh` answers with an absence
 * or an error this page shows the platform's own words rather than a
 * paraphrase — the reason the backbone is not serving is a fact the platform
 * states, not one this console gets to soften.
 */
export default function MeshPage() {
  const mesh = useResource<MeshStatus>(platform.mesh, {
    key: "operations-mesh",
    label: "GET /mesh",
    intervalMs: 10_000,
  });
  const regions = useResource<Regions>(platform.regions, {
    key: "mesh-regions",
    label: "GET /regions",
    intervalMs: 10_000,
  });

  const inbox = mesh.data?.inbox_depth ?? null;

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Mesh backbone"
          meta={<Freshness resource={mesh} name="mesh" />}
          actions={
            mesh.data === null ? null : mesh.data.error !== undefined ? (
              <Chip tone="bad">reporting an error</Chip>
            ) : mesh.data.served ? (
              <Chip tone="ok">served</Chip>
            ) : (
              <Chip tone="warn">not served</Chip>
            )
          }
        />
        <PanelBody>
          <ResourceView resource={mesh} loadingRows={2}>
            {(m) =>
              m.error !== undefined ? (
                // The platform can report served-and-broken — a backbone that is
                // bound but inconsistent. Its own words are the diagnosis.
                <StateBlock
                  tone="bad"
                  label="mesh error"
                  headline="The platform reports an error on the mesh backbone."
                >
                  <p className="num">{m.error}</p>
                </StateBlock>
              ) : !m.served ? (
                <StateBlock
                  tone="warn"
                  label="not served"
                  headline="The platform answered, and the answer is that no mesh is being served."
                >
                  <p>
                    Cells publishing deltas have nowhere to land them and no new envelopes are
                    being dispatched. Each cell keeps trading inside the envelopes it already
                    holds and stops when they expire.
                  </p>
                </StateBlock>
              ) : (
                <KpiRow>
                  <Kpi
                    label="Cells served"
                    value={formatCount(m.cells_served)}
                    note="cells the centre has exchanged with"
                  />
                  <Kpi
                    label="Deltas absorbed"
                    value={formatCount(m.deltas_absorbed)}
                    note="cell state published up and taken in"
                  />
                  <Kpi
                    label="Envelopes dispatched"
                    value={formatCount(m.envelopes_dispatched)}
                    note="capital grants sent down for cells to poll"
                  />
                  <Kpi
                    label="Inbox depth"
                    value={formatCount(m.inbox_depth)}
                    tone={inbox !== null && inbox > 0 ? "warn" : "neutral"}
                    note={
                      inbox !== null && inbox > 0
                        ? "deltas received and not yet absorbed"
                        : "nothing waiting to be absorbed"
                    }
                  />
                </KpiRow>
              )
            }
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Cells on the backbone"
          meta={<Freshness resource={regions} name="regions" />}
        />
        <PanelBody flush>
          <ResourceView resource={regions} loadingRows={4}>
            {(r) =>
              r.cells.length === 0 ? (
                <EmptyBlock headline="No cell has reported to the centre.">
                  <p>
                    An empty list here means no regional cell has published a state delta yet —
                    not that the backbone is down. The panel above says whether the backbone is
                    serving.
                  </p>
                </EmptyBlock>
              ) : (
                <TableWell maxHeight="380px" label="Edge cells">
                  <table className="dt">
                    <thead>
                      <tr>
                        <th scope="col">Cell</th>
                        <th scope="col">Last report age</th>
                        <th scope="col" className="n">
                          Positions
                        </th>
                        <th scope="col" className="n">
                          Strategies
                        </th>
                        <th scope="col" className="n">
                          Gross
                        </th>
                        <th scope="col" className="n">
                          Breaks
                        </th>
                        <th scope="col">State</th>
                      </tr>
                    </thead>
                    <tbody>
                      {r.cells.map((cell) => (
                        <tr
                          key={cell.cell}
                          data-alert={cell.stale || cell.halted ? "true" : undefined}
                        >
                          <td className="num">{cell.cell}</td>
                          <td className="num">{cell.age}</td>
                          <td className="n">{formatCount(cell.positions)}</td>
                          <td className="n">{formatCount(cell.strategies)}</td>
                          <td className="n">{formatDecimal(cell.gross)}</td>
                          <td className="n">{formatCount(cell.reconciliation_breaks)}</td>
                          <td className="flex gap-1">
                            <Chip tone={cell.stale ? "warn" : "ok"}>
                              {cell.stale ? "stale" : "fresh"}
                            </Chip>
                            {cell.halted ? <Chip tone="bad">halted</Chip> : null}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </TableWell>
              )
            }
          </ResourceView>
          {regions.data !== null ? (
            <p className="px-3 py-2 text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
              A cell is stale when its last report is older than the platform&rsquo;s freshness
              bound of {regions.data.freshness_bound}. Stale is not halted: per ADR 0008 a cell
              that cannot reach the centre decides alone inside its envelopes.
            </p>
          ) : null}
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="How the backbone works" />
        <PanelBody>
          <p className="max-w-[90ch] text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
            Per ADR 0011 the mesh replaced managed pub/sub. Cells publish state deltas up and
            poll capital envelopes down; a cell that cannot reach the centre keeps trading
            inside the envelopes it already holds and stops when they expire. Disconnection is
            therefore a bounded condition, not an emergency — the emergency is an envelope that
            never expires, which is why every envelope carries its own clock.
          </p>
        </PanelBody>
      </Panel>
    </div>
  );
}
