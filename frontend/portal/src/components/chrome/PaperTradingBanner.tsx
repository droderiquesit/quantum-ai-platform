"use client";

import { usePlatform } from "./PlatformProvider";

/**
 * The declaration that this console cannot place a real order.
 *
 * Present on every route, above everything else, and never conditional on
 * state, a preference or a page. It carries two facts: that execution here is
 * simulated, and — read live from the platform — whether the process behind it
 * is even *capable* of live trading. The second is the one that matters during
 * an incident, because "paper only" is a property of the deployment and a
 * banner that asserts it without checking is a banner that will one day be
 * wrong.
 */
export function PaperTradingBanner() {
  const { health, status } = usePlatform();
  const liveCapable = health.data?.live_capable ?? status.data?.live_capable ?? null;

  return (
    <div className="paper-strip" role="note" aria-label="Trading mode">
      <div className="paper-strip-inner mx-[52px] flex h-[26px] items-center justify-center gap-3 px-3">
        <span
          className="text-[11px] font-bold tracking-[0.22em] text-[color:var(--color-paper)]"
          data-testid="paper-trading-banner"
        >
          PAPER TRADING
        </span>
        <span className="hidden text-[10.5px] tracking-[0.06em] text-[color:var(--color-ink-dim)] sm:inline">
          Simulated execution only — no capital is at risk from this console.
        </span>
        {liveCapable === true ? (
          <span
            className="chip"
            data-tone="bad"
            title="The platform reports live_capable = true. Nothing here can send a live order, but the process behind it is configured to be able to."
          >
            platform is live-capable
          </span>
        ) : null}
        {liveCapable === false ? (
          <span
            className="chip"
            data-tone="ok"
            title="The platform reports live_capable = false: it cannot reach a live venue."
          >
            platform paper-only
          </span>
        ) : null}
      </div>
    </div>
  );
}
