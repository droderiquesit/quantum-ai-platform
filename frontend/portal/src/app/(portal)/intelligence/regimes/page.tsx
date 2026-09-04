"use client";

import { useMemo } from "react";
import { Chip, Metric, MetricRow, StreamControls } from "@/components/data/Bits";
import { EventFeed } from "@/components/data/EventFeed";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { StateBlock } from "@/components/data/States";
import { formatClock, formatCount } from "@/lib/format";
import { useEventStream } from "@/lib/hooks/useEventStream";
import { summarisePayload, type StreamEnvelope } from "@/lib/sse/envelope";

/**
 * Regime detection, read from the platform's own signal stream.
 *
 * `GET /api/v1/stream/signals` carries `regime.changed`, and the regime the
 * platform currently believes is, by definition, the payload of the newest
 * such event. This page is a client of that stream and nothing else. It used
 * to render a seeded "current regime", a confidence gauge and a transition
 * matrix under a SIMULATED DATA banner; the illustration is gone because the
 * route that carries the fact it illustrated already exists, and the two
 * figures it had no source for — classification confidence and the
 * transition matrix — are named below as the gap they are rather than
 * generated.
 *
 * With no `regime.changed` recorded, the page says so. It does not pick a
 * regime to show: an invented "range-bound" on a page an operator reads to
 * learn the market state is a fabrication however plausible the choice.
 */

const REGIME_TOPIC = "regime.changed";

function isRegimeChange(envelope: StreamEnvelope): boolean {
  return envelope.type === REGIME_TOPIC;
}

function asText(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}

/** The regime a `regime.changed` payload names, under whichever key it uses. */
function regimeOf(envelope: StreamEnvelope): string {
  const payload = envelope.payload;
  return (
    asText(payload["to"]) ??
    asText(payload["regime"]) ??
    asText(payload["new_regime"]) ??
    asText(payload["state"]) ??
    summarisePayload(envelope)
  );
}

export default function RegimesPage() {
  const signals = useEventStream({
    channel: "signals",
    label: "SSE /stream/signals (regimes)",
    maxEvents: 200,
  });

  const changes = useMemo(() => signals.events.filter(isRegimeChange), [signals.events]);
  const feed = useMemo(() => ({ ...signals, events: changes }), [signals, changes]);
  const current = changes[0] ?? null;
  const otherOnStream = signals.events.length - changes.length;

  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[2fr_3fr]">
        <Panel>
          <PanelHead
            title="Current regime"
            meta={<StreamControls stream={signals} name="signals" />}
            actions={<Chip>SSE /api/v1/stream/signals</Chip>}
          />
          <PanelBody>
            {current ? (
              <div className="flex flex-col gap-3" data-testid="current-regime">
                <p className="num text-[19px] font-semibold text-[color:var(--color-ink)]">
                  {regimeOf(current)}
                </p>
                <MetricRow>
                  <Metric
                    label="Changed at"
                    value={formatClock(current.receivedAt)}
                    hint={current.eventTime ? `event_time ${current.eventTime}` : "as received"}
                  />
                  <Metric label="Cursor" value={current.cursor ?? "—"} hint="log position" />
                  <Metric
                    label="Changes seen"
                    value={formatCount(changes.length)}
                    hint="regime.changed since connect"
                  />
                </MetricRow>
                <p className="text-[11px] text-[color:var(--color-ink-faint)]">
                  The newest <code className="num">regime.changed</code> on the stream, shown as the
                  platform sent it. Nothing here is classified in the browser.
                </p>
              </div>
            ) : (
              <StateBlock
                tone="neutral"
                label="no regime change recorded"
                headline="The platform has recorded no regime change on this stream."
                compact
              >
                <p>
                  The current regime is the newest <code className="num">regime.changed</code> event,
                  and none has arrived
                  {signals.events.length > 0
                    ? ` — although ${formatCount(signals.events.length)} other signal event(s) have, so the feed itself is live`
                    : ""}
                  . This page does not choose a regime to display in its place.
                </p>
              </StateBlock>
            )}
          </PanelBody>
        </Panel>

        <Panel>
          <PanelHead title="Regime changes" />
          <PanelBody flush>
            {signals.events.length > 0 && changes.length === 0 ? (
              <p
                className="border-b border-[color:var(--color-line)] px-3 py-1 text-[11px] text-[color:var(--color-ink-faint)]"
                data-testid="regime-filter-premise"
              >
                {formatCount(otherOnStream)} signal event(s) on the stream are not regime changes and
                are not shown here.
              </p>
            ) : null}
            <EventFeed stream={feed} channel="signals" maxHeight="44vh" />
          </PanelBody>
        </Panel>
      </div>

      <Panel>
        <PanelHead title="What the platform does not serve" actions={<Chip tone="warn">gap</Chip>} />
        <PanelBody>
          <StateBlock
            tone="warn"
            label="not served"
            headline="Classification confidence and the transition matrix have no platform surface."
            compact
          >
            <p>
              The world model in <code className="num">backend/crates/services/qip-world-model</code>{" "}
              tracks the believed state of the world in-process, and{" "}
              <code className="num">/stream/signals</code> declares <code className="num">regime.changed</code>{" "}
              as a topic it carries. No component in this deployment publishes that topic yet, and
              nothing exposes a classification with its confidence or a transition matrix. Those
              would need a <code className="num">GET /api/v1/regimes</code> answering the current
              regime, its confidence and the row of transition probabilities from it. Until then this
              page shows the changes the stream delivers and no figure it cannot source.
            </p>
          </StateBlock>
        </PanelBody>
      </Panel>
    </div>
  );
}
