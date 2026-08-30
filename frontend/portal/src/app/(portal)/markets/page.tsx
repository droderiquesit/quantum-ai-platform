"use client";

import { useMemo, useState } from "react";
import { Chip, Freshness, Metric, MetricRow, StreamControls } from "@/components/data/Bits";
import { EventFeed } from "@/components/data/EventFeed";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { ResourceView } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import { formatCount, formatDurationMs } from "@/lib/format";
import { useEventStream } from "@/lib/hooks/useEventStream";
import { useResource } from "@/lib/hooks/useResource";

/**
 * The live surface: normalised market data and the signals derived from it,
 * with the reference state the REST surface can still be asked for.
 */
export default function GlobalMarkets() {
  const market = useEventStream({ channel: "market", label: "SSE /stream/market", maxEvents: 300 });
  const signals = useEventStream({ channel: "signals", label: "SSE /stream/signals", maxEvents: 200 });

  const [filter, setFilter] = useState("");

  const markets = useResource<unknown>(platform.markets, {
    key: "markets",
    label: "GET /markets",
    intervalMs: 30_000,
  });
  const assets = useResource<unknown>(platform.assets, {
    key: "assets",
    label: "GET /assets",
    intervalMs: 60_000,
  });

  const filtered = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    if (needle === "") return market.events;
    return market.events.filter(
      (event) =>
        event.type.toLowerCase().includes(needle) ||
        JSON.stringify(event.payload).toLowerCase().includes(needle),
    );
  }, [market.events, filter]);

  const filteredStream = useMemo(() => ({ ...market, events: filtered }), [market, filtered]);

  const medianIngest = useMemo(() => median(market.events.map((e) => e.ingestLagMs)), [market.events]);
  const medianTransit = useMemo(
    () => median(market.events.map((e) => e.transitLagMs)),
    [market.events],
  );

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead title="Feed health" />
        <PanelBody>
          <MetricRow>
            <Metric
              label="Market events"
              value={formatCount(market.received)}
              hint={market.dropped > 0 ? `${market.dropped} dropped from view` : "since connect"}
            />
            <Metric
              label="Signal events"
              value={formatCount(signals.received)}
              hint="since connect"
            />
            <Metric
              label="Median ingest lag"
              value={formatDurationMs(medianIngest)}
              hint="ingest_time − event_time"
            />
            <Metric
              label="Median transit"
              value={formatDurationMs(medianTransit)}
              hint="arrival − ingest_time"
            />
            <Metric
              label="Resume cursor"
              value={market.cursor ?? "—"}
              hint="what a reconnect asks for"
            />
            <Metric
              label="Sequence gaps"
              value={formatCount(market.gaps.length)}
              tone={market.gaps.length > 0 ? "bad" : "ok"}
              hint="contiguity of this connection"
            />
          </MetricRow>
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="Market feed"
          meta={<StreamControls stream={market} name="market" />}
          actions={
            <>
              <label className="sr-only" htmlFor="market-filter">
                Filter market events
              </label>
              <input
                id="market-filter"
                className="input h-[24px] w-[200px]"
                placeholder="filter by type or payload"
                value={filter}
                onChange={(event) => setFilter(event.target.value)}
              />
              <button
                type="button"
                className="btn"
                data-variant="ghost"
                onClick={market.clear}
                disabled={market.events.length === 0}
              >
                Clear
              </button>
            </>
          }
        />
        <PanelBody flush>
          {filter.trim() !== "" && market.events.length > 0 ? (
            <p className="border-b border-[color:var(--color-line)] px-3 py-1 text-[11px] text-[color:var(--color-ink-faint)]">
              Showing {filtered.length} of {market.events.length} retained events matching “{filter}”.
            </p>
          ) : null}
          <EventFeed stream={filteredStream} channel="market" maxHeight="46vh" />
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="Signals, anomalies and regime changes" meta={<StreamControls stream={signals} name="signals" />} />
        <PanelBody flush>
          <EventFeed stream={signals} channel="signals" maxHeight="32vh" />
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

function median(values: readonly (number | null)[]): number | null {
  const present = values.filter((value): value is number => value !== null).sort((a, b) => a - b);
  if (present.length === 0) return null;
  const middle = Math.floor(present.length / 2);
  if (present.length % 2 === 1) return present[middle] ?? null;
  const low = present[middle - 1];
  const high = present[middle];
  return low === undefined || high === undefined ? null : (low + high) / 2;
}

/**
 * The platform answered with a shape this console does not model yet. Showing
 * it verbatim is the honest option: inventing a table for it is not.
 */
function RawJson({ value }: { value: unknown }) {
  return (
    <pre className="num max-h-[220px] overflow-auto whitespace-pre-wrap break-all border border-[color:var(--color-line)] bg-[color:var(--color-sunken)] p-2 text-[11px] text-[color:var(--color-ink-dim)]">
      {JSON.stringify(value, null, 2)}
    </pre>
  );
}
