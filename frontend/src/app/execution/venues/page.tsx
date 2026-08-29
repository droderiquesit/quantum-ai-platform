"use client";

import { useMemo } from "react";
import { Chip, Freshness } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { Strategies } from "@/lib/api/types";
import { formatCount } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

interface VenueRow {
  readonly venue: string;
  readonly candidates: number;
  readonly holdingCapital: number;
}

/**
 * The venues this deployment actually touches, derived from the one surface
 * that names them today: the strategy register.
 *
 * There is no venue directory endpoint, and this page does not pretend one
 * exists. Each registered strategy candidate carries the venue it trades on,
 * so the table below is an aggregation of real registrations — a venue appears
 * here because something is registered against it, and for no other reason.
 * The market and asset reads are made alongside it and rendered exactly as the
 * platform answers, gates and all.
 */
export default function VenueStatusPage() {
  const strategies = useResource<Strategies>(platform.strategies, {
    key: "execution-venues-strategies",
    label: "GET /strategies",
    intervalMs: 15_000,
  });
  const markets = useResource<unknown>(platform.markets, {
    key: "execution-venues-markets",
    label: "GET /markets",
    intervalMs: 30_000,
  });
  const assets = useResource<unknown>(platform.assets, {
    key: "execution-venues-assets",
    label: "GET /assets",
    intervalMs: 60_000,
  });

  const venueRows = useMemo<readonly VenueRow[]>(() => {
    const byVenue = new Map<string, { candidates: number; holdingCapital: number }>();
    for (const candidate of strategies.data?.strategies ?? []) {
      const entry = byVenue.get(candidate.venue) ?? { candidates: 0, holdingCapital: 0 };
      entry.candidates += 1;
      if (candidate.holds_capital) entry.holdingCapital += 1;
      byVenue.set(candidate.venue, entry);
    }
    return [...byVenue.entries()]
      .sort((a, b) => b[1].candidates - a[1].candidates || a[0].localeCompare(b[0]))
      .map(([venue, counts]) => ({
        venue,
        candidates: counts.candidates,
        holdingCapital: counts.holdingCapital,
      }));
  }, [strategies.data]);

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Venues, from strategy registrations"
          meta={<Freshness resource={strategies} name="strategies" />}
          actions={
            strategies.data ? (
              <Chip>
                {formatCount(strategies.data.strategies.length)} candidate(s) registered
              </Chip>
            ) : null
          }
        />
        <PanelBody flush>
          <ResourceView resource={strategies} loadingRows={5}>
            {(data) =>
              data.strategies.length === 0 ? (
                <EmptyBlock headline="No venue can be listed, because no strategy is registered.">
                  <p>
                    The venue list on this page derives entirely from strategy registrations, and{" "}
                    <code className="num">GET /api/v1/strategies</code> answered with none. A venue
                    the platform is not registered against is a venue this console has no evidence
                    of, so nothing is listed in its place.
                  </p>
                </EmptyBlock>
              ) : (
                <TableWell maxHeight="40vh" label="Venues">
                  <table className="dt">
                    <thead>
                      <tr>
                        <th scope="col">Venue</th>
                        <th scope="col" className="n">
                          Candidates registered
                        </th>
                        <th scope="col" className="n">
                          Holding capital
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {venueRows.map((row) => (
                        <tr key={row.venue}>
                          <td className="num">{row.venue}</td>
                          <td className="n">{formatCount(row.candidates)}</td>
                          <td className="n">{formatCount(row.holdingCapital)}</td>
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

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <Panel>
          <PanelHead
            title="Market state (REST)"
            meta={<Freshness resource={markets} name="market state" />}
            actions={<Chip>GET /api/v1/markets</Chip>}
          />
          <PanelBody>
            <ResourceView resource={markets} loadingRows={3}>
              {(data) => <RawJson value={data} />}
            </ResourceView>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead
            title="Reference universe"
            meta={<Freshness resource={assets} name="assets" />}
            actions={<Chip>GET /api/v1/assets</Chip>}
          />
          <PanelBody>
            <ResourceView resource={assets} loadingRows={3}>
              {(data) => <RawJson value={data} />}
            </ResourceView>
          </PanelBody>
        </Panel>
      </div>
    </div>
  );
}

/**
 * The platform answered with a shape this console does not model. Showing it
 * verbatim is the honest option; inventing a table for it is not.
 */
function RawJson({ value }: { value: unknown }) {
  return (
    <pre className="num max-h-[220px] overflow-auto whitespace-pre-wrap break-all border border-[color:var(--color-line)] bg-[color:var(--color-sunken)] p-2 text-[11px] text-[color:var(--color-ink-dim)]">
      {JSON.stringify(value, null, 2)}
    </pre>
  );
}
