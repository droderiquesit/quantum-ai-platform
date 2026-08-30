"use client";

import { Chip, Freshness, StreamControls } from "@/components/data/Bits";
import { EventFeed } from "@/components/data/EventFeed";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { EmptyBlock, MissingEndpointBlock, ResourceView } from "@/components/data/States";
import { Bars } from "@/components/viz/primitives";
import { platform } from "@/lib/api/client";
import { NOT_YET_SERVED } from "@/lib/api/endpoints";
import type { Portfolio, Regions } from "@/lib/api/types";
import { formatCount } from "@/lib/format";
import { useEventStream } from "@/lib/hooks/useEventStream";
import { useResource } from "@/lib/hooks/useResource";

/**
 * The positions page, without position rows — because the platform does not
 * serve them, and a table of invented rows here would be the most dangerous
 * screen in the console.
 *
 * What it renders instead is every real position-adjacent fact the platform
 * does serve: the book counts, each cell's own count of open positions from
 * its regional report, and the live positions stream. The row-level book sits
 * behind the desk's capability gate, and that gap is stated by name rather
 * than papered over with a zero — a zero from an endpoint that never returns
 * positions reads as a flat book, which is the one thing this page must never
 * claim.
 */
export default function PositionsPage() {
  const portfolio = useResource<Portfolio>(platform.portfolio, {
    key: "positions-portfolio",
    label: "GET /portfolio",
    intervalMs: 15_000,
  });
  const regions = useResource<Regions>(platform.regions, {
    key: "positions-regions",
    label: "GET /regions",
    intervalMs: 20_000,
  });
  const positions = useEventStream({ channel: "positions", label: "positions stream" });

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Book counts"
          meta={<Freshness resource={portfolio} name="portfolio" />}
          actions={<Chip>GET /api/v1/portfolio</Chip>}
        />
        <PanelBody>
          <ResourceView resource={portfolio} loadingRows={2}>
            {(data) => (
              <div className="flex flex-col gap-2">
                <KpiRow>
                  <Kpi label="Proposals" value={formatCount(data.proposals)} note="staged by the loop" />
                  <Kpi label="Orders" value={formatCount(data.orders)} note="all states" />
                  <Kpi label="Fills" value={formatCount(data.fills)} note="recorded by this process" />
                  <Kpi
                    label="Execution posture"
                    value={data.paper_only ? "PAPER TRADING" : "LIVE FILLS PRESENT"}
                    tone={data.paper_only ? "ok" : "bad"}
                    note="whether any fill came from a real venue"
                  />
                </KpiRow>
                <p className="text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
                  These are counts, not positions. Nothing here says what the book holds — only how
                  much has moved through it.
                </p>
              </div>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Position rows" actions={<Chip tone="warn">no endpoint</Chip>} />
        <PanelBody>
          <MissingEndpointBlock endpoint={NOT_YET_SERVED["positions"]!} />
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Positions per cell"
          meta={<Freshness resource={regions} name="regions" />}
          actions={<Chip>GET /api/v1/regions</Chip>}
        />
        <PanelBody>
          <ResourceView resource={regions} loadingRows={4}>
            {(data) =>
              data.cells.length === 0 ? (
                <EmptyBlock headline="No cell has reported, so there is no count to show.">
                  <p>
                    <code className="num">GET /api/v1/regions</code> answered with an empty cell
                    list. That is a measured zero — no regional cell has filed a report in this
                    deployment — not a failure to read the endpoint, which would render as its own
                    state above this panel.
                  </p>
                </EmptyBlock>
              ) : (
                <div className="flex flex-col gap-2">
                  <Bars
                    items={data.cells.map((cell) => ({
                      label: cell.stale ? `${cell.cell} (stale)` : cell.cell,
                      value: cell.positions,
                      tone: cell.halted ? ("down" as const) : ("accent" as const),
                    }))}
                    unit=" pos"
                  />
                  <p className="text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
                    Each count is the cell reporting on its own book — the only real
                    position-adjacent numbers this console has. The rows behind each count stay in
                    the cell. A cell marked stale last reported outside the {data.freshness_bound}{" "}
                    freshness bound, so its count is that old.
                  </p>
                </div>
              )
            }
          </ResourceView>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Position events, live"
          meta={<StreamControls stream={positions} name="positions" />}
          actions={<Chip>SSE /api/v1/stream/positions</Chip>}
        />
        <PanelBody flush>
          <EventFeed stream={positions} channel="positions" maxHeight="34vh" />
        </PanelBody>
      </Panel>
    </div>
  );
}
