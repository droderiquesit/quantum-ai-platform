"use client";

import { Chip, Freshness, KeyValue, StreamControls } from "@/components/data/Bits";
import { EventFeed } from "@/components/data/EventFeed";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { ResourceView } from "@/components/data/States";
import { AreaChart } from "@/components/viz/primitives";
import { platform } from "@/lib/api/client";
import type { SystemMetrics } from "@/lib/api/types";
import { formatClock, formatCount, formatDurationMs } from "@/lib/format";
import { useEventStream } from "@/lib/hooks/useEventStream";
import { useResource } from "@/lib/hooks/useResource";
import { describeWindow, useSeries } from "@/lib/hooks/useSeries";

/**
 * The platform's own counters, watched over time by this console.
 *
 * The platform serves counters, not curves: `/system/metrics` says how many
 * cycles have run, never how the number moved. Every sparkline and chart here
 * is therefore a series this browser accumulated by polling, and each one is
 * captioned with its own window — a two-minute trace presented as a trend
 * would be a lie about coverage, and on a telemetry page that lie is the one
 * that matters most.
 *
 * The page also accounts for its own instrument: the poll's latency, age, and
 * failure count are shown beside the numbers they fetched, because a counter
 * read through a struggling poll is only as good as that poll.
 */
export default function TelemetryPage() {
  const metrics = useResource<SystemMetrics>(platform.systemMetrics, {
    key: "telemetry-metrics",
    label: "GET /system/metrics",
    intervalMs: 10_000,
  });
  const health = useEventStream({ channel: "health", label: "health stream" });

  const data = metrics.data;
  // One series per counter, appended only when the value changes, so a flat
  // poll does not stretch the window with duplicate points.
  const cycles = useSeries(data?.cycles ?? null);
  const events = useSeries(data?.events_logged ?? null);
  const queued = useSeries(data?.opportunities_queued ?? null);
  const proposals = useSeries(data?.proposals ?? null);
  const orders = useSeries(data?.orders ?? null);
  const fills = useSeries(data?.fills ?? null);
  const refusals = useSeries(data?.refusals ?? null);

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Process counters"
          meta={<Freshness resource={metrics} name="metrics" />}
          actions={<Chip tone="warn">NOT SCRAPED</Chip>}
        />
        <PanelBody>
          <ResourceView resource={metrics} loadingRows={2}>
            {(m) => (
              <KpiRow>
                <Kpi
                  label="Cycles"
                  value={formatCount(m.cycles)}
                  series={cycles}
                  trend="accent"
                  note="loop iterations since this process started"
                />
                <Kpi
                  label="Events logged"
                  value={formatCount(m.events_logged)}
                  series={events}
                  trend="accent"
                  note="appended to the hash-chained log"
                />
                <Kpi
                  label="Opportunities queued"
                  value={formatCount(m.opportunities_queued)}
                  series={queued}
                  note="found by DISCOVER, awaiting REASON"
                />
                <Kpi
                  label="Proposals"
                  value={formatCount(m.proposals)}
                  series={proposals}
                  note="cleared the action bar in DECIDE"
                />
                <Kpi
                  label="Orders"
                  value={formatCount(m.orders)}
                  series={orders}
                  note="released by ACT against the simulator"
                />
                <Kpi
                  label="Fills"
                  value={formatCount(m.fills)}
                  series={fills}
                  tone={m.live_fills ? "bad" : "neutral"}
                  note={m.live_fills ? "A LIVE FILL IS PRESENT" : "every fill simulated"}
                />
                <Kpi
                  label="Refusals"
                  value={formatCount(m.refusals)}
                  series={refusals}
                  note="orders the risk engine declined to release"
                />
              </KpiRow>
            )}
          </ResourceView>
        </PanelBody>
      </Panel>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[3fr_1fr]">
        <Panel>
          <PanelHead
            title="Events logged, as observed here"
            meta={<Freshness resource={metrics} name="the events counter" />}
          />
          <PanelBody>
            <AreaChart
              values={events.values}
              height={160}
              label="events logged"
              caption={describeWindow(events)}
            />
            <p className="mt-2 text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
              This curve begins when this tab was opened, not when the platform started. It is
              lost on reload and covers only what this browser watched; the caption states the
              window for that reason.
            </p>
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead
            title="This page's own poll"
            meta={<Freshness resource={metrics} name="this poll" />}
          />
          <PanelBody>
            <dl className="flex flex-col">
              <KeyValue label="Last answer took">
                {formatDurationMs(metrics.latencyMs)}
              </KeyValue>
              <KeyValue label="Last answer landed at">
                {metrics.receivedAt === null ? "—" : `${formatClock(metrics.receivedAt)} UTC`}
              </KeyValue>
              <KeyValue label="Consecutive failed attempts">
                {formatCount(metrics.attempts)}
              </KeyValue>
              <KeyValue label="Poll period">10s</KeyValue>
            </dl>
            <p className="mt-2 text-[11px] leading-relaxed text-[color:var(--color-ink-faint)]">
              Every counter above was read through this poll, so its health bounds theirs. A
              figure fetched by a failing poll is a stale figure wearing a fresh face.
            </p>
          </PanelBody>
        </Panel>
      </div>

      <Panel>
        <PanelHead
          title="Health stream"
          meta={<StreamControls stream={health} name="health" />}
        />
        <PanelBody flush>
          <EventFeed stream={health} channel="health" maxHeight="34vh" />
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead title="What this page is, and is not" />
        <PanelBody>
          <p
            data-testid="collection-gap"
            className="max-w-[90ch] text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]"
          >
            These are process counters read over HTTP from one running process, plus one event
            stream, and <strong>nothing collects them</strong>. The binaries do emit a Prometheus{" "}
            <span className="num">/metrics</span> surface; no collector is attached to it — the
            managed collector is not vendored or attested, and no execution node is deployed. Every
            alert policy in the deployment is gated behind{" "}
            <span className="num">workload_metrics_exist</span>, which is false in every committed
            environment, so none is stored and none is evaluated. A breach charted here pages no
            one. This page is a browser watching one process, and it is the whole of the
            observation.
          </p>
        </PanelBody>
      </Panel>
    </div>
  );
}
