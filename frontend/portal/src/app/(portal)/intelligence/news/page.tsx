"use client";

import { Chip } from "@/components/data/Bits";
import { Panel, PanelBody, PanelHead } from "@/components/data/Panel";
import { SimulatedBanner } from "@/components/data/Simulated";
import { StateBlock } from "@/components/data/States";
import { Bars } from "@/components/viz/primitives";
import { simBetween } from "@/lib/sim";

/**
 * News and sentiment: what is true today on top, an illustration below.
 *
 * The two halves are separated on purpose and must stay that way. The top
 * panel is the platform's actual position — no live news source is ingested
 * and no news surface is served — stated without simulation, because "we have
 * no news" is a fact worth the whole panel. The bottom half illustrates what
 * the surface would carry, under the banner, with invented sectors and
 * fictional instruments only: a fabricated headline about a real company is a
 * fabrication however clearly the page is labelled, so no real entity may
 * appear here.
 */

interface SimulatedHeadline {
  /** A fictional instrument from the console's invented universe. */
  readonly instrument: string;
  /** An invented sector — not an industry classification anyone publishes. */
  readonly sector: string;
  readonly headline: string;
  /** Sentiment in [-1, 1], from the seeded generator. */
  readonly sentiment: number;
}

const HEADLINES: readonly SimulatedHeadline[] = [
  {
    instrument: "EQ-AURORA",
    sector: "orbital logistics",
    headline:
      "Aurora-basket assemblers guide deliveries higher after a debottlenecked launch window",
  },
  {
    instrument: "EQ-BOREAL",
    sector: "synthetic agriculture",
    headline: "Boreal growers report a weaker glasshouse harvest as input costs stay elevated",
  },
  {
    instrument: "FX-KESTREL",
    sector: "monetary policy",
    headline: "The Kestrel-bloc central bank holds its corridor and hints at a slower unwind",
  },
  {
    instrument: "FX-MERIDIAN",
    sector: "trade flows",
    headline: "Meridian-corridor freight volumes contract for a second consecutive quarter",
  },
  {
    instrument: "CR-ORRERY",
    sector: "structured credit",
    headline: "Orrery-index issuers refinance early, tightening the on-the-run spread",
  },
  {
    instrument: "CM-THALASSA",
    sector: "deep-sea energy",
    headline: "Thalassa-strip supply disruption resolved sooner than the forward curve priced",
  },
].map((entry) => ({
  ...entry,
  sentiment: Math.round(simBetween(`news:sentiment:${entry.instrument}`, -1, 1) * 100) / 100,
}));

function sentimentTone(value: number): "up" | "down" | "flat" {
  if (value > 0.15) return "up";
  if (value < -0.15) return "down";
  return "flat";
}

export default function NewsPage() {
  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead title="What the platform ingests today" actions={<Chip tone="warn">no live source</Chip>} />
        <PanelBody>
          <StateBlock
            tone="warn"
            label="not served"
            headline="This deployment ingests no live news source, and serves no news surface."
          >
            <p>
              The absorption machinery is real: the narrative adapter in{" "}
              <code className="num">backend/crates/services/qip-market-ingestion/src/narrative.rs</code>{" "}
              decodes news items, corporate filings and macroeconomic releases into sensed
              records, anchored on the instant each document became knowable. But it runs
              in-process, no vendor feed is configured here, and nothing exposes what was
              absorbed over HTTP — the contract this page is written against,{" "}
              <code className="num">GET /api/v1/news</code>, does not exist yet.
            </p>
            <p className="mt-1.5 text-[color:var(--color-ink-faint)]">
              So the emptiness above is a stated absence, not a quiet news day. The cards below
              are a labelled illustration of what the surface would carry, and nothing more.
            </p>
          </StateBlock>
        </PanelBody>
      </Panel>

      <SimulatedBanner subject="news and sentiment" contract="GET /api/v1/news">
        <p className="max-w-[80ch] text-[11.5px] leading-relaxed text-[color:var(--color-ink-dim)]">
          Every headline below is invented, about a fictional instrument in an invented sector.
          No real company, publication or event appears here, so no card can be read as a claim
          about the world.
        </p>
      </SimulatedBanner>

      <div className="grid grid-cols-1 gap-3 lg:grid-cols-2 xl:grid-cols-3">
        {HEADLINES.map((item) => {
          const tone = sentimentTone(item.sentiment);
          return (
            <Panel key={item.instrument}>
              <PanelHead
                title={item.instrument}
                meta={<Chip tone="info">{item.sector}</Chip>}
                actions={
                  <Chip tone={tone === "up" ? "ok" : tone === "down" ? "bad" : "neutral"}>
                    {tone === "up" ? "reads positive" : tone === "down" ? "reads negative" : "reads neutral"}
                  </Chip>
                }
              />
              <PanelBody>
                <div className="flex flex-col gap-3">
                  <p className="text-[12.5px] leading-relaxed text-[color:var(--color-ink)]">
                    {item.headline}
                  </p>
                  <Bars
                    items={[{ label: "sentiment", value: item.sentiment, tone }]}
                  />
                  <p className="text-[10px] text-[color:var(--color-ink-faint)]">
                    Sentiment in [-1, 1], generated from a fixed seed. The headline is invented.
                  </p>
                </div>
              </PanelBody>
            </Panel>
          );
        })}
      </div>
    </div>
  );
}
