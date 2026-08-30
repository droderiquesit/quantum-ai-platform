"use client";

import Link from "next/link";
import { useMemo, useState } from "react";
import { Chip, Freshness, Metric, MetricRow, StreamControls } from "@/components/data/Bits";
import { EventFeed } from "@/components/data/EventFeed";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import type { Fills, Orders } from "@/lib/api/types";
import { formatCount, formatDecimal } from "@/lib/format";
import { useEventStream } from "@/lib/hooks/useEventStream";
import { useResource } from "@/lib/hooks/useResource";

type StateFilter = "all" | "open" | "terminal";

const TERMINAL = new Set(["filled", "cancelled", "canceled", "rejected", "expired", "refused"]);

/**
 * The blotter: every order this process holds, every fill behind it, and the
 * lifecycle transitions as they happen.
 *
 * The simulated column is on every row rather than in a legend. A blotter where
 * the reader has to work out which fills were real is a blotter that will be
 * misread, and this deployment is paper-only precisely so that column can be
 * checked at a glance.
 */
export default function OrderBlotter() {
  const orders = useResource<Orders>(platform.orders, {
    key: "orders",
    label: "GET /orders",
    intervalMs: 5_000,
  });
  const fills = useResource<Fills>(platform.fills, {
    key: "fills",
    label: "GET /fills",
    intervalMs: 5_000,
  });
  const stream = useEventStream({ channel: "orders", label: "SSE /stream/orders", maxEvents: 300 });

  const [stateFilter, setStateFilter] = useState<StateFilter>("all");
  const [query, setQuery] = useState("");

  const visibleOrders = useMemo(() => {
    const rows = orders.data?.orders ?? [];
    const needle = query.trim().toLowerCase();
    return rows
      .filter((order) => {
        if (stateFilter === "all") return true;
        const terminal = TERMINAL.has(order.state.toLowerCase());
        return stateFilter === "terminal" ? terminal : !terminal;
      })
      .filter(
        (order) =>
          needle === "" ||
          order.id.toLowerCase().includes(needle) ||
          order.instrument.toLowerCase().includes(needle),
      );
  }, [orders.data, stateFilter, query]);

  const liveFills = fills.data?.any_live_fill ?? null;

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Blotter summary"
          meta={<Freshness resource={orders} name="orders" />}
          actions={
            <Link className="btn" data-variant="primary" href="/order-entry">
              New paper order
            </Link>
          }
        />
        <PanelBody>
          <MetricRow>
            <Metric label="Orders" value={formatCount(orders.data?.orders.length)} hint="all states" />
            <Metric
              label="Refusals"
              value={formatCount(orders.data?.refusals)}
              tone={(orders.data?.refusals ?? 0) > 0 ? "warn" : undefined}
              hint="rejected before reaching a venue"
            />
            <Metric label="Fills" value={formatCount(fills.data?.fills.length)} hint="recorded" />
            <Metric
              label="Reconciliation breaks"
              value={formatCount(orders.data?.reconciliation_breaks.length)}
              tone={(orders.data?.reconciliation_breaks.length ?? 0) > 0 ? "bad" : "ok"}
              hint="book against venue"
            />
            <Metric
              label="Any live fill"
              value={liveFills === null ? "—" : liveFills ? "YES" : "no"}
              tone={liveFills ? "bad" : "ok"}
              hint="whether any fill came from a real venue"
            />
          </MetricRow>
          {orders.data && orders.data.reconciliation_breaks.length > 0 ? (
            <ul className="mt-2 flex flex-col gap-1" role="alert">
              {orders.data.reconciliation_breaks.map((reason, index) => (
                <li key={index} className="text-[11.5px] text-[color:var(--color-down)]">
                  {reason}
                </li>
              ))}
            </ul>
          ) : null}
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Order lifecycle"
          meta={<StreamControls stream={stream} name="orders" />}
          actions={
            <button
              type="button"
              className="btn"
              data-variant="ghost"
              onClick={() => {
                orders.refresh();
                fills.refresh();
              }}
            >
              Reconcile over REST
            </button>
          }
        />
        <PanelBody flush>
          <EventFeed stream={stream} channel="orders" maxHeight="30vh" />
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Orders"
          meta={<Freshness resource={orders} name="orders" />}
          actions={
            <>
              <div className="seg" role="group" aria-label="Filter orders by state">
                {(["all", "open", "terminal"] as const).map((option) => (
                  <button
                    key={option}
                    type="button"
                    aria-pressed={stateFilter === option}
                    onClick={() => setStateFilter(option)}
                  >
                    {option}
                  </button>
                ))}
              </div>
              <label className="sr-only" htmlFor="order-search">
                Search orders by id or instrument
              </label>
              <input
                id="order-search"
                className="input h-[24px] w-[180px]"
                placeholder="id or instrument"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
              />
            </>
          }
        />
        <PanelBody flush>
          <ResourceView resource={orders} loadingRows={6}>
            {(data) =>
              data.orders.length === 0 ? (
                <EmptyBlock headline="This process holds no orders.">
                  <p>
                    Observed, not assumed: <code className="num">GET /api/v1/orders</code> returned
                    an empty list.
                  </p>
                </EmptyBlock>
              ) : visibleOrders.length === 0 ? (
                <EmptyBlock headline={`No order matches the current filter.`}>
                  <p>
                    {data.orders.length} order(s) are held; none is {stateFilter}
                    {query.trim() === "" ? "" : ` and matching “${query}”`}.
                  </p>
                </EmptyBlock>
              ) : (
                <TableWell maxHeight="40vh" label="Orders">
                  <table className="dt">
                    <thead>
                      <tr>
                        <th scope="col">Order id</th>
                        <th scope="col">Instrument</th>
                        <th scope="col">Side</th>
                        <th scope="col" className="n">
                          Quantity
                        </th>
                        <th scope="col" className="n">
                          Filled
                        </th>
                        <th scope="col">State</th>
                        <th scope="col">Execution</th>
                      </tr>
                    </thead>
                    <tbody>
                      {visibleOrders.map((order) => (
                        <tr key={order.id} data-alert={order.simulated ? undefined : "true"}>
                          <td className="num">{order.id}</td>
                          <td className="num">{order.instrument}</td>
                          <td>
                            <span
                              className="num text-[11px]"
                              data-direction={
                                order.side.toLowerCase() === "buy" ? "positive" : "negative"
                              }
                            >
                              {order.side.toUpperCase()}
                            </span>
                          </td>
                          <td className="n">{formatDecimal(order.quantity)}</td>
                          <td className="n">{formatDecimal(order.filled)}</td>
                          <td>
                            <Chip tone={TERMINAL.has(order.state.toLowerCase()) ? "neutral" : "info"}>
                              {order.state}
                            </Chip>
                          </td>
                          <td>
                            <Chip tone={order.simulated ? "ok" : "bad"}>
                              {order.simulated ? "paper" : "LIVE"}
                            </Chip>
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

      <Panel>
        <PanelHead title="Fill history" meta={<Freshness resource={fills} name="fills" />} />
        <PanelBody flush>
          <ResourceView resource={fills} loadingRows={6}>
            {(data) =>
              data.fills.length === 0 ? (
                <EmptyBlock headline="No fill has been recorded.">
                  <p>
                    Observed, not assumed: <code className="num">GET /api/v1/fills</code> returned an
                    empty list.
                  </p>
                </EmptyBlock>
              ) : (
                <TableWell maxHeight="40vh" label="Fill history">
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
                      {data.fills.map((fill, index) => (
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
              )
            }
          </ResourceView>
        </PanelBody>
      </Panel>
    </div>
  );
}
