"use client";

import { useMemo } from "react";
import { Chip, Freshness, Metric, MetricRow, StreamControls } from "@/components/data/Bits";
import { EventFeed } from "@/components/data/EventFeed";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { ResourceView, StateBlock } from "@/components/data/States";
import { platform } from "@/lib/api/client";
import { formatCount, formatTimestamp } from "@/lib/format";
import { useEventStream } from "@/lib/hooks/useEventStream";
import { useResource } from "@/lib/hooks/useResource";
import type { StreamEnvelope } from "@/lib/sse/envelope";

/**
 * News and sentiment, read from the platform's own market stream and from
 * `GET /news`.
 *
 * `GET /api/v1/stream/market` carries every narrative topic the ingestion
 * service records — `news.received`, `fundamental.updated`, `macro.updated` —
 * and the feed on this page is a client of that stream and nothing else. It
 * used to render six invented headlines under a SIMULATED DATA banner; that
 * illustration is gone, because a real route existed for the thing it
 * illustrated, and a placeholder beside a real feed is exactly the mixed
 * panel the console's rules forbid.
 *
 * What is honest about an empty feed: this deployment configures no vendor
 * narrative adapter, so the stream is open and carries nothing on these
 * topics. The page says that in the feed's own "connected, no events" state
 * rather than dressing it up — "no news has been ingested" is a fact, and a
 * quiet feed and a dead socket are kept distinguishable by the stream
 * controls beside it.
 *
 * The second panel reads `GET /news`, which `routes.rs` serves and answers
 * with an absence and its reason. This page used to tell its reader that
 * route did not exist — a false statement on the screen, made in the
 * platform's disfavour — and paraphrased the reason from memory. Now the
 * platform's own sentence is rendered, and if the route ever answers a body
 * instead, that body is shown verbatim and labelled unread rather than
 * through a shape this console guessed.
 */

/** The wire names of the topics the narrative adapter publishes. */
const NARRATIVE_TOPICS = new Set(["news.received", "fundamental.updated", "macro.updated"]);

function isNarrative(envelope: StreamEnvelope): boolean {
  return NARRATIVE_TOPICS.has(envelope.type);
}

function asNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function asText(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}

/** A `NewsItem` payload, rendered with its own fields where they are present. */
function NewsRow({ envelope }: { envelope: StreamEnvelope }) {
  const payload = envelope.payload;
  const headline = asText(payload["headline"]);
  const source = asText(payload["source"]);
  const sentiment =
    typeof payload["sentiment"] === "object" && payload["sentiment"] !== null
      ? (payload["sentiment"] as Record<string, unknown>)
      : null;
  const polarity = sentiment ? asNumber(sentiment["polarity"]) : null;
  const confidence = sentiment ? asNumber(sentiment["confidence"]) : null;
  const publishedAt = asText(payload["published_at"]);

  if (headline === null) {
    // Not a news item — a fundamental or macro update, or a shape this
    // console has not modelled. The type chip in the row already names it;
    // the payload is shown as the platform sent it rather than guessed at.
    return <span>{JSON.stringify(payload).slice(0, 200)}</span>;
  }
  return (
    <span className="flex flex-wrap items-center gap-2" data-testid="news-row">
      <span className="text-[color:var(--color-ink)]">{headline}</span>
      {source ? <Chip tone="info">{source}</Chip> : null}
      {polarity !== null ? (
        <Chip tone={polarity > 0.15 ? "ok" : polarity < -0.15 ? "bad" : "neutral"}>
          polarity {polarity.toFixed(2)}
          {confidence !== null ? ` · conf ${confidence.toFixed(2)}` : ""}
        </Chip>
      ) : null}
      {publishedAt ? (
        <span className="num text-[10px] text-[color:var(--color-ink-faint)]">
          published {formatTimestamp(publishedAt)}
        </span>
      ) : null}
    </span>
  );
}

export default function NewsPage() {
  const market = useEventStream({
    channel: "market",
    label: "SSE /stream/market (narrative)",
    maxEvents: 300,
  });
  const news = useResource<unknown>(platform.news, {
    key: "news",
    label: "GET /news",
    intervalMs: 60_000,
  });

  const narrative = useMemo(() => market.events.filter(isNarrative), [market.events]);
  const feed = useMemo(() => ({ ...market, events: narrative }), [market, narrative]);
  const newsCount = useMemo(
    () => narrative.filter((envelope) => envelope.type === "news.received").length,
    [narrative],
  );
  const otherOnStream = market.events.length - narrative.length;

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Narrative feed"
          meta={<StreamControls stream={market} name="market" />}
          actions={<Chip>SSE /api/v1/stream/market</Chip>}
        />
        <PanelBody>
          <MetricRow>
            <Metric
              label="News items"
              value={formatCount(newsCount)}
              hint="news.received, since connect"
            />
            <Metric
              label="Narrative events"
              value={formatCount(narrative.length)}
              hint="news, fundamentals and macro releases"
            />
            <Metric
              label="Other market events"
              value={formatCount(otherOnStream)}
              hint="on the same stream, not shown here"
            />
            <Metric
              label="Resume cursor"
              value={market.cursor ?? "—"}
              hint="what a reconnect asks for"
            />
          </MetricRow>
        </PanelBody>
        <PanelBody flush>
          {/* The premise beside the conclusion: a filtered-empty feed on a
              stream that carried other events must never read as a stream that
              carried nothing. */}
          {market.events.length > 0 && narrative.length === 0 ? (
            <p
              className="border-b border-[color:var(--color-line)] px-3 py-1 text-[11px] text-[color:var(--color-ink-faint)]"
              data-testid="news-filter-premise"
            >
              The stream has delivered {formatCount(market.events.length)} event(s) since connect,
              none of them narrative. This is a measured absence of news, not a silent feed.
            </p>
          ) : null}
          <EventFeed
            stream={feed}
            channel="market"
            maxHeight="52vh"
            renderRow={(envelope) => <NewsRow envelope={envelope} />}
          />
        </PanelBody>
      </Panel>

      <Panel>
        <PanelHead
          title="What is behind this feed"
          meta={<Freshness resource={news} name="news" />}
          actions={<Chip>GET /api/v1/news</Chip>}
        />
        <PanelBody>
          {news.outcome !== null && news.outcome.kind === "unavailable" ? (
            <StateBlock
              tone="warn"
              label="not ingested"
              headline="The platform serves no news in this deployment, and says why."
              compact
            >
              <p className="num" data-testid="news-route-reason">
                {news.outcome.reason}
              </p>
              <p className="mt-1.5">
                The platform&rsquo;s own answer, under the subject{" "}
                <span className="num">{news.outcome.subject}</span>, and not this console&rsquo;s
                paraphrase of it. The absorption machinery is real: the narrative adapter in{" "}
                <code className="num">backend/crates/services/qip-market-ingestion/src/narrative.rs</code>{" "}
                decodes news items, corporate filings and macroeconomic releases into sensed
                records, anchored on the instant each document became knowable, and publishes them
                on the topics the feed above subscribes to. When an adapter is configured, the rows
                render there with the item&rsquo;s own headline, source and scored sentiment.
              </p>
              <p className="mt-1.5 text-[color:var(--color-ink-faint)]" data-testid="news-route-history">
                What is still missing on the platform side is a history: the stream opens at the
                live edge with a bounded backlog, and{" "}
                <code className="num">GET /api/v1/news</code> is served but matched to one expression
                that answers this absence, with no arm that pages through what was ingested before
                this tab opened.
              </p>
            </StateBlock>
          ) : (
            <ResourceView resource={news} loadingRows={2}>
              {(body) => (
                <StateBlock
                  tone="info"
                  label="served, not yet read"
                  headline="The platform answered GET /news with a body, and this console has not been taught its shape."
                  compact
                >
                  <p>
                    Shown as it arrived, so what the platform served is on the screen rather than
                    hidden behind a shape this page would otherwise have guessed.
                  </p>
                  <pre className="num mt-1.5 max-h-[40vh] overflow-auto whitespace-pre-wrap text-[10.5px]" data-testid="news-route-body">
                    {JSON.stringify(body, null, 2)}
                  </pre>
                </StateBlock>
              )}
            </ResourceView>
          )}
        </PanelBody>
      </Panel>
    </div>
  );
}
