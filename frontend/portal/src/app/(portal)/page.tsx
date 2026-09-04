"use client";

import Link from "next/link";
import { useEffect, useMemo, useState } from "react";
import type { ChartConfiguration } from "chart.js";
import { Freshness } from "@/components/data/Bits";
import { RunCycleCard } from "@/components/data/RunCycle";
import { EmptyBlock, ResourceView } from "@/components/data/States";
import { Icon, type IconName } from "@/components/chrome/icons";
import { usePlatform } from "@/components/chrome/PlatformProvider";
import { ChartJs } from "@/components/viz/ChartJs";
import { platform } from "@/lib/api/client";
import type { Fills, Governance, Opportunities, SystemMetrics } from "@/lib/api/types";
import { formatCount, formatDecimal, formatPercent } from "@/lib/format";
import { useResource } from "@/lib/hooks/useResource";
import { describeWindow, useSeries, type Series } from "@/lib/hooks/useSeries";

/**
 * The executive dashboard, in the licensed template's composition (ADR 0015):
 * its hero, its four-card stat row, its performance panel with the side
 * column, its movers strip and trades table — with every figure read from the
 * platform. Where the template shows demo content this platform does not
 * serve (a name, a calendar, a year of price history), the block renders the
 * honest equivalent in the same visual position rather than the demo.
 */

interface SessionUser {
  readonly email: string;
  readonly displayName: string | null;
}

/** UTC, like every clock on this console. */
function greetingFor(hour: number): string {
  if (hour < 5) return "Working late";
  if (hour < 12) return "Good morning";
  if (hour < 18) return "Good afternoon";
  return "Good evening";
}

export default function ExecutiveDashboard() {
  const { health, status } = usePlatform();

  const metrics = useResource<SystemMetrics>(platform.systemMetrics, {
    key: "dash-metrics",
    label: "GET /system/metrics",
    intervalMs: 10_000,
  });
  const opportunities = useResource<Opportunities>(platform.opportunities, {
    key: "dash-opportunities",
    label: "GET /opportunities",
    intervalMs: 10_000,
  });
  const fills = useResource<Fills>(platform.fills, {
    key: "dash-fills",
    label: "GET /fills",
    intervalMs: 15_000,
  });
  const governance = useResource<Governance>(platform.governance, {
    key: "dash-governance",
    label: "GET /system/governance",
    intervalMs: 30_000,
  });

  const data = metrics.data;
  const cycles = useSeries(data?.cycles ?? null);
  const events = useSeries(data?.events_logged ?? null);
  const fillCount = useSeries(data?.fills ?? null);
  const refusals = useSeries(data?.refusals ?? null);

  const [user, setUser] = useState<SessionUser | null>(null);
  useEffect(() => {
    let cancelled = false;
    fetch("/api/auth/session", { cache: "no-store" })
      .then((response) => (response.ok ? response.json() : null))
      .then((body) => {
        if (!cancelled && body?.status === "authenticated") setUser(body.session.user);
      })
      .catch(() => {
        /* open console: greet the operator */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const name = user?.displayName ?? user?.email.split("@")[0] ?? "operator";
  const halted = health.data?.halted ?? status.data?.halted ?? null;

  const chartConfig = useMemo<ChartConfiguration>(
    () => ({
      type: "line",
      data: {
        labels: events.values.map((_, index) => `${index + 1}`),
        datasets: [
          {
            label: "events logged",
            data: [...events.values],
            borderColor: "var(--color-accent)",
            backgroundColor: "rgba(16, 185, 129, 0.12)",
            fill: true,
            tension: 0.35,
            pointRadius: 0,
            borderWidth: 2,
          },
        ],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: { legend: { display: false } },
        scales: {
          x: { display: false },
          y: {
            grid: { color: "var(--color-border)" },
            ticks: { color: "var(--color-muted)", precision: 0 },
          },
        },
      },
    }),
    [events.values],
  );

  return (
    <div className="inner-content max-sm:px-3 max-lg:px-4 max-lg:py-6 lg:p-6 space-y-6">
      {/* ── Hero ───────────────────────────────────────────────────────── */}
      <div className="relative overflow-hidden rounded-2xl bg-panel border border-border p-6">
        <div
          className="absolute top-0 right-0 w-96 h-96 bg-gradient-to-bl from-emerald-500/20 to-transparent rounded-full -translate-y-1/2 translate-x-1/3 pointer-events-none"
          aria-hidden="true"
        />
        <div className="relative flex flex-wrap items-end justify-between gap-4">
          <div>
            <h1 className="text-2xl lg:text-3xl font-bold text-text mb-1">
              {greetingFor(new Date().getUTCHours())}, {name}
            </h1>
            <p className="text-sm text-muted max-w-[60ch]">
              {halted === null
                ? "Reading the platform…"
                : halted
                  ? "The platform is halted. Nothing trades until the kill switch clears."
                  : "The loop is running. Execution is simulated end to end — paper trading, no capital at risk."}
            </p>
            <div className="flex flex-wrap items-center gap-2 mt-3">
              <span className="px-3 py-1 rounded-full bg-emerald-500/10 text-emerald-500 text-xs font-bold">
                {status.data ? `autonomy ${status.data.autonomy}` : "autonomy —"}
              </span>
              <span className="px-3 py-1 rounded-full bg-indigo-500/10 text-indigo-400 text-xs font-bold">
                PAPER TRADING
              </span>
              {health.data && health.data.reconciliation_breaks > 0 ? (
                <span className="px-3 py-1 rounded-full bg-red-500/10 text-red-500 text-xs font-bold">
                  {formatCount(health.data.reconciliation_breaks)} reconciliation break(s)
                </span>
              ) : null}
            </div>
          </div>
          <Link
            href="/loop"
            className="inline-flex items-center gap-2 px-5 py-3 rounded-xl bg-accent text-white text-sm font-semibold hover:opacity-90 transition-opacity"
          >
            <Icon name="waypoints" className="w-4 h-4" />
            Run the loop
          </Link>
        </div>
      </div>

      {/* ── Stat row ───────────────────────────────────────────────────── */}
      <div className="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-4 gap-4 lg:gap-5">
        <StatCard
          label="Cycles"
          value={formatCount(data?.cycles)}
          icon="waypoints"
          gradient="from-emerald-500 to-teal-500"
          bar="from-emerald-500 to-cyan-500"
          series={cycles}
        />
        <StatCard
          label="Events logged"
          value={formatCount(data?.events_logged)}
          icon="activity"
          gradient="from-sky-500 to-indigo-500"
          bar="from-sky-500 to-indigo-500"
          series={events}
        />
        <StatCard
          label="Fills (simulated)"
          value={formatCount(data?.fills)}
          icon="line-chart"
          gradient="from-emerald-500 to-cyan-500"
          bar="from-emerald-500 to-teal-500"
          series={fillCount}
          alarm={data?.live_fills ? "A LIVE FILL IS PRESENT" : undefined}
        />
        <StatCard
          label="Refusals"
          value={formatCount(data?.refusals)}
          icon="alert-triangle"
          gradient="from-amber-500 to-orange-500"
          bar="from-amber-500 to-orange-500"
          series={refusals}
        />
      </div>

      {/* ── Performance + side column ──────────────────────────────────── */}
      <div className="grid grid-cols-1 xl:grid-cols-3 gap-5 lg:gap-6">
        <div className="xl:col-span-2 rounded-2xl bg-panel border border-border p-6">
          <div className="flex flex-wrap items-center justify-between gap-3 mb-4">
            <h3 className="font-bold text-lg text-text">Performance Overview</h3>
            <div className="flex items-center gap-2">
              {(["1H", "1D", "1W"] as const).map((range) => (
                <button
                  key={range}
                  type="button"
                  disabled
                  title="History begins when a platform-side series exists — this line is what this tab has observed."
                  className="px-3 py-1.5 rounded-lg text-xs font-semibold bg-bg border border-border text-muted opacity-50 cursor-not-allowed"
                >
                  {range}
                </button>
              ))}
              <Freshness resource={metrics} name="metrics" />
            </div>
          </div>
          {events.values.length < 2 ? (
            <div className="flex h-[260px] items-center justify-center rounded-xl border border-dashed border-border text-sm text-muted">
              {events.values.length === 0
                ? "Nothing observed yet — the line begins with the second answer."
                : "One observation — a line needs at least two."}
            </div>
          ) : (
            <ChartJs config={chartConfig} height={260} label="Events logged, as observed by this tab" />
          )}
          <p className="mt-3 text-xs text-muted">
            Events logged, {describeWindow(events)}. The platform serves counters, not curves —
            this line is what this browser watched it do.
          </p>
        </div>

        <div className="space-y-5">
          {/* AI Market Summary — composed from real reads only. */}
          <div className="rounded-2xl bg-panel border border-border p-5">
            <div className="flex items-center justify-between mb-3">
              <h3 className="font-bold text-text">AI Market Summary</h3>
              <Icon name="sparkles" className="w-4 h-4 text-accent" />
            </div>
            <ResourceView resource={governance} loadingRows={3}>
              {(review) => (
                <p className="text-sm text-muted leading-relaxed">
                  The loop has run <span className="font-semibold text-text">{formatCount(status.data?.cycles)}</span>{" "}
                  cycle(s) and logged{" "}
                  <span className="font-semibold text-text">{formatCount(status.data?.events)}</span>{" "}
                  event(s). The opportunity queue holds{" "}
                  <span className="font-semibold text-text">
                    {formatCount(data?.opportunities_queued)}
                  </span>
                  . Governance reviewed {formatCount(review.agents)} agent(s):{" "}
                  {review.findings.length === 0 ? (
                    <span className="text-emerald-500 font-semibold">no findings</span>
                  ) : (
                    <span className="text-amber-500 font-semibold">
                      {formatCount(review.findings.length)} finding(s)
                    </span>
                  )}
                  . Every sentence here is composed from those reads — nothing is generated.
                </p>
              )}
            </ResourceView>
          </div>

          {/* Watchlist — the real queue. */}
          <div className="rounded-2xl bg-panel border border-border p-5">
            <div className="flex items-center justify-between mb-3">
              <h3 className="font-bold text-text">Watchlist</h3>
              <Freshness resource={opportunities} name="opportunities" />
            </div>
            <ResourceView resource={opportunities} loadingRows={4}>
              {(queue) =>
                queue.opportunities.length === 0 ? (
                  <EmptyBlock headline="The queue is empty.">
                    <p>
                      Measured, not assumed: <code className="num">GET /api/v1/opportunities</code>{" "}
                      returned an empty list. Run a cycle to fill it.
                    </p>
                  </EmptyBlock>
                ) : (
                  <ul className="space-y-2">
                    {[...queue.opportunities]
                      .sort((a, b) => b.score - a.score)
                      .slice(0, 4)
                      .map((opportunity) => (
                        <li
                          key={opportunity.id}
                          className="flex items-center justify-between gap-3 rounded-xl border border-border bg-bg px-3 py-2"
                        >
                          <div className="min-w-0">
                            <p className="num text-xs text-muted truncate">{opportunity.id}</p>
                            <p className="text-sm text-text truncate">{opportunity.headline}</p>
                          </div>
                          <span className="shrink-0 px-2 py-1 rounded-lg bg-emerald-500/10 text-emerald-500 text-xs font-bold">
                            {formatPercent(opportunity.confidence)}
                          </span>
                        </li>
                      ))}
                  </ul>
                )
              }
            </ResourceView>
          </div>

          {/* Economic Events — the honest absence, in the template's card. */}
          <div className="rounded-2xl bg-panel border border-border p-5">
            <h3 className="font-bold text-text mb-3">Economic Events</h3>
            <p className="text-sm text-muted leading-relaxed">
              No economic-calendar source is connected.{" "}
              <code className="num">GET /api/v1/calendar</code> is not served, and this console
              does not invent a schedule to fill the card.
            </p>
          </div>
        </div>
      </div>

      {/* ── Top movers ─────────────────────────────────────────────────── */}
      <div className="rounded-2xl bg-panel border border-border p-6">
        <div className="flex items-center justify-between mb-4">
          <h3 className="font-bold text-text">Top Movers — simulated fills by instrument</h3>
          <Freshness resource={fills} name="fills" />
        </div>
        <ResourceView resource={fills} loadingRows={2}>
          {(book) => <TopMovers book={book} />}
        </ResourceView>
      </div>

      {/* ── Recent trades ──────────────────────────────────────────────── */}
      <div className="rounded-2xl bg-panel border border-border overflow-hidden">
        <div className="flex items-center justify-between p-6 pb-4">
          <h3 className="font-bold text-lg text-text">Recent Trades</h3>
          <Link href="/execution/fills" className="text-sm font-semibold text-accent hover:underline">
            View all
          </Link>
        </div>
        <ResourceView resource={fills} loadingRows={4}>
          {(book) =>
            book.fills.length === 0 ? (
              <div className="px-6 pb-6">
                <EmptyBlock headline="No fill has occurred.">
                  <p>The simulator has filled nothing yet — an empty book, not a failed read.</p>
                </EmptyBlock>
              </div>
            ) : (
              <div className="overflow-x-auto">
                {book.any_live_fill ? (
                  <p className="mx-6 mb-3 rounded-xl bg-red-500/10 px-4 py-2 text-sm font-bold text-red-500">
                    A LIVE FILL IS PRESENT — this platform must never produce one. Investigate
                    before trusting anything else on this page.
                  </p>
                ) : null}
                <table className="w-full text-sm">
                  <thead>
                    <tr className="text-left text-xs uppercase tracking-wider text-muted border-b border-border">
                      <th className="px-6 py-3 font-semibold">Order</th>
                      <th className="px-6 py-3 font-semibold">Instrument</th>
                      <th className="px-6 py-3 font-semibold">Side</th>
                      <th className="px-6 py-3 font-semibold text-right">Quantity</th>
                      <th className="px-6 py-3 font-semibold text-right">Price</th>
                      <th className="px-6 py-3 font-semibold">Venue</th>
                      <th className="px-6 py-3 font-semibold">Mode</th>
                    </tr>
                  </thead>
                  <tbody>
                    {book.fills.slice(-6).reverse().map((fill) => (
                      <tr
                        key={`${fill.order}-${fill.instrument}-${fill.price}`}
                        className="border-b border-border last:border-b-0 hover:bg-border/20"
                      >
                        <td className="px-6 py-3 num text-muted">{fill.order}</td>
                        <td className="px-6 py-3 font-semibold text-text">{fill.instrument}</td>
                        <td className="px-6 py-3">
                          <span
                            className={`px-2 py-0.5 rounded-md text-xs font-bold ${
                              fill.side === "buy"
                                ? "bg-emerald-500/10 text-emerald-500"
                                : "bg-red-500/10 text-red-500"
                            }`}
                          >
                            {fill.side.toUpperCase()}
                          </span>
                        </td>
                        <td className="px-6 py-3 num text-right">{formatDecimal(fill.quantity)}</td>
                        <td className="px-6 py-3 num text-right">{formatDecimal(fill.price)}</td>
                        <td className="px-6 py-3 num text-muted">{fill.venue}</td>
                        <td className="px-6 py-3">
                          <span
                            className={`px-2 py-0.5 rounded-md text-xs font-bold ${
                              fill.simulated
                                ? "bg-emerald-500/10 text-emerald-500"
                                : "bg-red-500/10 text-red-500"
                            }`}
                          >
                            {fill.simulated ? "SIMULATED" : "LIVE"}
                          </span>
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

      <RunCycleCard
        onRan={() => {
          metrics.refresh();
          opportunities.refresh();
          fills.refresh();
          status.refresh();
        }}
      />
    </div>
  );
}

/**
 * The template's stat card. Its progress bar is given real semantics: the
 * current value's share of the largest value this tab has observed, said in
 * the title — never an invented "+12% vs yesterday".
 */
function StatCard({
  label,
  value,
  icon,
  gradient,
  bar,
  series,
  alarm,
}: {
  label: string;
  value: string;
  icon: IconName;
  gradient: string;
  bar: string;
  series: Series;
  alarm?: string;
}) {
  const peak = series.values.reduce((top, entry) => Math.max(top, entry), 0);
  const current = series.values[series.values.length - 1] ?? 0;
  const share = peak > 0 ? Math.round((current / peak) * 100) : 0;

  return (
    <div className="rounded-2xl bg-panel border border-border p-5 hover:-translate-y-1 hover:shadow-2xl hover:shadow-accent/10 transition duration-300">
      <div className="flex items-center justify-between mb-4">
        <div
          className={`w-12 h-12 rounded-2xl bg-gradient-to-br ${gradient} flex items-center justify-center`}
        >
          <Icon name={icon} className="w-6 h-6 text-white shrink-0" />
        </div>
        {alarm ? (
          <span className="px-2 py-1 rounded-md bg-red-500/10 text-red-500 text-[10px] font-bold">
            {alarm}
          </span>
        ) : null}
      </div>
      <p className="text-sm text-muted mb-1">{label}</p>
      <p className="text-2xl font-bold text-text num">{value}</p>
      <div
        className="flex items-center gap-2 mt-3"
        title={`${share}% of the largest value this tab has observed`}
      >
        <div className="flex-1 h-1.5 bg-border rounded-full overflow-hidden">
          <div
            className={`h-full bg-gradient-to-r ${bar} rounded-full`}
            style={{ width: `${share}%` }}
          />
        </div>
        <span className="text-xs text-muted whitespace-nowrap">{describeWindow(series)}</span>
      </div>
    </div>
  );
}

/** Fills netted by instrument; direction is the sign of net signed quantity. */
function TopMovers({ book }: { book: Fills }) {
  const net = new Map<string, number>();
  for (const fill of book.fills) {
    // Quantities are decimal strings; only their sign and relative magnitude
    // are needed here, so parseFloat is acceptable for ordering — the numbers
    // shown to the reader below are counts, not the parsed values.
    const quantity = Number.parseFloat(fill.quantity) * (fill.side === "buy" ? 1 : -1);
    net.set(fill.instrument, (net.get(fill.instrument) ?? 0) + quantity);
  }
  const movers = [...net.entries()].sort((a, b) => Math.abs(b[1]) - Math.abs(a[1])).slice(0, 6);

  if (movers.length === 0) {
    return <EmptyBlock headline="No fill has occurred — nothing has moved." />;
  }
  const counts = new Map<string, number>();
  for (const fill of book.fills) counts.set(fill.instrument, (counts.get(fill.instrument) ?? 0) + 1);

  return (
    <div className="grid grid-cols-2 sm:grid-cols-3 xl:grid-cols-6 gap-3">
      {movers.map(([instrument, signed]) => (
        <div key={instrument} className="rounded-xl border border-border bg-bg p-3">
          <p className="num text-xs text-muted truncate">{instrument}</p>
          <p
            className={`text-lg font-bold ${signed >= 0 ? "text-emerald-500" : "text-red-500"}`}
          >
            {signed >= 0 ? "▲ net long" : "▼ net short"}
          </p>
          <p className="text-xs text-muted">{formatCount(counts.get(instrument))} fill(s)</p>
        </div>
      ))}
    </div>
  );
}
