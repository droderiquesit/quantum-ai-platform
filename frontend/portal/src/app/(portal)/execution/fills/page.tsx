"use client";

import { useMemo, useState } from "react";
import { Chip, Freshness } from "@/components/data/Bits";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { Fills, Orders } from "@/lib/api/types";
import { formatCount, formatDecimal } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";

type SideFilter = "all" | "buy" | "sell";

/**
 * Every fill this process has recorded, with the execution provenance of each
 * one on its own row.
 *
 * The headline figure is `any_live_fill`, because on a paper-only deployment
 * it has exactly one acceptable value. It is rendered from the platform's own
 * flag rather than derived by scanning the rows here: the platform's claim is
 * the audited one, and a browser-side recount that disagreed with it would be
 * a second source of truth for the single fact that matters most.
 */
export default function TradesAndFillsPage() {
  const fills = useResource<Fills>(platform.fills, {
    key: "execution-fills",
    label: "GET /fills",
    intervalMs: 10_000,
  });
  const orders = useResource<Orders>(platform.orders, {
    key: "execution-fills-orders",
    label: "GET /orders",
    intervalMs: 10_000,
  });

  const [side, setSide] = useState<SideFilter>("all");

  // Filtered in memory only. Nothing here decides anything; the platform's
  // list is rendered whole or narrowed, never recomputed.
  const visible = useMemo(() => {
    const rows = fills.data?.fills ?? [];
    if (side === "all") return rows;
    return rows.filter((fill) => fill.side.toLowerCase() === side);
  }, [fills.data, side]);

  const total = fills.data?.fills.length ?? 0;
  const anyLive = fills.data?.any_live_fill ?? null;

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead title="Trades &amp; fills" meta={<Freshness resource={fills} name="fills" />} />
        <PanelBody>
          <KpiRow>
            <Kpi label="Fills" value={formatCount(fills.data?.fills.length)} note="recorded by the platform" />
            <Kpi
              label="Refusals"
              value={formatCount(orders.data?.refusals)}
              tone={(orders.data?.refusals ?? 0) > 0 ? "warn" : "neutral"}
              note={orders.data === null ? "reading GET /orders" : "rejected before reaching a venue"}
            />
            <Kpi
              label="Any live fill"
              value={anyLive === null ? "—" : anyLive ? "YES" : "none"}
              tone={anyLive === null ? "neutral" : anyLive ? "bad" : "ok"}
              note={
                anyLive === null
                  ? "reading GET /fills"
                  : anyLive
                    ? "A LIVE FILL IS PRESENT"
                    : "every fill simulated"
              }
            />
          </KpiRow>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Fills"
          meta={<Freshness resource={fills} name="fills" />}
          actions={
            <div className="seg" role="group" aria-label="Filter fills by side">
              {(["all", "buy", "sell"] as const).map((option) => (
                <button
                  key={option}
                  type="button"
                  aria-pressed={side === option}
                  onClick={() => setSide(option)}
                >
                  {option}
                </button>
              ))}
            </div>
          }
        />
        <PanelBody flush>
          <ResourceView resource={fills} loadingRows={6}>
            {(data) =>
              data.fills.length === 0 ? (
                <EmptyBlock headline="No fill has occurred.">
                  <p>
                    Observed, not assumed: <code className="num">GET /api/v1/fills</code> answered
                    and its list is empty. Nothing has executed — which is not the same panel this
                    would be if the platform could not be reached.
                  </p>
                </EmptyBlock>
              ) : visible.length === 0 ? (
                <EmptyBlock headline={`No fill matches the ${side} filter.`}>
                  <p>
                    {formatCount(data.fills.length)} fill(s) are recorded; none is a {side}. Clear
                    the filter to see all of them.
                  </p>
                </EmptyBlock>
              ) : (
                <>
                  {side !== "all" ? (
                    <p className="border-b border-[color:var(--color-line)] px-3 py-1 text-[11px] text-[color:var(--color-ink-faint)]">
                      Showing {formatCount(visible.length)} of {formatCount(total)} recorded fill(s)
                      on the {side} side.
                    </p>
                  ) : null}
                  <TableWell maxHeight="46vh" label="Fills">
                    <table className="dt">
                      <thead>
                        <tr>
                          <th scope="col">Order</th>
                          <th scope="col">Instrument</th>
                          <th scope="col">Side</th>
                          <th scope="col" className="n">
                            Quantity
                          </th>
                          <th scope="col" className="n">
                            Price
                          </th>
                          <th scope="col">Venue</th>
                          <th scope="col">Execution</th>
                        </tr>
                      </thead>
                      <tbody>
                        {visible.map((fill, index) => (
                          <tr
                            key={`${fill.order}-${index}`}
                            data-alert={fill.simulated ? undefined : "true"}
                          >
                            <td className="num">{fill.order}</td>
                            <td className="num">{fill.instrument}</td>
                            <td>
                              <span
                                className="num text-[11px]"
                                data-direction={
                                  fill.side.toLowerCase() === "buy" ? "positive" : "negative"
                                }
                              >
                                {fill.side.toUpperCase()}
                              </span>
                            </td>
                            <td className="n">{formatDecimal(fill.quantity)}</td>
                            <td className="n">{formatDecimal(fill.price)}</td>
                            <td className="num">{fill.venue}</td>
                            <td>
                              <Chip tone={fill.simulated ? "ok" : "bad"}>
                                {fill.simulated ? "simulated" : "LIVE VENUE"}
                              </Chip>
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </TableWell>
                </>
              )
            }
          </ResourceView>
        </PanelBody>
      </Panel>
    </div>
  );
}
