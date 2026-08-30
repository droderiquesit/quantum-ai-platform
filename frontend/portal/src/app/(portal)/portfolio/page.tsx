"use client";

import { useMemo } from "react";
import type { ChartConfiguration } from "chart.js";
import { Freshness, StreamControls } from "@/components/data/Bits";
import { EventFeed } from "@/components/data/EventFeed";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { Icon } from "@/components/chrome/icons";
import { ChartJs } from "@/components/viz/ChartJs";
import { platform } from "@/lib/api/client";
import type { Capital, Portfolio } from "@/lib/api/types";
import { formatCount, formatDecimal, formatTimestamp } from "@/lib/format";
import { useEventStream } from "@/lib/hooks/useEventStream";
import { useResource } from "@/lib/hooks/useResource";

/**
 * The portfolio, in the licensed template's my-portfolio composition
 * (ADR 0015): the four-card strip, holdings table, allocation donut, and an
 * insights card — bound to what this platform actually serves. That is
 * counts, the paper-only flag, and capital envelopes; position rows and cash
 * balances sit behind the desk's capability gate and are not served over
 * HTTP. Where the template shows a dollar balance, this page says exactly
 * that, in the same visual slot. A fabricated balance on a portfolio page is
 * the most dangerous pixel this console could draw.
 */

/** Doughnut slice palette — the template's category hues, via tokens. */
const SLICES = [
  "var(--color-accent)",
  "var(--color-secondary)",
  "var(--color-teal)",
  "var(--color-orange)",
  "var(--color-info)",
  "var(--color-crypto)",
];

export default function PortfolioOverview() {
  const portfolio = useResource<Portfolio>(platform.portfolio, {
    key: "portfolio-summary",
    label: "GET /portfolio",
    intervalMs: 15_000,
  });
  const capital = useResource<Capital>(platform.capital, {
    key: "portfolio-capital",
    label: "GET /capital",
    intervalMs: 20_000,
  });
  const stream = useEventStream({ channel: "positions", label: "positions stream" });

  const envelopes = useMemo(() => capital.data?.envelopes ?? [], [capital.data]);

  // Allocation by cell, counted — never money strings parsed into floats.
  const byCell = useMemo(() => {
    const cells = new Map<string, number>();
    for (const envelope of envelopes) {
      cells.set(envelope.cell, (cells.get(envelope.cell) ?? 0) + 1);
    }
    return [...cells.entries()].sort((a, b) => b[1] - a[1]);
  }, [envelopes]);

  const donut = useMemo<ChartConfiguration>(
    () => ({
      type: "doughnut",
      data: {
        labels: byCell.map(([cell]) => cell),
        datasets: [
          {
            data: byCell.map(([, count]) => count),
            backgroundColor: byCell.map((_, index) => SLICES[index % SLICES.length]),
            borderWidth: 0,
          },
        ],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        cutout: "68%",
        plugins: {
          legend: { position: "bottom", labels: { color: "var(--color-muted)", boxWidth: 10 } },
        },
      },
    }),
    [byCell],
  );

  const book = portfolio.data;

  return (
    <div className="inner-content max-sm:px-3 max-lg:px-4 max-lg:py-6 lg:p-6 space-y-6">
      {/* ── Title ──────────────────────────────────────────────────────── */}
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-2xl lg:text-3xl font-bold text-text mb-1">Portfolio</h1>
          <p className="text-sm text-muted max-w-[70ch]">
            Counts and capital grants, read live. Position rows and cash balances are behind the
            desk&rsquo;s capability gate — <code className="num">GET /api/v1/positions</code> is
            not served — and this page says so rather than inventing a balance.
          </p>
        </div>
        <Freshness resource={portfolio} name="portfolio" />
      </div>

      {/* ── The template's four-card strip ─────────────────────────────── */}
      <div className="grid grid-cols-1 xs:grid-cols-2 xl:grid-cols-4 gap-4">
        <HoldingStat
          gradient="from-emerald-500 to-teal-500"
          icon="table-2"
          label="Proposals"
          value={formatCount(book?.proposals)}
        />
        <HoldingStat
          gradient="from-sky-500 to-indigo-500"
          icon="activity"
          label="Orders"
          value={formatCount(book?.orders)}
        />
        <HoldingStat
          gradient="from-emerald-500 to-cyan-500"
          icon="line-chart"
          label="Fills"
          value={formatCount(book?.fills)}
        />
        <div className="rounded-2xl bg-panel border border-border p-5 hover:border-accent/40 transition-colors">
          <div className="flex items-start justify-between mb-4">
            <div
              className={`w-12 h-12 rounded-xl bg-gradient-to-br ${
                book === null || book.paper_only ? "from-indigo-500 to-violet-500" : "from-red-500 to-rose-600"
              } flex items-center justify-center shrink-0`}
            >
              <Icon name="shield-check" className="w-6 h-6 text-white shrink-0" />
            </div>
            {book !== null && !book.paper_only ? (
              <span className="inline-flex items-center gap-1 px-2.5 py-1 rounded-lg text-xs font-semibold bg-red-500/15 text-red-500">
                ALARM
              </span>
            ) : null}
          </div>
          <p className="text-sm text-muted mb-1">Execution mode</p>
          <p
            className={`text-2xl font-bold ${
              book === null ? "text-text" : book.paper_only ? "text-text" : "text-red-500"
            }`}
          >
            {book === null ? "—" : book.paper_only ? "PAPER ONLY" : "LIVE FILLS PRESENT"}
          </p>
        </div>
      </div>

      {/* ── Holdings + allocation ──────────────────────────────────────── */}
      <div className="grid grid-cols-1 xl:grid-cols-3 gap-5 lg:gap-6">
        <div className="xl:col-span-2 rounded-2xl bg-panel border border-border overflow-hidden">
          <div className="flex items-center justify-between p-6 pb-4">
            <h3 className="font-bold text-lg text-text">Capital Holdings — envelopes</h3>
            <Freshness resource={capital} name="capital" />
          </div>
          <ResourceView resource={capital} loadingRows={5}>
            {(grants) =>
              grants.envelopes.length === 0 ? (
                <div className="px-6 pb-6">
                  <EmptyBlock headline="No envelope is outstanding.">
                    <p>
                      No cell holds a grant right now — the desk has nothing deployed, which is a
                      measured fact, not a failed read.
                    </p>
                  </EmptyBlock>
                </div>
              ) : (
                <div className="overflow-x-auto">
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="text-left text-xs uppercase tracking-wider text-muted border-b border-border">
                        <th className="px-6 py-3 font-semibold">Cell</th>
                        <th className="px-6 py-3 font-semibold">Strategy</th>
                        <th className="px-6 py-3 font-semibold text-right">Gross limit</th>
                        <th className="px-6 py-3 font-semibold">Use</th>
                        <th className="px-6 py-3 font-semibold">Expires</th>
                      </tr>
                    </thead>
                    <tbody>
                      {grants.envelopes.map((envelope) => (
                        <tr
                          key={`${envelope.cell}:${envelope.strategy}`}
                          className="border-b border-border last:border-b-0 hover:bg-border/20"
                        >
                          <td className="px-6 py-3 num text-text">{envelope.cell}</td>
                          <td className="px-6 py-3 num text-muted">{envelope.strategy}</td>
                          <td className="px-6 py-3 num text-right">
                            {formatDecimal(envelope.gross_limit)}
                          </td>
                          <td className="px-6 py-3">
                            {envelope.used.reported ? (
                              <span className="px-2 py-0.5 rounded-md bg-emerald-500/10 text-emerald-500 text-xs font-bold">
                                {formatDecimal(envelope.used.gross_committed)} committed
                              </span>
                            ) : (
                              <span
                                className="px-2 py-0.5 rounded-md bg-amber-500/10 text-amber-500 text-xs font-bold"
                                title={envelope.used.reason}
                              >
                                NOT REPORTED
                              </span>
                            )}
                          </td>
                          <td className="px-6 py-3 num text-muted text-xs">
                            {formatTimestamp(envelope.expires_at)}
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

        <div className="space-y-5">
          {/* Allocation donut over envelope counts per cell. */}
          <div className="rounded-2xl bg-panel border border-border p-5">
            <h3 className="font-bold text-text mb-3">Portfolio Allocation</h3>
            {byCell.length === 0 ? (
              <p className="text-sm text-muted leading-relaxed">
                Nothing to allocate a chart to: no capital envelope is outstanding. The donut
                draws when a grant exists.
              </p>
            ) : (
              <>
                <ChartJs config={donut} height={220} label="Envelopes per cell" />
                <p className="mt-3 text-xs text-muted">
                  Slices are envelope <em>counts</em> per cell. Money shares are not drawn — gross
                  limits are decimal strings this console never parses into floats.
                </p>
              </>
            )}
          </div>

          {/* The template's AI Insights card, carrying the honest position. */}
          <div className="rounded-2xl bg-panel border border-border p-5">
            <div className="flex items-center justify-between mb-3">
              <h3 className="font-bold text-text">Insights</h3>
              <Icon name="sparkles" className="w-4 h-4 text-accent" />
            </div>
            <p className="text-sm text-muted leading-relaxed">
              Position-level insight needs position-level data, and the platform gates that
              behind the desk capability. When <code className="num">GET /api/v1/positions</code>{" "}
              exists, this card carries attribution; until then it carries this sentence, because
              generated commentary about an unreadable book would be fiction with a chart.
            </p>
          </div>
        </div>
      </div>

      {/* ── Live positions feed ────────────────────────────────────────── */}
      <div className="rounded-2xl bg-panel border border-border overflow-hidden">
        <div className="flex flex-wrap items-center justify-between gap-3 p-6 pb-4">
          <h3 className="flex items-center gap-2 font-bold text-lg text-text">
            <span className="live-dot" aria-hidden="true" />
            Positions Stream
          </h3>
          <StreamControls stream={stream} name="positions" />
        </div>
        <EventFeed stream={stream} channel="positions" />
      </div>
    </div>
  );
}

/** The template's holding card: gradient icon square, number, caption. */
function HoldingStat({
  gradient,
  icon,
  label,
  value,
}: {
  gradient: string;
  icon: Parameters<typeof Icon>[0]["name"];
  label: string;
  value: string;
}) {
  return (
    <div className="rounded-2xl bg-panel border border-border p-5 hover:border-accent/40 transition-colors">
      <div className="flex items-start justify-between mb-4">
        <div
          className={`w-12 h-12 rounded-xl bg-gradient-to-br ${gradient} flex items-center justify-center shrink-0`}
        >
          <Icon name={icon} className="w-6 h-6 text-white shrink-0" />
        </div>
      </div>
      <p className="text-sm text-muted mb-1">{label}</p>
      <p className="text-2xl font-bold text-text num">{value}</p>
    </div>
  );
}
