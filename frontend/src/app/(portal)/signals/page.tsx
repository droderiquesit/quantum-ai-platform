"use client";

import { useMemo, useState } from "react";
import { Freshness, StreamControls } from "@/components/data/Bits";
import { EventFeed } from "@/components/data/EventFeed";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { Icon } from "@/components/chrome/icons";
import { platform } from "@/lib/api/client";
import type { Opportunities, Opportunity, Proposals } from "@/lib/api/types";
import { formatCount, formatPercent } from "@/lib/format";
import { useEventStream } from "@/lib/hooks/useEventStream";
import { useResource } from "@/lib/hooks/useResource";

/**
 * Live opportunities, in the licensed template's live-signals composition
 * (ADR 0015): the four-stat strip, the filterable card grid, the history
 * table, the live feed. Algorik's glossary calls these opportunities — the
 * platform's own word — so the template's "signal" copy becomes
 * "opportunity" while its structure and rhythm stay.
 *
 * Every figure is read from the platform. The filter asserts its premise
 * ("matched X of Y") so a filtered-empty grid can never be mistaken for an
 * empty queue, and an empty queue never for a failed read.
 */

const CONFIDENCE_STEPS = [
  { label: "All", floor: 0 },
  { label: "≥ 50%", floor: 0.5 },
  { label: "≥ 75%", floor: 0.75 },
] as const;

export default function LiveOpportunities() {
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

  const [query, setQuery] = useState("");
  const [floor, setFloor] = useState(0);

  const queue = useMemo(() => opportunities.data?.opportunities ?? [], [opportunities.data]);
  const matched = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return queue.filter(
      (entry) =>
        entry.confidence >= floor &&
        (needle === "" ||
          entry.id.toLowerCase().includes(needle) ||
          entry.headline.toLowerCase().includes(needle)),
    );
  }, [queue, query, floor]);

  const best = queue.reduce<number | null>(
    (top, entry) => (top === null || entry.score > top ? entry.score : top),
    null,
  );
  const meanConfidence =
    queue.length === 0
      ? null
      : queue.reduce((sum, entry) => sum + entry.confidence, 0) / queue.length;

  return (
    <div className="inner-content max-sm:px-3 max-lg:px-4 max-lg:py-6 lg:p-6 space-y-6">
      {/* ── Title ──────────────────────────────────────────────────────── */}
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-2xl lg:text-3xl font-bold text-text mb-1">Live Opportunities</h1>
          <p className="text-sm text-muted">
            What DISCOVER found and REASON has not yet ruled on — polled from the platform, never
            invented.
          </p>
        </div>
        <Freshness resource={opportunities} name="opportunities" />
      </div>

      {/* ── Stat strip — the template's four small cards ───────────────── */}
      <div className="grid grid-cols-1 xs:grid-cols-2 xl:grid-cols-4 gap-4 sm:gap-6">
        <MiniStat
          icon="radio"
          tint="text-cyan-500"
          well="bg-cyan-500/10"
          value={formatCount(queue.length)}
          label="In the queue"
        />
        <MiniStat
          icon="table-2"
          tint="text-emerald-500"
          well="bg-emerald-500/10"
          value={formatCount(proposals.data?.proposals.length)}
          label="Proposals"
        />
        <MiniStat
          icon="sparkles"
          tint="text-indigo-400"
          well="bg-indigo-500/10"
          value={best === null ? "—" : best.toFixed(3)}
          label="Best score"
        />
        <MiniStat
          icon="activity"
          tint="text-amber-500"
          well="bg-amber-500/10"
          value={meanConfidence === null ? "—" : formatPercent(meanConfidence)}
          label="Mean confidence"
        />
      </div>

      {/* ── Filter bar ─────────────────────────────────────────────────── */}
      <div className="flex flex-wrap items-center gap-3 rounded-2xl bg-panel border border-border p-4">
        <div className="relative flex-1 min-w-[220px]">
          <Icon
            name="search"
            className="w-4 h-4 absolute left-3.5 top-1/2 -translate-y-1/2 text-muted pointer-events-none"
          />
          <input
            type="text"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Filter by id or headline…"
            aria-label="Filter opportunities"
            className="w-full pl-10 pr-4 py-2.5 rounded-xl bg-bg border border-border text-sm text-text placeholder:text-muted focus:outline-none focus:border-accent"
          />
        </div>
        <div className="flex items-center gap-2" role="group" aria-label="Minimum confidence">
          {CONFIDENCE_STEPS.map((step) => (
            <button
              key={step.label}
              type="button"
              onClick={() => setFloor(step.floor)}
              aria-pressed={floor === step.floor}
              className={`px-3 py-1.5 rounded-lg text-xs font-semibold border transition-colors ${
                floor === step.floor
                  ? "bg-accent text-white border-accent"
                  : "bg-bg border-border text-muted hover:text-text"
              }`}
            >
              {step.label}
            </button>
          ))}
        </div>
        {/* The premise, always on screen: a filtered-empty grid must never
            read as an empty queue. */}
        <span className="ml-auto px-3 py-1.5 rounded-full bg-border/40 text-xs font-semibold text-muted">
          matched {formatCount(matched.length)} of {formatCount(queue.length)}
        </span>
      </div>

      {/* ── Opportunity cards ──────────────────────────────────────────── */}
      <ResourceView resource={opportunities} loadingRows={6}>
        {() =>
          queue.length === 0 ? (
            <EmptyBlock headline="The queue is empty.">
              <p>
                Measured, not assumed: <code className="num">GET /api/v1/opportunities</code>{" "}
                returned an empty list. Run a cycle to fill it.
              </p>
            </EmptyBlock>
          ) : matched.length === 0 ? (
            <EmptyBlock headline="No opportunity matches the filter.">
              <p>
                The queue holds {formatCount(queue.length)} — the filter, not the platform,
                emptied this grid.
              </p>
            </EmptyBlock>
          ) : (
            <div className="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-3 gap-4 sm:gap-5">
              {matched.map((opportunity) => (
                <OpportunityCard key={opportunity.id} opportunity={opportunity} />
              ))}
            </div>
          )
        }
      </ResourceView>

      {/* ── History — proposals, in the template's table dress ─────────── */}
      <div className="rounded-2xl bg-panel border border-border overflow-hidden">
        <div className="flex items-center justify-between p-6 pb-4">
          <h2 className="font-bold text-lg text-text">Recent Proposals</h2>
          <Freshness resource={proposals} name="proposals" />
        </div>
        <ResourceView resource={proposals} loadingRows={4}>
          {(data) =>
            data.proposals.length === 0 ? (
              <div className="px-6 pb-6">
                <EmptyBlock headline="Nothing has been proposed.">
                  <p>
                    Either nothing reached the action bar, or every thesis that did was rejected
                    in review.
                  </p>
                </EmptyBlock>
              </div>
            ) : (
              <div className="overflow-x-auto">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="text-left text-xs uppercase tracking-wider text-muted border-b border-border">
                      <th className="px-6 py-3 font-semibold">Id</th>
                      <th className="px-6 py-3 font-semibold">Status</th>
                      <th className="px-6 py-3 font-semibold text-right">Legs</th>
                      <th className="px-6 py-3 font-semibold text-right">Gross</th>
                      <th className="px-6 py-3 font-semibold text-right">Turnover</th>
                      <th className="px-6 py-3 font-semibold">Rationale</th>
                    </tr>
                  </thead>
                  <tbody>
                    {data.proposals.map((proposal) => (
                      <tr
                        key={proposal.id}
                        className="border-b border-border last:border-b-0 hover:bg-border/20"
                      >
                        <td className="px-6 py-3 num text-muted">{proposal.id}</td>
                        <td className="px-6 py-3">
                          <span
                            className={`px-2 py-0.5 rounded-md text-xs font-bold ${
                              proposal.status === "approved"
                                ? "bg-emerald-500/10 text-emerald-500"
                                : proposal.status === "rejected"
                                  ? "bg-red-500/10 text-red-500"
                                  : "bg-border/50 text-muted"
                            }`}
                          >
                            {proposal.status.toUpperCase()}
                          </span>
                        </td>
                        <td className="px-6 py-3 num text-right">{formatCount(proposal.legs)}</td>
                        <td className="px-6 py-3 num text-right">{proposal.gross.toFixed(2)}</td>
                        <td className="px-6 py-3 num text-right">{proposal.turnover.toFixed(2)}</td>
                        <td className="px-6 py-3 text-muted max-w-[36ch] truncate" title={proposal.rationale}>
                          {proposal.rationale}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )
          }
        </ResourceView>
      </div>

      {/* ── The live feed ──────────────────────────────────────────────── */}
      <div className="rounded-2xl bg-panel border border-border overflow-hidden">
        <div className="flex flex-wrap items-center justify-between gap-3 p-6 pb-4">
          <h2 className="flex items-center gap-2 font-bold text-lg text-text">
            <span className="live-dot" aria-hidden="true" />
            Opportunity Stream
          </h2>
          <StreamControls stream={stream} name="signals" />
        </div>
        <EventFeed stream={stream} channel="signals" />
      </div>
    </div>
  );
}

/** The template's small stat card: icon well, number, caption. */
function MiniStat({
  icon,
  tint,
  well,
  value,
  label,
}: {
  icon: Parameters<typeof Icon>[0]["name"];
  tint: string;
  well: string;
  value: string;
  label: string;
}) {
  return (
    <div className="rounded-2xl bg-panel border border-border p-5">
      <div className="flex items-center justify-between mb-3">
        <div className={`w-10 h-10 rounded-xl ${well} flex items-center justify-center`}>
          <Icon name={icon} className={`w-5 h-5 ${tint} shrink-0`} />
        </div>
      </div>
      <p className="text-2xl font-bold text-text num">{value}</p>
      <p className="text-sm text-muted">{label}</p>
    </div>
  );
}

/**
 * The template's signal card, carrying an opportunity. Its take-profit /
 * stop-loss slots are not faked: this platform's queue carries score,
 * confidence and detectors, so those are what the card shows.
 */
function OpportunityCard({ opportunity }: { opportunity: Opportunity }) {
  const confidence = Math.round(opportunity.confidence * 100);
  return (
    <div className="rounded-2xl bg-panel border border-border p-5 hover:border-accent/40 transition-colors">
      <div className="flex items-start justify-between gap-3 mb-3">
        <div className="min-w-0">
          <h3 className="text-lg font-bold text-text truncate">{opportunity.id}</h3>
          <p className="text-sm text-muted line-clamp-2">{opportunity.headline}</p>
        </div>
        <span className="shrink-0 px-2.5 py-1 rounded-lg text-xs font-semibold bg-emerald-500/15 text-emerald-500">
          score {opportunity.score.toFixed(3)}
        </span>
      </div>
      <div className="flex items-center gap-2 mb-3" title={`confidence ${confidence}%`}>
        <div className="flex-1 h-1.5 bg-border rounded-full overflow-hidden">
          <div
            className="h-full bg-gradient-to-r from-emerald-500 to-cyan-500 rounded-full"
            style={{ width: `${confidence}%` }}
          />
        </div>
        <span className="text-xs font-bold text-emerald-500 whitespace-nowrap">{confidence}%</span>
      </div>
      <div className="flex flex-wrap gap-1.5">
        {opportunity.detectors.length === 0 ? (
          <span className="text-xs text-muted">no detector named</span>
        ) : (
          opportunity.detectors.map((detector) => (
            <span
              key={detector}
              className="px-2 py-0.5 rounded-md bg-border/40 text-[11px] font-medium text-muted"
            >
              {detector}
            </span>
          ))
        )}
      </div>
    </div>
  );
}
