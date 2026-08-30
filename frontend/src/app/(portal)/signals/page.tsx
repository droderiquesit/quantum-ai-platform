"use client";

import { Chip, Freshness, StreamControls } from "@/components/data/Bits";
import { EventFeed } from "@/components/data/EventFeed";
import { Kpi, KpiRow } from "@/components/data/Kpi";
import { Panel, PanelBody, PanelHead, TableWell } from "@/components/data/Panel";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { Bars } from "@/components/viz/primitives";
import { platform } from "@/lib/api/client";
import type { Opportunities, Proposals } from "@/lib/api/types";
import { formatCount, formatPercent } from "@/lib/format";
import { useEventStream } from "@/lib/hooks/useEventStream";
import { useResource } from "@/lib/hooks/useResource";

/**
 * What the platform found, what it proposed, and the signal feed underneath.
 *
 * The queue and the proposals are two halves of one story — an opportunity that
 * never became a proposal was reasoned about and rejected, and that is the more
 * interesting half. Both are polled; the stream beside them is the same events
 * arriving as they happen, so a queue that has stopped growing can be told
 * apart from a feed that has stopped delivering.
 */
export default function LiveSignals() {
  const opportunities = useResource<Opportunities>(platform.opportunities, {
    key: "signals-opportunities",
    label: "GET /opportunities",
    intervalMs: 8_000,
  });
  const proposals = useResource<Proposals>(platform.proposals, {
    key: "signals-proposals",
    label: "GET /proposals",
    intervalMs: 8_000,
  });
  const stream = useEventStream({ channel: "signals", label: "signals stream", maxEvents: 200 });

  const queue = opportunities.data?.opportunities ?? [];
  const book = proposals.data?.proposals ?? [];

  // Which detectors are actually firing. Counted from the queue itself rather
  // than from a detector registry, because the registry would list detectors
  // that have never produced anything and this panel is about what did.
  const byDetector = new Map<string, number>();
  for (const opportunity of queue) {
    for (const detector of opportunity.detectors) {
      byDetector.set(detector, (byDetector.get(detector) ?? 0) + 1);
    }
  }
  const detectors = [...byDetector.entries()]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([label, value]) => ({ label, value, tone: "accent" as const }));

  const scored = queue.filter((o) => Number.isFinite(o.score));
  const best = scored.reduce<number | null>(
    (top, o) => (top === null || o.score > top ? o.score : top),
    null,
  );
  const meanConfidence =
    queue.length === 0
      ? null
      : queue.reduce((sum, o) => sum + o.confidence, 0) / queue.length;

  return (
    <div className="flex flex-col gap-3 p-3">
      <Panel>
        <PanelHead
          title="Signal state"
          meta={<Freshness resource={opportunities} name="opportunities" />}
        />
        <PanelBody>
          <KpiRow>
            <Kpi
              label="In the queue"
              value={formatCount(queue.length)}
              note="found by DISCOVER, not yet decided"
            />
            <Kpi
              label="Proposals"
              value={formatCount(book.length)}
              note="cleared the action bar in DECIDE"
            />
            <Kpi
              label="Best score"
              value={best === null ? "—" : best.toFixed(3)}
              note={best === null ? "nothing scored" : "highest in the queue now"}
            />
            <Kpi
              label="Mean confidence"
              value={meanConfidence === null ? "—" : formatPercent(meanConfidence)}
              note={
                meanConfidence === null
                  ? "no opportunity to average"
                  : `across ${formatCount(queue.length)} opportunit${queue.length === 1 ? "y" : "ies"}`
              }
            />
            <Kpi
              label="Distinct detectors firing"
              value={formatCount(detectors.length)}
              note="counted from the queue, not from the registry"
            />
          </KpiRow>
        </PanelBody>
      </Panel>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[2fr_1fr]">
        <Panel>
          <PanelHead
            title="Opportunity queue"
            meta={<Freshness resource={opportunities} name="opportunities" />}
          />
          <PanelBody flush>
            <ResourceView resource={opportunities} loadingRows={6}>
              {(data) =>
                data.opportunities.length === 0 ? (
                  <EmptyBlock headline="Nothing is queued.">
                    <p>
                      <code className="num">GET /api/v1/opportunities</code> answered with an empty
                      list. That is a measured absence, not a failed read.
                    </p>
                  </EmptyBlock>
                ) : (
                  <TableWell maxHeight="420px" label="Opportunity queue">
                    <table className="dt">
                      <thead>
                        <tr>
                          <th scope="col">Id</th>
                          <th scope="col">Headline</th>
                          <th scope="col" className="n">
                            Score
                          </th>
                          <th scope="col" className="n">
                            Confidence
                          </th>
                          <th scope="col">Detectors</th>
                        </tr>
                      </thead>
                      <tbody>
                        {data.opportunities.map((opportunity) => (
                          <tr key={opportunity.id}>
                            <td className="num">{opportunity.id}</td>
                            <td className="whitespace-normal">{opportunity.headline}</td>
                            <td className="n">{opportunity.score.toFixed(3)}</td>
                            <td className="n">{formatPercent(opportunity.confidence)}</td>
                            <td className="num text-[10px] text-[color:var(--color-ink-dim)]">
                              {opportunity.detectors.join(", ") || "—"}
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
          <PanelHead title="Detectors firing" />
          <PanelBody>
            {detectors.length === 0 ? (
              <EmptyBlock headline="No detector has produced anything in the current queue.">
                <p>
                  This ranks detectors by what is in the queue now. An empty queue means no
                  detector fired, not that no detector is registered.
                </p>
              </EmptyBlock>
            ) : (
              <Bars items={detectors} />
            )}
          </PanelBody>
        </Panel>
      </div>

      <Panel>
        <PanelHead title="Proposals" meta={<Freshness resource={proposals} name="proposals" />} />
        <PanelBody flush>
          <ResourceView resource={proposals} loadingRows={5}>
            {(data) =>
              data.proposals.length === 0 ? (
                <EmptyBlock headline="Nothing has been proposed.">
                  <p>
                    Either nothing reached the action bar, or every thesis that did was rejected in
                    review. <code className="num">GET /api/v1/proposals</code> returned an empty
                    list.
                  </p>
                </EmptyBlock>
              ) : (
                <TableWell maxHeight="360px" label="Proposals">
                  <table className="dt">
                    <thead>
                      <tr>
                        <th scope="col">Id</th>
                        <th scope="col">Status</th>
                        <th scope="col" className="n">
                          Legs
                        </th>
                        <th scope="col" className="n">
                          Gross
                        </th>
                        <th scope="col" className="n">
                          Turnover
                        </th>
                        <th scope="col">Rationale</th>
                      </tr>
                    </thead>
                    <tbody>
                      {data.proposals.map((proposal) => (
                        <tr key={proposal.id}>
                          <td className="num">{proposal.id}</td>
                          <td>
                            <Chip
                              tone={
                                proposal.status === "approved"
                                  ? "ok"
                                  : proposal.status === "rejected"
                                    ? "bad"
                                    : "neutral"
                              }
                            >
                              {proposal.status}
                            </Chip>
                          </td>
                          <td className="n">{formatCount(proposal.legs)}</td>
                          <td className="n">{proposal.gross.toFixed(2)}</td>
                          <td className="n">{proposal.turnover.toFixed(2)}</td>
                          <td className="whitespace-normal">{proposal.rationale}</td>
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
        <PanelHead
          title="Signal stream"
          meta={<StreamControls stream={stream} name="signals" />}
        />
        <PanelBody flush>
          <EventFeed stream={stream} channel="signals" />
        </PanelBody>
      </Panel>
    </div>
  );
}
